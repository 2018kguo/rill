//! Where Streamdown cuts a text into blocks: `marked`'s (v18, GFM) top-level tokens, a
//! block per token, blank lines joining the block before. Streamdown parses each block
//! on its own, so the cuts decide what continues into what. Only the tokens' extents are
//! computed, after `marked`'s tokenizer (`Lexer.blockTokens`, `Tokenizer.blockquote` and
//! `list`, the block rules); what the renderer does not cover yet is [`Fallback`]:
//! fences, HTML, definitions, tables, indented code, math.

use crate::simd::ByteSet;
use crate::{Fallback, NL, Result, SP, newline, run};

const TAB: u16 = 9;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Space,
    Heading,
    Hr,
    Blockquote,
    List,
    Paragraph,
}

struct Token {
    kind: Kind,
    /// Where it starts in the text lexed (meaningful at the top level only).
    start: usize,
    /// A blockquote's or list's `raw` (later lines may be lexed with it again).
    raw: Vec<u16>,
}

/// The UTF-16 offsets where Streamdown's blocks of `text` start, the first excepted.
pub(crate) fn cuts(text: &[u16]) -> Result<Vec<usize>> {
    precheck(text)?;
    let mut tokens = Vec::new();
    lex(text, &mut tokens, false)?;
    Ok(tokens.iter().filter(|token| token.kind != Kind::Space && token.start > 0).map(|token| token.start).collect())
}

/// Fallback for what may make a token this does not lex: fences (```, ~~~), math
/// (`$$`), definitions (`]:`), HTML at a line's start (after indents and quote markers).
/// 16 units at a time, looking closer only at those characters.
fn precheck(text: &[u16]) -> Result {
    const SET: ByteSet = ByteSet::new(b"`~$]<");
    let fails = |at: usize| {
        let unit = text[at];
        let next = |offset: usize| text.get(at + offset).copied();
        match unit {
            0x60 | 0x7E => next(1) == Some(unit) && next(2) == Some(unit),
            0x24 => next(1) == Some(0x24),
            0x5D => next(1) == Some(u16::from(b':')),
            _ => text[..at].iter().rev().find(|&&u| u != SP && u != u16::from(b'>')).is_none_or(|&u| u == NL),
        }
    };
    let mut chunks = text.chunks_exact(16);
    for (index, chunk) in chunks.by_ref().enumerate() {
        let mut mask = SET.mask16_units(chunk.try_into().expect("16 units"));
        while mask != 0 {
            if fails(index * 16 + mask.trailing_zeros() as usize) {
                return Err(Fallback);
            }
            mask &= mask - 1;
        }
    }
    let rest = text.len() - chunks.remainder().len();
    for (at, &unit) in text.iter().enumerate().skip(rest) {
        if unit < 0x80 && SET.contains(unit as u8) && fails(at) {
            return Err(Fallback);
        }
    }
    Ok(())
}

fn lex(src: &[u16], tokens: &mut Vec<Token>, mut clipped: bool) -> Result {
    let mut pos = 0;
    while pos < src.len() {
        let s = &src[pos..];
        let spaces = space(s);
        if spaces > 0 {
            match tokens.last_mut() {
                Some(last) if spaces == 1 => last.raw.push(NL),
                _ => tokens.push(Token { kind: Kind::Space, start: pos, raw: Vec::new() }),
            }
            pos += spaces;
            continue;
        }
        let first = line(s, 0);
        if run(first, SP) >= 4 {
            return Err(Fallback); // indented code
        }
        let (kind, length) = if heading(first, 3) {
            (Kind::Heading, first.len())
        } else if hr(first, 3) {
            (Kind::Hr, first.len())
        } else if let Some(length) = blockquote(s)? {
            (Kind::Blockquote, length)
        } else if let Some(length) = list(s) {
            (Kind::List, length)
        } else if table(s) {
            return Err(Fallback);
        } else if let Some(length) = lheading(s) {
            (Kind::Heading, length)
        } else {
            let length = paragraph(s, 0, true);
            let merge = clipped && tokens.last().is_some_and(|last| last.kind == Kind::Paragraph);
            clipped = false;
            pos += length;
            if !merge {
                tokens.push(Token { kind: Kind::Paragraph, start: pos - length, raw: Vec::new() });
            }
            continue;
        };
        let raw = if matches!(kind, Kind::Blockquote | Kind::List) { s[..length].to_vec() } else { Vec::new() };
        tokens.push(Token { kind, start: pos, raw });
        pos += length;
    }
    Ok(())
}

fn line(s: &[u16], at: usize) -> &[u16] {
    &s[at..newline(s, at)]
}

