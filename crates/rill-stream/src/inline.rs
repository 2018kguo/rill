//! Inline content: text, code spans, emphasis and strong, escapes and hard breaks, written
//! as Streamdown's HTML. Emphasis follows CommonMark's delimiter algorithm as micromark
//! runs it.
//!
//! Text is copied straight into the output as it is scanned; delimiter runs are written
//! as their characters and remembered by position. Only a paragraph where emphasis
//! matched is rewritten, once, to put the tags in.

use crate::unicode::{PUNCTUATION_OR_SYMBOL, WHITESPACE, in_table};
use crate::simd::ByteSet;
use crate::{Fallback, Result};

const STRONG_OPEN: &str = r#"<span class="font-semibold" data-streamdown="strong">"#;
const STRONG_CLOSE: &str = "</span>";
const CODE_OPEN: &str =
    r#"<code class="rounded bg-muted px-1.5 py-0.5 font-mono text-sm" data-streamdown="inline-code">"#;

/// Bytes that end a run of plain text.
const SPECIAL: ByteSet = ByteSet::new(b"*_`\\\n&<>[]~");

/// The first byte at or after `at` that is in `SPECIAL`, or the end.
#[inline(always)]
fn next_special(bytes: &[u8], mut at: usize) -> usize {
    while let Some(chunk) = bytes.get(at..at + 16) {
        let mask = SPECIAL.mask16(chunk.try_into().expect("16 bytes"));
        if mask != 0 {
            return at + mask.trailing_zeros() as usize;
        }
        at += 16;
    }
    while at < bytes.len() && !SPECIAL.contains(bytes[at]) {
        at += 1;
    }
    at
}

#[derive(Default)]
pub struct Inline {
    runs: Vec<Run>,
    /// Indices into `runs` still able to match.
    delims: Vec<usize>,
    /// What the scan found, in source order; the text between them is copied as it is.
    events: Vec<Event>,
}

/// Something at a source position other than plain text.
#[derive(Clone, Copy)]
struct Event {
    /// Where the plain text before it ends.
    at: u32,
    /// Where plain text resumes after it.
    resume: u32,
    kind: Kind,
}

