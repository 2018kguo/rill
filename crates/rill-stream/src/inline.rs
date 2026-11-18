//! Inline content: text, code spans, emphasis and strong, escapes and hard breaks, written
//! as Streamdown's HTML. Emphasis follows CommonMark's delimiter algorithm as micromark
//! runs it.
//!
//! Text is copied straight into the output as it is scanned; delimiter runs are written
//! as their characters and remembered by position. Only a paragraph where emphasis
//! matched is rewritten, once, to put the tags in.

use crate::unicode::{PUNCTUATION_OR_SYMBOL, WHITESPACE, in_table};
use crate::{Fallback, Result};

const STRONG_OPEN: &str = r#"<span class="font-semibold" data-streamdown="strong">"#;
const STRONG_CLOSE: &str = "</span>";
const CODE_OPEN: &str =
    r#"<code class="rounded bg-muted px-1.5 py-0.5 font-mono text-sm" data-streamdown="inline-code">"#;

/// Bytes that end a run of plain text.
static SPECIAL: [bool; 256] = {
    let mut table = [false; 256];
    let mut index = 0;
    let specials = b"*_`\\\n&<>[]~";
    while index < specials.len() {
        table[specials[index] as usize] = true;
        index += 1;
    }
    table
};

#[derive(Default)]
pub struct Inline {
    runs: Vec<Run>,
    /// Indices into `runs` still able to match.
    delims: Vec<usize>,
}

/// A run of `*` or `_`.
struct Run {
    /// Where its characters start in the paragraph's HTML.
    at: usize,
    marker: u8,
    original: u32,
    count: u32,
    can_open: bool,
    can_close: bool,
    /// Tags opened after the remaining characters, outermost first (bit set: strong).
    opens: Tags,
    /// Tags closed before them, innermost first.
    closes: Tags,
}

#[derive(Clone, Copy, Default)]
struct Tags {
    bits: u64,
    len: u32,
}

impl Tags {
    fn push_front(&mut self, strong: bool) -> Result {
        if self.len == 64 {
            return Err(Fallback);
        }
        self.bits = (self.bits << 1) | u64::from(strong);
        self.len += 1;
        Ok(())
    }
    fn push_back(&mut self, strong: bool) -> Result {
        if self.len == 64 {
            return Err(Fallback);
        }
        self.bits |= u64::from(strong) << self.len;
        self.len += 1;
        Ok(())
    }
    /// The tags in order (bit 0 first: the outermost opener, the innermost closer).
    fn write(self, out: &mut String, open: bool) {
        for bit in 0..self.len {
            let strong = self.bits >> bit & 1 == 1;
            out.push_str(match (strong, open) {
                (true, true) => STRONG_OPEN,
                (true, false) => STRONG_CLOSE,
                (false, true) => "<em>",
                (false, false) => "</em>",
            });
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Punct,
    Other,
}

/// micromark's `classifyCharacter`.
fn class(c: Option<char>) -> Class {
    match c {
        None => Class::Space,
        Some(c) if c.is_ascii() => match c {
            ' ' | '\n' | '\t' | '\r' | '\x0b' | '\x0c' => Class::Space,
            c if c.is_ascii_punctuation() => Class::Punct,
            _ => Class::Other,
        },
        Some(c) if in_table(WHITESPACE, c) => Class::Space,
        Some(c) if in_table(PUNCTUATION_OR_SYMBOL, c) => Class::Punct,
        Some(_) => Class::Other,
    }
}

fn escape_into(out: &mut String, text: &str) {
    let bytes = text.as_bytes();
    let mut start = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        let entity = match byte {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            _ => continue,
        };
        out.push_str(&text[start..index]);
        out.push_str(entity);
        start = index + 1;
    }
    out.push_str(&text[start..]);
}

impl Inline {
    /// Renders `text` into `out`: a paragraph's lines from its first content character
    /// to its last, continuation lines with their indents.
    pub fn render(&mut self, text: &str, out: &mut String) -> Result {
        self.runs.clear();
        self.delims.clear();
        let start = out.len();
        self.scan(text, out, start)?;
        if self.emphasis()? {
            // Grow the paragraph's HTML in place, moving each piece right once, from the end.
            let tag_len = |tags: Tags, open: bool| -> usize {
                (0..tags.len)
                    .map(|bit| match (tags.bits >> bit & 1 == 1, open) {
                        (true, true) => STRONG_OPEN.len(),
                        (true, false) => STRONG_CLOSE.len(),
                        (false, true) => 4,
                        (false, false) => 5,
                    })
                    .sum()
            };
            let growth: usize = self
                .runs
                .iter()
                .map(|run| tag_len(run.closes, false) + tag_len(run.opens, true) + run.count as usize)
                .sum::<usize>()
                - self.runs.iter().map(|run| run.original as usize).sum::<usize>();
            let old_len = out.len();
            // SAFETY: every byte written below is ASCII markup or moved whole UTF-8 text.
            let buffer = unsafe { out.as_mut_vec() };
            buffer.resize(old_len + growth, 0);
            let mut end = old_len; // end of the not-yet-moved source
            let mut write = old_len + growth;
            let mut scratch = String::new();
            for run in self.runs.iter().rev() {
                let source_end = start + run.at + run.original as usize;
                let piece = end - source_end;
                buffer.copy_within(source_end..end, write - piece);
                write -= piece;
                scratch.clear();
                run.closes.write(&mut scratch, false);
                for _ in 0..run.count {
                    scratch.push(char::from(run.marker));
                }
                run.opens.write(&mut scratch, true);
                write -= scratch.len();
                buffer[write..write + scratch.len()].copy_from_slice(scratch.as_bytes());
                end = start + run.at;
            }
            debug_assert_eq!(write, end);
        }
        Ok(())
    }