/// JavaScript's `\s`.
fn js_space(unit: u16) -> bool {
    matches!(unit, 0x09..=0x0D | 0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 | 0xFEFF)
}

fn blank(line: &[u16]) -> bool {
    line.iter().all(|&u| u == SP || u == TAB)
}

/// `!line.trim()`.
fn trims_empty(line: &[u16]) -> bool {
    line.iter().all(|&u| js_space(u))
}

fn is(unit: Option<&u16>, byte: u8) -> bool {
    unit == Some(&u16::from(byte))
}

/// `^(?:[ \t]*(?:\n|$))+`.
fn space(s: &[u16]) -> usize {
    let mut pos = 0;
    loop {
        let end = newline(s, pos);
        if !blank(&s[pos..end]) {
            return pos;
        }
        if end == s.len() {
            return end;
        }
        pos = end + 1;
    }
}

/// ` {0,max}((?:-[\t ]*){3,}|(?:_[ \t]*){3,}|(?:\*[ \t]*){3,})` to the line's end.
fn hr(line: &[u16], max: usize) -> bool {
    let spaces = run(line, SP);
    let rest = &line[spaces..];
    let Some(&c) = rest.first() else { return false };
    if spaces > max || !matches!(c, 0x2D | 0x5F | 0x2A) {
        return false;
    }
    rest.iter().all(|&u| u == c || u == SP || u == TAB) && rest.iter().filter(|&&u| u == c).count() >= 3
}

/// ` {0,max}#{1,6}(?:\s|$)`.
fn heading(line: &[u16], max: usize) -> bool {
    let spaces = run(line, SP);
    let hashes = run(&line[spaces..], u16::from(b'#'));
    spaces <= max && (1..=6).contains(&hashes) && line.get(spaces + hashes).is_none_or(|&u| js_space(u))
}

/// ` {0,max}>`.
fn quote(line: &[u16], max: usize) -> bool {
    let spaces = run(line, SP);
    spaces <= max && is(line.get(spaces), b'>')
}

/// A list marker after at most `max` spaces (`[*+-]|\d{1,9}[.)]`): where it ends, and
/// its bullet or its digits' count and delimiter.
fn marker(line: &[u16], max: usize) -> Option<(usize, u16, usize)> {
    let spaces = run(line, SP);
    if spaces > max {
        return None;
    }
    let &c = line.get(spaces)?;
    if matches!(c, 0x2A | 0x2B | 0x2D) {
        return Some((spaces + 1, c, 0));
    }
    let digits = line[spaces..].iter().take_while(|&&u| (0x30..=0x39).contains(&u)).count();
    let &delimiter = line.get(spaces + digits)?;
    ((1..=9).contains(&digits) && matches!(delimiter, 0x2E | 0x29)).then_some((spaces + digits + 1, delimiter, digits))
}

/// A marker followed by a space or tab, or ending the line (`(?:[ \t]|$)`).
fn bullet(line: &[u16], max: usize) -> Option<(usize, u16, usize)> {
    marker(line, max).filter(|&(end, ..)| line.get(end).is_none_or(|&u| u == SP || u == TAB))
}

/// A root paragraph's list interruption: ` {0,3}(?:[*+-]|1[.)])[ \t]+[^ \t\n]`.
fn list_interrupts(line: &[u16]) -> bool {
    marker(line, 3).is_some_and(|(end, _, digits)| {
        let spaces = line[end..].iter().take_while(|&&u| u == SP || u == TAB).count();
        (digits == 0 || (digits == 1 && is(line.get(end - 2), b'1'))) && spaces > 0 && end + spaces < line.len()
    })
}

/// ` {0,3}(?:=+|-+) *` to the line's end.
fn underline(line: &[u16]) -> bool {
    let spaces = run(line, SP);
    let Some(&c) = line.get(spaces) else { return false };
    let marks = run(&line[spaces..], c);
    spaces <= 3 && matches!(c, 0x3D | 0x2D) && line[spaces + marks..].iter().all(|&u| u == SP)
}

/// A table's delimiter row without pipes: ` {0,3}:?-+:? *` to the line's end.
fn delimiter(line: &[u16]) -> bool {
    let spaces = run(line, SP);
    let mut at = spaces + usize::from(is(line.get(spaces), b':'));
    let dashes = run(&line[at..], u16::from(b'-'));
    at += dashes;
    at += usize::from(is(line.get(at), b':'));
    spaces <= 3 && dashes > 0 && line[at..].iter().all(|&u| u == SP)
}

/// Whether a table (`gfmTable`) starts at the line at `at`: it and a delimiter row.
fn table_at(s: &[u16], at: usize) -> bool {
    let header = line(s, at);
    let next = at + header.len() + 1;
    header.iter().any(|&u| u != SP) && next <= s.len() && delimiter(line(s, next))
}