#[derive(Clone, Copy)]
enum Kind {
    /// A line ending (with the spaces before it and the indent after it).
    Line { hard: bool },
    /// A backslash before a line ending.
    Break,
    /// A fixed replacement (an escaped character, `&gt;`, ...).
    Literal(&'static str),
    /// A code span's content `[start, end)`; `trim` after a hard break.
    Code { start: u32, end: u32, trim: bool },
    /// A delimiter run, `runs[index]`.
    Run { index: u32 },
}

/// A run of `*` or `_`.
struct Run {
    marker: u8,
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
    fn write(self, out: &mut Vec<u8>, open: bool) {
        for bit in 0..self.len {
            let strong = self.bits >> bit & 1 == 1;
            out.extend_from_slice(match (strong, open) {
                (true, true) => STRONG_OPEN,
                (true, false) => STRONG_CLOSE,
                (false, true) => "<em>",
                (false, false) => "</em>",
            }.as_bytes());
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

/// ASCII punctuation that a backslash escapes, as its HTML.
fn escaped(byte: u8) -> &'static str {
    const PUNCT: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
    match byte {
        b'&' => "&amp;",
        b'<' => "&lt;",
        b'>' => "&gt;",
        _ => {
            let at = PUNCT.find(char::from(byte)).expect("ASCII punctuation");
            &PUNCT[at..=at]
        }
    }
}

impl Inline {
    /// Renders `text` into `out`: a paragraph's lines from its first content character
    /// to its last, continuation lines with their indents.
    pub fn render(&mut self, text: &str, out: &mut String) -> Result {
        self.runs.clear();
        self.delims.clear();
        self.events.clear();
        self.scan(text)?;
        self.emphasis()?;
        // SAFETY: only whole UTF-8 text between ASCII positions and ASCII markup go in.
        let buffer = unsafe { out.as_mut_vec() };
        let bytes = text.as_bytes();
        let mut cursor = 0;
        for event in &self.events {
            buffer.extend_from_slice(&bytes[cursor..event.at as usize]);
            match event.kind {
                Kind::Line { hard } => buffer.extend_from_slice(if hard { b"<br>\n" } else { b"\n" }),
                Kind::Break => buffer.extend_from_slice(b"<br>\n"),
                Kind::Literal(html) => buffer.extend_from_slice(html.as_bytes()),
                Kind::Code { start, end, trim } => {
                    buffer.extend_from_slice(CODE_OPEN.as_bytes());
                    code_content(buffer, &bytes[start as usize..end as usize], trim);
                    buffer.extend_from_slice(b"</code>");
                }
                Kind::Run { index } => {
                    let run = &self.runs[index as usize];
                    run.closes.write(buffer, false);
                    for _ in 0..run.count {
                        buffer.push(run.marker);
                    }
                    run.opens.write(buffer, true);
                }
            }
            cursor = event.resume as usize;
        }
        buffer.extend_from_slice(&bytes[cursor..]);
        Ok(())
    }

    fn event(&mut self, at: usize, resume: usize, kind: Kind) {
        self.events.push(Event { at: at as u32, resume: resume as u32, kind });
    }

    fn scan(&mut self, text: &str) -> Result {
        let bytes = text.as_bytes();
        if u32::try_from(bytes.len()).is_err() {
            return Err(Fallback);
        }
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
        // Where plain text began: the spaces before a line ending are trimmed back to it.
        let mut plain = 0;
        // mdast-util-to-hast trims the start of whatever follows a hard break: the source
        // position right after the last break.
        let mut broke_at = usize::MAX;
        loop {
            at = next_special(bytes, at);
            let Some(&byte) = bytes.get(at) else {
                return Ok(());
            };
            match byte {
                b'\n' => {
                    let mut spaces = at;
                    while spaces > plain && bytes[spaces - 1] == b' ' {
                        spaces -= 1;
                    }
                    let hard = at - spaces >= 2;
                    let resume = skip_spaces(bytes, at + 1);
                    self.event(spaces, resume, Kind::Line { hard });
                    if hard {
                        broke_at = resume;
                    }
                    at = resume;
                }
                b'\\' => match bytes.get(at + 1) {
                    Some(b'\n') => {
                        let resume = skip_spaces(bytes, at + 2);
                        self.event(at, resume, Kind::Break);
                        broke_at = resume;
                        at = resume;
                    }
                    Some(&next) if next.is_ascii_punctuation() => {
                        self.event(at, at + 2, Kind::Literal(escaped(next)));
                        at += 2;
                    }
                    _ => {
                        at += 1;
                        continue; // a literal backslash is text
                    }
                },
                b'`' => {
                    let run = count(bytes, at, b'`');
                    match closing_backticks(bytes, at + run, run) {
                        Some(end) => {
                            let trim = broke_at == at;
                            self.event(at, end + run, Kind::Code { start: (at + run) as u32, end: end as u32, trim });
                            at = end + run;
                        }
                        None => {
                            at += run;
                            continue; // literal backticks are text
                        }
                    }
                }
                b'>' => {
                    self.event(at, at + 1, Kind::Literal("&gt;"));
                    at += 1;
                }
                // A `&` that cannot start a character reference, a `<` that cannot start
                // HTML or an autolink.
                b'&' if !bytes.get(at + 1).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'#') => {
                    self.event(at, at + 1, Kind::Literal("&amp;"));
                    at += 1;
                }
                b'<' if !bytes
                    .get(at + 1)
                    .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?')) =>
                {
                    self.event(at, at + 1, Kind::Literal("&lt;"));
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
                    if can_open || can_close {
                        let length32 = length as u32;
                        self.delims.push(self.runs.len());
                        let index = self.runs.len() as u32;
                        self.runs.push(Run {
                            marker: byte,
                            count: length32,
                            can_open,
                            can_close,
                            opens: Tags::default(),
                            closes: Tags::default(),
                        });
                        self.event(at, at + length, Kind::Run { index });
                    }
                    at += length;
                    // A run that can neither open nor close is text.
                    if !(can_open || can_close) {
                        continue;
                    }
                }
                // Links, images, raw HTML, references, strikethrough, autolinks: the
                // exact path.
                _ => return Err(Fallback),
            }
            plain = at;
        }
    }

    /// CommonMark's "process emphasis" over every delimiter run; whether any matched.
    fn emphasis(&mut self) -> Result {
        // ponytail: quadratic opener search; add openers_bottom if adversarial input shows up.
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
        Ok(())
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
/// side when both have one and it is not all spaces; after a hard break, no leading spaces.
fn code_content(out: &mut Vec<u8>, raw: &[u8], after_break: bool) {
    let space = |b: u8| b == b' ' || b == b'\n';
    let strip = raw.len() >= 2 && space(raw[0]) && space(raw[raw.len() - 1]) && raw.iter().any(|&b| !space(b));
    let mut raw = if strip { &raw[1..raw.len() - 1] } else { raw };
    if after_break {
        while let [first, rest @ ..] = raw
            && space(*first)
        {
            raw = rest;
        }
    }
    for &byte in raw {
        match byte {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'\n' => out.push(b' '),
            _ => out.push(byte),
        }
    }
}