    fn scan(&mut self, text: &str, out: &mut String, start: usize) -> Result {
        let bytes = text.as_bytes();
        // GFM autolink literals (`www.`, `scheme://`, emails) take the exact path.
        if memchr::memchr(b'@', bytes).is_some() || memchr::memmem::find(bytes, b"://").is_some() {
            return Err(Fallback);
        }
        for dot in memchr::memchr_iter(b'.', bytes) {
            if dot >= 3 && bytes[dot - 3..dot].eq_ignore_ascii_case(b"www") {
                return Err(Fallback);
            }
        }
        let mut at = 0;
        let mut plain = 0;
        // mdast-util-to-hast trims the start of whatever follows a hard break.
        let mut after_break = false;
        loop {
            while at < bytes.len() && !SPECIAL[usize::from(bytes[at])] {
                at += 1;
            }
            if at > plain {
                out.push_str(&text[plain..at]);
                after_break = false;
            }
            let Some(&byte) = bytes.get(at) else {
                return Ok(());
            };
            let broke = after_break;
            after_break = false;
            match byte {
                b'\n' => {
                    // Spaces before a line ending: two or more make a hard break.
                    let trimmed = out[start..].trim_end_matches(' ').len() + start;
                    let spaces = out.len() - trimmed;
                    out.truncate(trimmed);
                    out.push_str(if spaces >= 2 { "<br>\n" } else { "\n" });
                    after_break = spaces >= 2;
                    at = skip_spaces(bytes, at + 1);
                }
                b'\\' => match bytes.get(at + 1) {
                    Some(b'\n') => {
                        out.push_str("<br>\n");
                        after_break = true;
                        at = skip_spaces(bytes, at + 2);
                    }
                    Some(&next) if next.is_ascii_punctuation() => {
                        escape_into(out, &text[at + 1..at + 2]);
                        at += 2;
                    }
                    _ => {
                        out.push('\\');
                        at += 1;
                    }
                },
                b'`' => {
                    let run = count(bytes, at, b'`');
                    match closing_backticks(bytes, at + run, run) {
                        Some(end) => {
                            out.push_str(CODE_OPEN);
                            code_content(out, &text[at + run..end], broke);
                            out.push_str("</code>");
                            at = end + run;
                        }
                        None => {
                            out.push_str(&text[at..at + run]);
                            at += run;
                        }
                    }
                }
                b'>' => {
                    out.push_str("&gt;");
                    at += 1;
                }
                // A `&` that cannot start a character reference, a `<` that cannot start
                // HTML or an autolink.
                b'&' if !bytes.get(at + 1).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'#') => {
                    out.push_str("&amp;");
                    at += 1;
                }
                b'<' if !bytes
                    .get(at + 1)
                    .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?')) =>
                {
                    out.push_str("&lt;");
                    at += 1;
                }
                b'*' | b'_' => {
                    let length = count(bytes, at, byte);
                    let before = class(text[..at].chars().next_back());
                    let after = class(text[at + length..].chars().next());
                    let left = after != Class::Space && (after != Class::Punct || before != Class::Other);
                    let right = before != Class::Space && (before != Class::Punct || after != Class::Other);
                    let (can_open, can_close) = if byte == b'*' {
                        (left, right)
                    } else {
                        (left && (!right || before == Class::Punct), right && (!left || after == Class::Punct))
                    };
                    let length32 = u32::try_from(length).map_err(|_| Fallback)?;
                    if can_open || can_close {
                        self.delims.push(self.runs.len());
                    }
                    self.runs.push(Run {
                        at: out.len() - start,
                        marker: byte,
                        original: length32,
                        count: length32,
                        can_open,
                        can_close,
                        opens: Tags::default(),
                        closes: Tags::default(),
                    });
                    out.push_str(&text[at..at + length]);
                    at += length;
                }
                // Links, images, raw HTML, references, strikethrough, autolinks: the
                // exact path.
                _ => return Err(Fallback),
            }
            plain = at;
        }
    }