/// `marked`'s table token (it needs a `:` in the delimiter row when there is no pipe).
fn table(s: &[u16]) -> bool {
    table_at(s, 0) && line(s, line(s, 0).len() + 1).contains(&u16::from(b':'))
}

/// Where a paragraph from `from` (a line's non-blank rest) ends: the rule's lookaheads
/// at each following line (`root`: the top level's, else a blockquote's).
fn paragraph(s: &[u16], from: usize, root: bool) -> usize {
    let mut end = newline(s, from);
    while end < s.len() {
        let next = end + 1;
        let l = line(s, next);
        let stops = l.is_empty()
            || hr(l, 3)
            || heading(l, 3)
            || quote(l, 3)
            || (blank(l) && next + l.len() < s.len())
            || if root { list_interrupts(l) || table_at(s, next) } else { bullet(l, 3).is_some() };
        if stops {
            break;
        }
        end = next + l.len();
    }
    end
}

/// `lheading`: lines up to a setext underline.
fn lheading(s: &[u16]) -> Option<usize> {
    // ` {0,3}(?:[*+-]|\d{1,9}[.)]) `
    let bull = |l: &[u16]| marker(l, 3).is_some_and(|(end, ..)| is(l.get(end), b' '));
    let first = line(s, 0);
    if bull(first) || quote(first, 3) || heading(first, 3) {
        return None;
    }
    let mut end = first.len();
    while end < s.len() {
        let next = end + 1;
        let l = line(s, next);
        if underline(l) {
            // JavaScript's `.` stops at line separators: no heading then (rare).
            return (!s[..end].iter().any(|&u| matches!(u, 0x2028 | 0x2029))).then_some(next + l.len());
        }
        if trims_empty(l) || bull(l) || quote(l, 3) || heading(l, 3) || hr(l, 3) {
            return None;
        }
        end = next + l.len();
    }
    None
}

/// The blockquote rule's match: `>` lines, a paragraph's lazy lines after them.
fn quote_capture(s: &[u16]) -> usize {
    let mut pos = 0;
    while pos < s.len() {
        let l = line(s, pos);
        let spaces = run(l, SP);
        if spaces > 3 || !is(l.get(spaces), b'>') {
            break;
        }
        let content = spaces + 1 + usize::from(is(l.get(spaces + 1), b' '));
        let end = if blank(&l[content..]) { pos + l.len() } else { paragraph(s, pos + content, false) };
        pos = (end + 1).min(s.len());
    }
    pos
}

fn join(lines: &[&[u16]]) -> Vec<u16> {
    let mut text = Vec::new();
    for (index, l) in lines.iter().enumerate() {
        if index > 0 {
            text.push(NL);
        }
        text.extend_from_slice(l);
    }
    text
}

/// A line without its quote marker (` {0,3}>[ \t]?`).
fn unquote(l: &[u16]) -> &[u16] {
    let spaces = run(l, SP);
    if spaces > 3 || !is(l.get(spaces), b'>') {
        return l;
    }
    let after = spaces + 1;
    &l[after + usize::from(matches!(l.get(after), Some(&SP | &TAB)))..]
}

/// `Tokenizer.blockquote`: its `raw`'s length.
fn blockquote(s: &[u16]) -> Result<Option<usize>> {
    let mut capture = quote_capture(s);
    if capture == 0 {
        return Ok(None);
    }
    while capture > 0 && s[capture - 1] == NL {
        capture -= 1;
    }
    let lines: Vec<&[u16]> = s[..capture].split(|&u| u == NL).collect();
    let count = lines.len();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < count {
        // Lines up to a continuation: lazy ones, then `>` ones.
        let mut end = at;
        let mut quoted = false;
        while end < count {
            if quote(lines[end], 3) {
                quoted = true;
            } else if quoted {
                break;
            }
            end += 1;
        }
        // Setext underlines indented out of reach, then the markers dropped.
        let mut text = Vec::new();
        for (index, &l) in lines[at..end].iter().enumerate() {
            if index > 0 {
                text.push(NL);
                if underline(l) {
                    text.extend_from_slice(&[SP; 4]);
                    text.extend_from_slice(&l[run(l, SP)..]);
                    continue;
                }
            }
            text.extend_from_slice(unquote(l));
        }
        at = end;
        lex(&text, &mut tokens, true)?;
        if at == count {
            break;
        }
        let Some(last) = tokens.last_mut() else { continue };
        match last.kind {
            Kind::Heading | Kind::Hr => break,
            Kind::Blockquote => {
                // The rest continues the nested quote, one marker less.
                let mut text = last.raw.clone();
                text.push(NL);
                let rest: Vec<&[u16]> = lines[at..].iter().map(|l| unquote(l)).collect();
                text.extend(join(&rest));
                let length = blockquote(&text)?.ok_or(Fallback)?;
                let leftover = &text[length..];
                let leftover = leftover.strip_prefix(&[NL]).unwrap_or(leftover);
                let left = if leftover.is_empty() { 0 } else { leftover.iter().filter(|&&u| u == NL).count() + 1 };
                at += if left > 0 { (count - at).saturating_sub(left) } else { count - at };
                last.raw = text[..length].to_vec();
                break;
            }
            Kind::List => {
                // The rest may continue the nested list.
                let old = last.raw.len();
                let mut text = last.raw.clone();
                text.push(NL);
                text.extend(join(&lines[at..]));
                let length = list(&text).ok_or(Fallback)?;
                if length > old {
                    at += text[old + 1..length].iter().filter(|&&u| u == NL).count() + 1;
                }
                last.raw = text[..length].to_vec();
            }
            _ => {}
        }
    }
    Ok(Some(lines[..at].iter().map(|l| l.len()).sum::<usize>() + at.saturating_sub(1)))
}

