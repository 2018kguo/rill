//! Where Streamdown cuts a text into blocks: `marked`'s (v18, GFM) top-level tokens, a
//! block per token, blank lines joining the block before. Streamdown parses each block
//! on its own, so the cuts decide what continues into what. Only the tokens' extents are
//! computed, after `marked`'s tokenizer (`Lexer.blockTokens`, `Tokenizer.blockquote` and
//! `list`, the block rules); what the renderer does not cover yet is [`Fallback`]:
//! HTML, definitions, indented code, math.

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
    Code,
    Table,
}

struct Token {
    kind: Kind,
    /// Where it starts in the text lexed (meaningful at the top level only).
    start: usize,
    /// A blockquote's or list's `raw` (later lines may be lexed with it again).
    raw: Vec<u16>,
}

/// Streamdown's footnote test: `[^` then 1 to 200 of `[\w-]` then `]`.
pub(crate) fn footnotes(text: &[u16]) -> bool {
    let mut at = 0;
    while let Some(offset) = text[at..].windows(2).position(|pair| pair == [0x5B, 0x5E]) {
        let start = at + offset + 2;
        let label = text[start..]
            .iter()
            .take(201)
            .take_while(|&&unit| crate::inline::note_unit(unit))
            .count();
        if (1..=200).contains(&label) && text.get(start + label) == Some(&0x5D) {
            return true;
        }
        at = start;
    }
    false
}

/// The UTF-16 offsets where Streamdown's blocks of `text` start, the first excepted;
/// `None` when marked registers a link definition (Streamdown keeps the text whole).
pub(crate) fn cuts(text: &[u16]) -> Result<Option<Vec<usize>>> {
    let mut tokens = Vec::new();
    if lex(text, &mut tokens, false)? {
        return Ok(None);
    }
    // A block with an odd count of `$$` takes the next (math): blocks other than code.
    let mut ends = tokens
        .iter()
        .skip(1)
        .map(|token| token.start)
        .chain([text.len()]);
    for token in &tokens {
        let end = ends.next().expect("one end a token");
        if token.kind != Kind::Code
            && text[token.start..end]
                .windows(2)
                .any(|pair| pair == [0x24, 0x24])
        {
            return Err(Fallback);
        }
    }
    Ok(Some(
        tokens
            .iter()
            .filter(|token| token.kind != Kind::Space && token.start > 0)
            .map(|token| token.start)
            .collect(),
    ))
}

/// A range of units.
type Range = (usize, usize);

/// A link definition at the start of `s` (marked's `def`, micromark's definition): its
/// label and destination (ranges of `s`) when it is one this is sure of (one line:
/// `[label]: destination`, a title on the line), `None` when it is not one; the exact
/// path for what may be one otherwise.
pub(crate) fn definition(s: &[u16]) -> Result<Option<(Range, Range)>> {
    let first = line(s, 0);
    let spaces = run(first, SP);
    if spaces > 3 || !is(first.get(spaces), b'[') {
        return Ok(None);
    }
    // On the text's last line, what is not a definition yet is a paragraph so far.
    let unsure = || {
        if first.len() >= s.len() {
            Ok(None)
        } else {
            Err(Fallback)
        }
    };
    let mut at = spaces + 1;
    while at < first.len() && !matches!(first[at], 0x5B | 0x5D) {
        at += if first[at] == 0x5C { 2 } else { 1 };
    }
    if !is(first.get(at), b']') || !is(first.get(at + 1), b':') {
        // A label going on past the line may close later.
        return if at >= first.len() {
            unsure()
        } else {
            Ok(None)
        };
    }
    let label = &first[spaces + 1..at];
    if label.iter().all(|&unit| js_space(unit)) {
        return Ok(None);
    }
    let rest = &first[at + 2..];
    let lead = run(rest, SP);
    let destination = rest[lead..]
        .iter()
        .take_while(|&&unit| !js_space(unit))
        .count();
    if destination == 0 {
        return unsure(); // ponytail: the destination on the next line
    }
    // ponytail: `<...>`, and parentheses (micromark balances them, marked does not).
    if rest[lead..lead + destination]
        .iter()
        .any(|&unit| matches!(unit, 0x3C | 0x28 | 0x29))
    {
        return Err(Fallback);
    }
    let after = &rest[lead + destination..];
    let gap = run(after, SP);
    let title = &after[gap..];
    let simple_title = title.is_empty()
        || (gap > 0
            && title.len() >= 2
            && matches!(
                (title[0], title[title.len() - 1]),
                (0x22, 0x22) | (0x27, 0x27) | (0x28, 0x29)
            )
            && !title[1..title.len() - 1]
                .iter()
                .any(|&unit| matches!(unit, 0x22 | 0x27 | 0x28 | 0x29 | 0x5C)));
    if !simple_title
        || (title.is_empty()
            && first.len() < s.len()
            && matches!(
                line(s, first.len() + 1)
                    .iter()
                    .find(|&&unit| unit != SP)
                    .copied(),
                Some(0x22 | 0x27 | 0x28)
            ))
    {
        return unsure(); // ponytail: titles past the line
    }
    let destination_at = at + 2 + lead;
    Ok(Some((
        (spaces + 1, at),
        (destination_at, destination_at + destination),
    )))
}