    /// CommonMark's "process emphasis" over every delimiter run; whether any matched.
    fn emphasis(&mut self) -> Result<bool> {
        // ponytail: quadratic opener search; add openers_bottom if adversarial input shows up.
        let mut matched = false;
        let mut closer_at = 0;
        while closer_at < self.delims.len() {
            let closer = self.delims[closer_at];
            let c = &self.runs[closer];
            if !(c.can_close && c.count > 0) {
                closer_at += 1;
                continue;
            }
            let (marker, close_count, closer_opens) = (c.marker, c.count, c.can_open);
            let found = (0..closer_at).rev().find(|&opener_at| {
                let o = &self.runs[self.delims[opener_at]];
                o.marker == marker
                    && o.can_open
                    && o.count > 0
                    // micromark applies the rule of three to what is left of each run.
                    && !((o.can_close || closer_opens)
                        && (o.count + close_count) % 3 == 0
                        && !(o.count % 3 == 0 && close_count % 3 == 0))
            });
            let Some(opener_at) = found else {
                closer_at += 1;
                continue;
            };
            matched = true;
            let opener = self.delims[opener_at];
            let open_count = self.runs[opener].count;
            let used = if open_count >= 2 && close_count >= 2 { 2 } else { 1 };
            self.runs[opener].count -= used;
            self.runs[opener].opens.push_front(used == 2)?;
            self.runs[closer].count -= used;
            self.runs[closer].closes.push_back(used == 2)?;
            // Runs between them can no longer match.
            self.delims.drain(opener_at + 1..closer_at);
            closer_at = opener_at + 1;
            if open_count == used {
                self.delims.remove(opener_at);
                closer_at -= 1;
            }
            if close_count == used {
                self.delims.remove(closer_at);
            }
        }
        Ok(matched)
    }
}

fn count(bytes: &[u8], at: usize, byte: u8) -> usize {
    bytes[at..].iter().take_while(|&&b| b == byte).count()
}

fn skip_spaces(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at) == Some(&b' ') {
        at += 1;
    }
    at
}

/// The start of the backtick run of exactly `run` ticks closing a code span opened at
/// `from`, or `None`.
fn closing_backticks(bytes: &[u8], from: usize, run: usize) -> Option<usize> {
    let mut at = from;
    while let Some(offset) = bytes[at..].iter().position(|&b| b == b'`') {
        at += offset;
        let length = count(bytes, at, b'`');
        if length == run {
            return Some(at);
        }
        at += length;
    }
    None
}

/// A code span's content, escaped: line endings as spaces, one space stripped from each
/// side when both have one and it is not all spaces.
fn code_content(out: &mut String, raw: &str, after_break: bool) {
    let bytes = raw.as_bytes();
    let space = |b: u8| b == b' ' || b == b'\n';
    let strip = bytes.len() >= 2
        && space(bytes[0])
        && space(bytes[bytes.len() - 1])
        && bytes.iter().any(|&b| !space(b));
    let raw = if strip { &raw[1..raw.len() - 1] } else { raw };
    let start = out.len();
    escape_into(out, raw);
    if raw.contains('\n') {
        let replaced = out[start..].replace('\n', " ");
        out.truncate(start);
        out.push_str(&replaced);
    }
    if after_break {
        let kept = out[start..].trim_start_matches(' ').to_owned();
        out.truncate(start);
        out.push_str(&kept);
    }
}
