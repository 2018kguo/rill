//! Inline content: text, code spans, emphasis and strong, escapes and hard breaks, written
//! as Streamdown's HTML. Emphasis follows CommonMark's delimiter algorithm as micromark
//! runs it.
//!
//! The scan records what is not plain text as events; the emit pass writes the tokens
//! (see `tokens`), the plain text between events copied as it is.

use rill_unicode::{PUNCTUATION_OR_SYMBOL, WHITESPACE, in_table};
use crate::simd::ByteSet;
use crate::tokens::{Out, tag};
use crate::{Fallback, Result};

/// Units past a resume point that scanning before it may read.
const LOOKAHEAD: usize = 3;

/// Bytes that end a run of plain text.
const SPECIAL: ByteSet = ByteSet::new(b"*_`\\\n&<[]~@:.");

/// The first unit at or after `at` that is in `SPECIAL`, or the end.
#[inline(always)]
fn next_special(units: &[u16], mut at: usize) -> usize {
    while let Some(chunk) = units.get(at..at + 16) {
        let mask = SPECIAL.mask16_units(chunk.try_into().expect("16 units"));
        if mask != 0 {
            return at + mask.trailing_zeros() as usize;
        }
        at += 16;
    }
    while at < units.len() && !(units[at] < 0x80 && SPECIAL.contains(units[at] as u8)) {
        at += 1;
    }
    at
}

/// A unit as an ASCII byte, or 0x80 when it is not ASCII.
#[inline(always)]
fn ascii(unit: u16) -> u8 {
    if unit < 0x80 { unit as u8 } else { 0x80 }
}

#[derive(Default)]
pub struct Inline {
    runs: Vec<Run>,
    /// Indices into `runs` still able to match.
    delims: Vec<usize>,
    /// What the scan found, in source order; the text between them is copied as it is.
    events: Vec<Event>,
    /// Where the last scan left the first backtick run without a closer (a later one
    /// may close it).
    open_ticks: Option<usize>,
    /// The spans of matched emphasis, from opener to closer.
    pairs: Vec<(u32, u32)>,
    /// The last render's tokens up to a point no later text can change.
    resume: Resume,
}