/// `Tokenizer.list`: its `raw`'s length.
fn list(s: &[u16]) -> Option<usize> {
    let (_, kind, digits) = bullet(line(s, 0), 3)?;
    let ordered = digits > 0;
    let mut pos = 0;
    let mut items = 0;
    while pos < s.len() {
        let first = line(&s[pos..], 0);
        // The same bullet (or delimiter), then a space, a tab or the line's end.
        let Some((marker_end, other, digits)) = bullet(first, 3) else { break };
        if other != kind || (digits > 0) != ordered || hr(first, 3) {
            break;
        }
        items += 1;
        let after = (pos + first.len() + 1).min(s.len());
        let mut item_line: &[u16] = &first[marker_end..];
        pos = after;
        let next = line(s, pos.min(s.len()));
        let mut blank_line = trims_empty(item_line);
        let indent = if blank_line {
            marker_end + 1
        } else {
            let at = item_line.iter().position(|&u| u != SP).unwrap_or(usize::MAX);
            marker_end + if at > 4 { 1 } else { at }
        };
        if blank_line && blank(next) {
            pos = (pos + next.len() + 1).min(s.len());
            continue;
        }
        // `cachedIndentRegex`: the item's block starts allow `min(3, indent - 1)` spaces.
        let max = indent.saturating_sub(1).min(3);
        while pos < s.len() {
            let l = line(s, pos);
            if heading(l, max) || quote(l, max) || bullet(l, max).is_some() || hr(l, max) {
                break;
            }
            let deep = l.iter().position(|&u| u != SP).is_some_and(|at| at >= indent);
            if !deep
                && !trims_empty(l)
                && (blank_line
                    || item_line.iter().position(|&u| u != SP).is_some_and(|at| at >= 4)
                    || heading(item_line, max)
                    || hr(item_line, max))
            {
                break;
            }
            blank_line = trims_empty(l);
            pos = (pos + l.len() + 1).min(s.len());
            item_line = &l[indent.min(l.len())..];
        }
    }
    if items == 0 {
        return None;
    }
    let mut end = pos;
    while end > 0 && js_space(s[end - 1]) {
        end -= 1;
    }
    Some(end)
}

#[cfg(test)]
mod tests {
    fn cuts(text: &str) -> Vec<usize> {
        let units: Vec<u16> = text.encode_utf16().collect();
        super::cuts(&units).unwrap_or_default()
    }

    #[test]
    fn streamdown_blocks() {
        // From Streamdown's own split (js/test/blocks-probe.ts).
        assert_eq!(cuts("a\n*  "), Vec::<usize>::new());
        assert_eq!(cuts("a\n* b"), vec![2]);
        assert_eq!(cuts("a\n1. b"), vec![2]);
        assert_eq!(cuts("*  \n`"), vec![4]);
        assert_eq!(cuts(" *\na"), vec![3]);
        assert_eq!(cuts("- - # \n- "), Vec::<usize>::new());
        assert_eq!(cuts("*a\n\n*\n"), vec![4]);
        assert_eq!(cuts("* *\n_"), Vec::<usize>::new());
        assert_eq!(cuts("- a\n# h"), vec![4]);
        assert_eq!(cuts("- # h\nb"), vec![6]);
        assert_eq!(cuts("-\n  a\nb"), Vec::<usize>::new());
        assert_eq!(cuts("*\n  # a"), Vec::<usize>::new());
    }
}