/// Fallback for what may make a paragraph, heading or rule's text a token this does
/// not lex: HTML at a line's start (after indents and quote markers). 16 units at a time, looking closer only at those characters.
fn precheck(text: &[u16]) -> Result {
    const SET: ByteSet = ByteSet::new(b"<");
    let fails = |at: usize| {
        text[..at]
            .iter()
            .rev()
            .find(|&&u| u != SP && u != u16::from(b'>'))
            .is_none_or(|&u| u == NL)
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

/// Lexes `src` into `tokens`: whether it holds a link definition (marked's `links`).
fn lex(src: &[u16], tokens: &mut Vec<Token>, mut clipped: bool) -> Result<bool> {
    let mut pos = 0;
    while pos < src.len() {
        let s = &src[pos..];
        let spaces = space(s);
        if spaces > 0 {
            match tokens.last_mut() {
                Some(last) if spaces == 1 => last.raw.push(NL),
                _ => tokens.push(Token {
                    kind: Kind::Space,
                    start: pos,
                    raw: Vec::new(),
                }),
            }
            pos += spaces;
            continue;
        }
        let first = line(s, 0);
        let (kind, length) = if run(first, SP) >= 4 {
            (Kind::Code, indented(s))
        } else if let Some(length) = fences(s)? {
            (Kind::Code, length)
        } else if heading(first, 3) {
            (Kind::Heading, first.len())
        } else if hr(first, 3) {
            (Kind::Hr, first.len())
        } else if let Some(length) = blockquote(s)? {
            (Kind::Blockquote, length)
        } else if let Some(length) = list(s)? {
            (Kind::List, length)
        } else if definition(s)?.is_some() {
            return Ok(true);
        } else if let Some(length) = table(s)? {
            (Kind::Table, length)
        } else if let Some(length) = lheading(s) {
            (Kind::Heading, length)
        } else {
            let length = paragraph(s, 0, true);
            precheck(&s[..length])?;
            let merge = clipped
                && tokens
                    .last()
                    .is_some_and(|last| last.kind == Kind::Paragraph);
            clipped = false;
            pos += length;
            if !merge {
                tokens.push(Token {
                    kind: Kind::Paragraph,
                    start: pos - length,
                    raw: Vec::new(),
                });
            }
            continue;
        };
        if kind == Kind::Heading || kind == Kind::Hr {
            precheck(&s[..length])?;
        }
        // ponytail: a definition inside a quote or a list (marked registers those too).
        if matches!(kind, Kind::Blockquote | Kind::List) && may_define(&s[..length]) {
            return Err(Fallback);
        }
        let raw = if matches!(kind, Kind::Blockquote | Kind::List) {
            s[..length].to_vec()
        } else {
            Vec::new()
        };
        tokens.push(Token {
            kind,
            start: pos,
            raw,
        });
        pos += length;
    }
    Ok(false)
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
    rest.iter().all(|&u| u == c || u == SP || u == TAB)
        && rest.iter().filter(|&&u| u == c).count() >= 3
}

/// ` {0,max}#{1,6}(?:\s|$)`.
fn heading(line: &[u16], max: usize) -> bool {
    let spaces = run(line, SP);
    let hashes = run(&line[spaces..], u16::from(b'#'));
    spaces <= max
        && (1..=6).contains(&hashes)
        && line.get(spaces + hashes).is_none_or(|&u| js_space(u))
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
    let digits = line[spaces..]
        .iter()
        .take_while(|&&u| (0x30..=0x39).contains(&u))
        .count();
    let &delimiter = line.get(spaces + digits)?;
    ((1..=9).contains(&digits) && matches!(delimiter, 0x2E | 0x29)).then_some((
        spaces + digits + 1,
        delimiter,
        digits,
    ))
}

/// A marker followed by a space or tab, or ending the line (`(?:[ \t]|$)`).
fn bullet(line: &[u16], max: usize) -> Option<(usize, u16, usize)> {
    marker(line, max).filter(|&(end, ..)| line.get(end).is_none_or(|&u| u == SP || u == TAB))
}

/// A root paragraph's list interruption: ` {0,3}(?:[*+-]|1[.)])[ \t]+[^ \t\n]`.
fn list_interrupts(line: &[u16]) -> bool {
    marker(line, 3).is_some_and(|(end, _, digits)| {
        let spaces = line[end..]
            .iter()
            .take_while(|&&u| u == SP || u == TAB)
            .count();
        (digits == 0 || (digits == 1 && is(line.get(end - 2), b'1')))
            && spaces > 0
            && end + spaces < line.len()
    })
}

/// ` {0,3}(?:=+|-+) *` to the line's end.
fn underline(line: &[u16]) -> bool {
    let spaces = run(line, SP);
    let Some(&c) = line.get(spaces) else {
        return false;
    };
    let marks = run(&line[spaces..], c);
    spaces <= 3 && matches!(c, 0x3D | 0x2D) && line[spaces + marks..].iter().all(|&u| u == SP)
}

/// `fences`: the opening line, the content and the closing line (or the rest of the
/// text). A closing fence micromark does not take (the other character after the
/// opening's run) is [`Fallback`].
fn fences(s: &[u16]) -> Result<Option<usize>> {
    let first = line(s, 0);
    let spaces = run(first, SP);
    let Some((fence, length)) = (spaces <= 3)
        .then(|| crate::block::fence_open(&first[spaces..]))
        .flatten()
    else {
        return Ok(None);
    };
    let mut end = first.len();
    while end < s.len() {
        let next = end + 1;
        let l = line(s, next);
        let indent = run(l, SP);
        let rest = &l[indent..];
        if indent <= 3 && run(rest, fence) >= length {
            let marks = rest.iter().take_while(|&&u| u == 0x60 || u == 0x7E).count();
            if rest[marks..].iter().all(|&u| u == SP) {
                if rest[..marks].iter().any(|&u| u != fence) {
                    return Err(Fallback);
                }
                return Ok(Some(next + l.len()));
            }
        }
        end = next + l.len();
    }
    Ok(Some(s.len()))
}

/// `code`: indented lines (four spaces and more), blank lines between them.
fn indented(s: &[u16]) -> usize {
    let mut end = 0;
    let mut at = 0;
    while at < s.len() {
        let l = line(s, at);
        if blank(l) {
            at += l.len() + 1;
            continue;
        }
        if run(l, SP) < 4 {
            break;
        }
        end = at + l.len();
        at = end + 1;
    }
    end
}

/// ` {0,max}(?:```|~~~)`.
fn fence_begins(line: &[u16], max: usize) -> bool {
    let spaces = run(line, SP);
    spaces <= max
        && matches!(line.get(spaces), Some(&(0x60 | 0x7E)))
        && run(&line[spaces..], line[spaces]) >= 3
}

/// `gfmTable`'s delimiter row (` {0,3}((?:\| *)?:?-+:? *(?:\| *:?-+:? *)*(?:\| *)?)`
/// to the line's end): its cell count, and whether it has a pipe or a colon.
fn delimiter(line: &[u16]) -> Option<(usize, bool)> {
    let is = |at: usize, byte: u8| line.get(at) == Some(&u16::from(byte));
    let spaces = run(line, SP);
    if spaces > 3 {
        return None;
    }
    let mut marked = false;
    // A cell (`:?-+:? *`) at `at`: where it ends.
    let mut cell = |at: usize| {
        let left = is(at, b':');
        let dashes = run(&line[at + usize::from(left)..], u16::from(b'-'));
        if dashes == 0 {
            return None;
        }
        let mut end = at + usize::from(left) + dashes;
        let right = is(end, b':');
        end += usize::from(right);
        marked |= left || right;
        Some(end + run(&line[end..], SP))
    };
    let mut at = spaces;
    let pipe = is(at, b'|');
    if pipe {
        at += 1 + run(&line[at + 1..], SP);
    }
    at = cell(at)?;
    let mut cells = 1;
    let mut pipes = pipe;
    while at < line.len() {
        if !is(at, b'|') {
            return None;
        }
        pipes = true;
        at += 1 + run(&line[at + 1..], SP);
        if at == line.len() {
            break;
        }
        at = cell(at)?;
        cells += 1;
    }
    Some((cells, marked || pipes))
}

/// Whether a table (`gfmTable`, without the tokenizer's checks) starts at the line at
/// `at`: a line with a non-space, then a delimiter row.
fn table_at(s: &[u16], at: usize) -> bool {
    let header = line(s, at);
    let next = at + header.len() + 1;
    header.iter().any(|&u| u != SP) && next <= s.len() && delimiter(line(s, next)).is_some()
}

/// `marked`'s header cell count (`splitCells`, no escapes): split at pipes, a blank
/// first and last cell dropped.
fn header_cells(line: &[u16]) -> usize {
    let cells: Vec<&[u16]> = line.split(|&u| u == u16::from(b'|')).collect();
    let blank = |cell: &&[u16]| cell.iter().all(|&u| js_space(u));
    let mut count = cells.len();
    let mut from = 0;
    if cells.first().is_some_and(blank) {
        count -= 1;
        from = 1;
    }
    if count > 0 && cells.last().is_some_and(blank) && cells.len() > from {
        count -= 1;
    }
    count
}

/// `Tokenizer.table`: the header, the delimiter row (with a pipe or a colon, as many
/// cells), then rows up to a blank line or another block's start.
fn table(s: &[u16]) -> Result<Option<usize>> {
    if !table_at(s, 0) {
        return Ok(None);
    }
    let header = line(s, 0);
    let rule_at = header.len() + 1;
    let rule = line(s, rule_at);
    let (count, marked) = delimiter(rule).expect("a delimiter row");
    if !marked || header_cells(&header[run(header, SP)..]) != count {
        return Ok(None);
    }
    if header.contains(&u16::from(b'\\')) {
        return Err(Fallback);
    }
    let mut end = rule_at + rule.len();
    while end < s.len() {
        let next = end + 1;
        let l = line(s, next);
        let spaces = run(l, SP);
        let rest = &l[spaces..];
        if spaces <= 3 && is(rest.first(), b'<') {
            return Err(Fallback);
        }
        let list = marker(l, 3).is_some_and(|(at, _, digits)| {
            (digits == 0 || (digits == 1 && is(l.get(at - 2), b'1')))
                && matches!(l.get(at), Some(&(SP | TAB)))
        });
        let stops = (blank(l) && next + l.len() < s.len())
            || hr(l, 3)
            || heading(l, 3)
            || quote(l, 3)
            || (spaces >= 4 && spaces < l.len())
            || (spaces <= 3 && crate::block::fence_open(rest).is_some())
            || list;
        if stops {
            break;
        }
        end = next + l.len();
    }
    Ok(Some(end))
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
            || (run(l, SP) <= 3 && crate::block::fence_open(&l[run(l, SP)..]).is_some())
            || (blank(l) && next + l.len() < s.len())
            || if root {
                list_interrupts(l) || table_at(s, next)
            } else {
                bullet(l, 3).is_some()
            };
        if stops {
            break;
        }
        end = next + l.len();
    }
    end
}

/// A line (followed by more) that `lheading` does not run across.
pub(crate) fn setext_stop(l: &[u16]) -> bool {
    let bull = marker(l, 3).is_some_and(|(end, ..)| is(l.get(end), b' '));
    let pipes =
        l.contains(&u16::from(b'|')) && l.iter().all(|&u| matches!(u, 0x20 | 0x2D | 0x3A | 0x7C));
    trims_empty(l)
        || bull
        || quote(l, 3)
        || heading(l, 3)
        || hr(l, 3)
        || fence_begins(l, 3)
        || pipes
}

/// `lheading`: lines up to a setext underline.
fn lheading(s: &[u16]) -> Option<usize> {
    // ` {0,3}(?:[*+-]|\d{1,9}[.)]) `
    let bull = |l: &[u16]| marker(l, 3).is_some_and(|(end, ..)| is(l.get(end), b' '));
    // ` {0,3}\|?(?:[:\- ]*\|)+[\:\- ]*\n`: a row of pipes, colons, dashes, spaces.
    let pipes = |l: &[u16], at: usize| {
        at + l.len() < s.len()
            && l.contains(&u16::from(b'|'))
            && l.iter().all(|&u| matches!(u, 0x20 | 0x2D | 0x3A | 0x7C))
    };
    let first = line(s, 0);
    if bull(first)
        || quote(first, 3)
        || heading(first, 3)
        || fence_begins(first, 3)
        || pipes(first, 0)
    {
        return None;
    }
    let mut end = first.len();
    while end < s.len() {
        let next = end + 1;
        let l = line(s, next);
        if underline(l) {
            // JavaScript's `.` stops at line separators: no heading then (rare).
            return (!s[..end].iter().any(|&u| matches!(u, 0x2028 | 0x2029)))
                .then_some(next + l.len());
        }
        if trims_empty(l)
            || bull(l)
            || quote(l, 3)
            || heading(l, 3)
            || hr(l, 3)
            || fence_begins(l, 3)
            || pipes(l, next)
        {
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
        let end = if blank(&l[content..]) {
            pos + l.len()
        } else {
            paragraph(s, pos + content, false)
        };
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
        if lex(&text, &mut tokens, true)? {
            return Err(Fallback);
        }
        if at == count {
            break;
        }
        let Some(last) = tokens.last_mut() else {
            continue;
        };
        match last.kind {
            Kind::Heading | Kind::Hr | Kind::Code => break,
            Kind::Blockquote => {
                // The rest continues the nested quote, one marker less.
                let mut text = last.raw.clone();
                text.push(NL);
                let rest: Vec<&[u16]> = lines[at..].iter().map(|l| unquote(l)).collect();
                text.extend(join(&rest));
                let length = blockquote(&text)?.ok_or(Fallback)?;
                let leftover = &text[length..];
                let leftover = leftover.strip_prefix(&[NL]).unwrap_or(leftover);
                let left = if leftover.is_empty() {
                    0
                } else {
                    leftover.iter().filter(|&&u| u == NL).count() + 1
                };
                at += if left > 0 {
                    (count - at).saturating_sub(left)
                } else {
                    count - at
                };
                last.raw = text[..length].to_vec();
                break;
            }
            Kind::List => {
                // The rest may continue the nested list.
                let old = last.raw.len();
                let mut text = last.raw.clone();
                text.push(NL);
                text.extend(join(&lines[at..]));
                let length = list(&text)?.ok_or(Fallback)?;
                if length > old {
                    at += text[old + 1..length].iter().filter(|&&u| u == NL).count() + 1;
                }
                last.raw = text[..length].to_vec();
            }
            _ => {}
        }
    }
    Ok(Some(
        lines[..at].iter().map(|l| l.len()).sum::<usize>() + at.saturating_sub(1),
    ))
}

/// Whether a quote's or list's `raw` may hold a link definition: a line that starts,
/// past its quote and list markers, with `[` and holds `]:`, outside fenced code. A fence
/// is taken as open only while its lines keep at least its markers' width (anything less
/// may have ended it: a definition there is assumed).
fn may_define(raw: &[u16]) -> bool {
    let mut fence: Option<(u16, usize, usize)> = None;
    for line in raw.split(|&u| u == NL) {
        let mut at = 0;
        loop {
            let rest = &line[at..];
            let spaces = run(rest, SP);
            if is(rest.get(spaces), b'>') {
                at += spaces + 1;
            } else if let Some((end, _, _)) = bullet(rest, usize::MAX)
                && (end == rest.len() || rest[end] == SP)
            {
                at += end;
            } else {
                at += spaces;
                break;
            }
        }
        let content = &line[at..];
        let ticks = content
            .first()
            .filter(|&&u| u == 0x60 || u == 0x7E)
            .map_or(0, |&u| run(content, u));
        match fence {
            Some((unit, count, width)) if at >= width => {
                if ticks >= count && content[0] == unit && content[ticks..].iter().all(|&u| u == SP)
                {
                    fence = None;
                }
                continue;
            }
            _ => fence = None,
        }
        if ticks >= 3 && !(content[0] == 0x60 && content[ticks..].contains(&0x60)) {
            fence = Some((content[0], ticks, at));
        } else if is(content.first(), b'[') && content.windows(2).any(|pair| pair == [0x5D, 0x3A]) {
            return true;
        }
    }
    false
}

/// `Tokenizer.list`: its `raw`'s length.
fn list(s: &[u16]) -> Result<Option<usize>> {
    let Some((_, kind, digits)) = bullet(line(s, 0), 3) else {
        return Ok(None);
    };
    let ordered = digits > 0;
    let mut pos = 0;
    let mut items = 0;
    while pos < s.len() {
        let first = line(&s[pos..], 0);
        // The same bullet (or delimiter), then a space, a tab or the line's end.
        let Some((marker_end, other, digits)) = bullet(first, 3) else {
            break;
        };
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
            let at = item_line
                .iter()
                .position(|&u| u != SP)
                .unwrap_or(usize::MAX);
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
            if run(l, SP) <= max && is(l.get(run(l, SP)), b'<') {
                return Err(Fallback); // an HTML block may start
            }
            if fence_begins(l, max)
                || heading(l, max)
                || quote(l, max)
                || bullet(l, max).is_some()
                || hr(l, max)
            {
                break;
            }
            let deep = l
                .iter()
                .position(|&u| u != SP)
                .is_some_and(|at| at >= indent);
            if !deep
                && !trims_empty(l)
                && (blank_line
                    || item_line
                        .iter()
                        .position(|&u| u != SP)
                        .is_some_and(|at| at >= 4)
                    || fence_begins(item_line, max)
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
        return Ok(None);
    }
    let mut end = pos;
    while end > 0 && js_space(s[end - 1]) {
        end -= 1;
    }
    Ok(Some(end))
}

#[cfg(test)]
mod tests {
    fn cuts(text: &str) -> Vec<usize> {
        let units: Vec<u16> = text.encode_utf16().collect();
        super::cuts(&units).ok().flatten().unwrap_or_default()
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