/// The tokens of `[..at]` of the content starting at `start` in the whole text: final
/// once no delimiter run or backtick run before `at` can pair with one after it.
#[derive(Default)]
struct Resume {
    start: usize,
    at: usize,
    words: Vec<u32>,
    text: Vec<u16>,
    origins: Vec<u32>,
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
    /// A fixed replacement (an escaped character).
    Literal(&'static str),
    /// A code span's content `[start, end)`; `trim` after a hard break.
    Code { start: u32, end: u32, trim: bool },
    /// A delimiter run, `runs[index]`.
    Run { index: u32 },
}

/// A run of `*` or `_`.
struct Run {
    /// Where the run is in the text.
    at: u32,
    end: u32,
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
    fn write(self, out: &mut Out, open: bool) {
        for bit in 0..self.len {
            if open {
                out.open(if self.bits >> bit & 1 == 1 { tag::STRONG } else { tag::EM });
            } else {
                out.close();
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Punct,
    Other,
}

/// micromark's `classifyCharacter`, which classifies UTF-16 units: a surrogate is "other".
fn class(unit: Option<u16>) -> Class {
    let Some(unit) = unit else {
        return Class::Space;
    };
    if unit < 0x80 {
        return match unit as u8 {
            b' ' | b'\n' | b'\t' | b'\r' | 0x0b | 0x0c => Class::Space,
            byte if byte.is_ascii_punctuation() => Class::Punct,
            _ => Class::Other,
        };
    }
    match char::from_u32(u32::from(unit)) {
        None => Class::Other,
        Some(c) if in_table(WHITESPACE, c) => Class::Space,
        Some(c) if in_table(PUNCTUATION_OR_SYMBOL, c) => Class::Punct,
        Some(_) => Class::Other,
    }
}

/// ASCII punctuation that a backslash escapes, as itself.
fn escaped(byte: u8) -> &'static str {
    const PUNCT: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
    let at = PUNCT.find(char::from(byte)).expect("ASCII punctuation");
    &PUNCT[at..=at]
}

impl Inline {
    /// Renders `text` into `out`: a paragraph's lines from its first content character
    /// to its last, continuation lines with their indents.
    pub fn render(&mut self, text: &[u16], out: &mut Out, stable: usize) -> Result {
        self.runs.clear();
        self.delims.clear();
        self.events.clear();
        self.pairs.clear();
        self.open_ticks = None;
        // The content's start in the whole text, and how much of it is as last time.
        let start = out.offset_of(text);
        let unchanged = stable.saturating_sub(start.unwrap_or(usize::MAX));
        let mut from = 0;
        if start == Some(self.resume.start) && self.resume.at > 0 && self.resume.at + LOOKAHEAD <= unchanged.min(text.len()) {
            out.extend(&self.resume.words, &self.resume.text, &self.resume.origins);
            from = self.resume.at;
        }
        let base = (out.words.len() - self.resume.words.len() * usize::from(from > 0), out.text.len() - self.resume.text.len() * usize::from(from > 0));
        self.scan(text, from)?;
        self.emphasis()?;
        // The next render's resume point: the latest one this render can vouch for.
        // (The next render checks that the text is the same through it and its lookahead.)
        let next = start.and_then(|_| self.checkpoint(text, from, text.len().saturating_sub(LOOKAHEAD)));
        self.resume.at = 0;
        let mut cursor = from;
        let record = |out: &Out, resume: &mut Resume, at: usize| {
            resume.words.clear();
            resume.words.extend_from_slice(&out.words[base.0..]);
            resume.origins.clear();
            resume.origins.extend_from_slice(&out.origins[base.0..]);
            resume.text.clear();
            resume.text.extend_from_slice(&out.text[base.1..]);
            resume.start = start.expect("a source slice");
            resume.at = at;
        };
        for event in &self.events {
            if let Some(next) = next
                && cursor <= next
                && next <= event.at as usize
            {
                out.text(&text[cursor..next]);
                record(out, &mut self.resume, next);
                cursor = next;
            }
            out.text(&text[cursor..event.at as usize]);
            match event.kind {
                Kind::Line { hard: false } => out.ascii("\n"),
                Kind::Line { hard: true } | Kind::Break => {
                    out.void(tag::BR);
                    out.ascii("\n");
                }
                Kind::Literal(literal) => out.ascii(literal),
                Kind::Code { start, end, trim } => {
                    out.open(tag::CODE);
                    code_content(out, &text[start as usize..end as usize], trim);
                    out.close();
                }
                Kind::Run { index } => {
                    let run = &self.runs[index as usize];
                    run.closes.write(out, false);
                    out.repeat(run.marker, run.count);
                    run.opens.write(out, true);
                }
            }
            cursor = event.resume as usize;
        }
        if let Some(next) = next
            && cursor <= next
        {
            out.text(&text[cursor..next]);
            record(out, &mut self.resume, next);
            cursor = next;
        }
        out.text(&text[cursor..]);
        Ok(())
    }

    /// The latest position up to `limit` the next render may resume at: before every
    /// run that may still open and every backtick run that may still open a code span,
    /// inside no matched emphasis, in plain text after a unit that starts nothing (so
    /// the scan from there is the scan through there).
    fn checkpoint(&self, text: &[u16], from: usize, limit: usize) -> Option<usize> {
        let mut ceiling = limit.min(self.open_ticks.unwrap_or(usize::MAX));
        for run in &self.runs {
            if run.can_open && run.count > 0 {
                ceiling = ceiling.min(run.at as usize);
                break;
            }
        }
        let valid = |at: usize| {
            let unit = text[at - 1];
            unit != 0x20
                && !(unit < 0x80 && SPECIAL.contains(unit as u8))
                && !self.pairs.iter().any(|&(start, end)| (start as usize) < at && at < end as usize)
        };
        // The plain stretches between events, from the last.
        let mut end = text.len();
        for event in self.events.iter().rev().map(Some).chain([None]) {
            let gap = event.map_or(from, |event| event.resume as usize);
            let mut at = end.min(ceiling);
            while at > gap.max(from) {
                if valid(at) {
                    return Some(at);
                }
                at -= 1;
            }
            let Some(event) = event else { break };
            end = event.at as usize;
        }
        None
    }

    fn event(&mut self, at: usize, resume: usize, kind: Kind) {
        self.events.push(Event { at: at as u32, resume: resume as u32, kind });
    }

    fn scan(&mut self, text: &[u16], from: usize) -> Result {
        let bytes = text;
        if u32::try_from(bytes.len()).is_err() {
            return Err(Fallback);
        }

        let mut at = from;
        // Where plain text began: the spaces before a line ending are trimmed back to it.
        let mut plain = from;
        // mdast-util-to-hast trims the start of whatever follows a hard break: the source
        // position right after the last break.
        let mut broke_at = usize::MAX;
        loop {
            at = next_special(bytes, at);
            let Some(&unit) = bytes.get(at) else {
                return Ok(());
            };
            let byte = unit as u8; // in SPECIAL: ASCII
            match byte {
                b'\n' => {
                    let mut spaces = at;
                    while spaces > plain && bytes[spaces - 1] == 0x20 {
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
                b'\\' => match bytes.get(at + 1).map(|&unit| ascii(unit)) {
                    Some(b'\n') => {
                        let resume = skip_spaces(bytes, at + 2);
                        self.event(at, resume, Kind::Break);
                        broke_at = resume;
                        at = resume;
                    }
                    // As before the escape, for the autolink literals below.
                    Some(b'@') => return Err(Fallback),
                    Some(b':') if bytes.get(at + 2..at + 4) == Some(&[0x2F, 0x2F]) => return Err(Fallback),
                    Some(b'.') if www_before(bytes, at) => return Err(Fallback),
                    Some(next) if next.is_ascii_punctuation() => {
                        self.event(at, at + 2, Kind::Literal(escaped(next)));
                        at += 2;
                    }
                    _ => {
                        at += 1;
                        continue; // a literal backslash is text
                    }
                },
                b'`' => {
                    let run = count(bytes, at, unit);
                    match closing_backticks(bytes, at + run, run) {
                        Some(end) => {
                            let trim = broke_at == at;
                            self.event(at, end + run, Kind::Code { start: (at + run) as u32, end: end as u32, trim });
                            at = end + run;
                        }
                        None => {
                            self.open_ticks.get_or_insert(at);
                            at += run;
                            continue; // literal backticks are text
                        }
                    }
                }
                // A `&` that cannot start a character reference, a `<` that cannot start
                // HTML or an autolink: text.
                b'&' if !bytes.get(at + 1).map(|&unit| ascii(unit)).is_some_and(|b| b.is_ascii_alphanumeric() || b == b'#') => {
                    at += 1;
                }
                b'<' if !bytes
                    .get(at + 1)
                    .map(|&unit| ascii(unit))
                    .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?')) =>
                {
                    at += 1;
                }
                // GFM autolink literals (`www.`, `scheme://`, emails) take the exact path.
                b'@' => return Err(Fallback),
                b':' | b'.' => {
                    let suspect = if byte == b':' {
                        bytes.get(at + 1..at + 3) == Some(&[0x2F, 0x2F])
                    } else {
                        www_before(bytes, at)
                    };
                    if suspect {
                        return Err(Fallback);
                    }
                    at += 1;
                    continue; // text
                }
                b'*' | b'_' => {
                    let length = count(bytes, at, unit);
                    let before = class(at.checked_sub(1).map(|at| text[at]));
                    let after = class(text.get(at + length).copied());
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
                            at: at as u32,
                            end: (at + length) as u32,
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
            self.pairs.push((self.runs[opener].at, self.runs[closer].end));
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

/// Whether `www` (any case) ends at `at`.
fn www_before(units: &[u16], at: usize) -> bool {
    at >= 3 && units[at - 3..at].iter().all(|&u| ascii(u) | 0x20 == b'w')
}

fn count(units: &[u16], at: usize, unit: u16) -> usize {
    units[at..].iter().take_while(|&&u| u == unit).count()
}

fn skip_spaces(units: &[u16], mut at: usize) -> usize {
    while units.get(at) == Some(&0x20) {
        at += 1;
    }
    at
}

/// The start of the backtick run of exactly `run` ticks closing a code span opened at
/// `from`, or `None`.
fn closing_backticks(units: &[u16], from: usize, run: usize) -> Option<usize> {
    const TICK: u16 = b'`' as u16;
    let mut at = from;
    while let Some(offset) = crate::simd::find_unit(&units[at..], TICK) {
        at += offset;
        let length = count(units, at, TICK);
        if length == run {
            return Some(at);
        }
        at += length;
    }
    None
}

/// A code span's content: line endings as spaces, one space stripped from each side when
/// both have one and it is not all spaces; after a hard break, no leading spaces.
fn code_content(out: &mut Out, raw: &[u16], after_break: bool) {
    let space = |u: u16| u == 0x20 || u == 0x0A;
    let strip = raw.len() >= 2 && space(raw[0]) && space(raw[raw.len() - 1]) && raw.iter().any(|&u| !space(u));
    let mut raw = if strip { &raw[1..raw.len() - 1] } else { raw };
    if after_break {
        let lead = raw.iter().take_while(|&&u| space(u)).count();
        raw = &raw[lead..];
    }
    for (index, line) in raw.split(|&u| u == 0x0A).enumerate() {
        if index > 0 {
            out.ascii(" ");
        }
        out.text(line);
    }
}
