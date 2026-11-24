//! Inline content: text, code spans, emphasis and strong, links and autolinks, character
//! references, escapes and hard breaks, written as Streamdown's HTML. Emphasis and links
//! follow CommonMark's delimiter and bracket algorithms as micromark runs them.
//!
//! The scan records what is not plain text as events; the emit pass writes the tokens
//! (see `tokens`), the plain text between events copied as it is.

use crate::simd::ByteSet;
use crate::tokens::{ISLAND, Out, overrides, setup, tag};
use crate::{Fallback, Result};
use rill_unicode::{PUNCTUATION_OR_SYMBOL, WHITESPACE, in_table};

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

/// A text's definitions and footnotes (Streamdown keeps a text with any in one block).
#[derive(Default)]
pub(crate) struct Refs {
    /// Link definitions: normalized label, destination (the first definition wins).
    /// Each definition's label, destination and title.
    pub links: Vec<(Vec<u16>, String, Option<String>)>,
    /// Footnote definitions: lowercased label, node (the first definition wins).
    pub notes: Vec<(Vec<u16>, u32)>,
    /// Footnotes called so far, in first-call order: `notes` index, call count.
    pub calls: Vec<(u32, u32)>,
}

impl Refs {
    pub fn clear(&mut self) {
        self.links.clear();
        self.notes.clear();
        self.calls.clear();
    }

    fn link(&self, label: &[u16]) -> Option<(&str, Option<&str>)> {
        self.links
            .iter()
            .find(|(name, ..)| name == label)
            .map(|(_, destination, title)| (destination.as_str(), title.as_deref()))
    }

    fn note(&self, label: &[u16]) -> Option<u32> {
        self.notes
            .iter()
            .position(|(name, _)| {
                name.len() == label.len() && name.iter().zip(label).all(|(&a, &b)| a == lower(b))
            })
            .map(|at| at as u32)
    }

    /// A call of footnote `note`: its number.
    fn call(&mut self, note: u32) -> u32 {
        match self.calls.iter().position(|&(called, _)| called == note) {
            Some(at) => {
                self.calls[at].1 += 1;
                at as u32 + 1
            }
            None => {
                self.calls.push((note, 1));
                self.calls.len() as u32
            }
        }
    }
}

pub(crate) fn lower(unit: u16) -> u16 {
    if (0x41..=0x5A).contains(&unit) {
        unit + 32
    } else {
        unit
    }
}

/// A footnote label's units: `[A-Za-z0-9_-]` (Streamdown's test; others take the exact path).
pub(crate) fn note_unit(unit: u16) -> bool {
    unit < 0x80
        && (unit as u8 == b'_' || unit as u8 == b'-' || (unit as u8).is_ascii_alphanumeric())
}

/// micromark's `normalizeIdentifier` for an ASCII label (whitespace collapsed, case
/// folded); `None` for one this does not fold (non-ASCII, escapes, brackets).
pub(crate) fn normalize_label(label: &[u16]) -> Option<Vec<u16>> {
    let mut out = Vec::with_capacity(label.len());
    for &unit in label {
        match unit {
            0x20 | 0x09 | 0x0A | 0x0D => {
                if out.last().is_some_and(|&last| last != 0x20) {
                    out.push(0x20);
                }
            }
            0x5B..=0x5D => return None,
            0x80.. => return None,
            _ => out.push(lower(unit)),
        }
    }
    if out.last() == Some(&0x20) {
        out.pop();
    }
    (!out.is_empty() && label.len() <= 999).then_some(out)
}

#[derive(Default)]
pub struct Inline {
    /// The text's definitions and footnotes (empty outside a text Streamdown keeps whole).
    pub(crate) refs: Refs,
    runs: Vec<Run>,
    /// Indices into `runs` still able to match.
    delims: Vec<usize>,
    /// Indices into `runs` of `~` runs still able to match.
    tildes: Vec<usize>,
    /// The first run's marker and place: micromark resolves strikethrough first when a
    /// `~` run comes before every `*`/`_` run.
    first: Option<(u8, u32)>,
    /// What the scan found, in source order; the text between them is copied as it is.
    events: Vec<Event>,
    /// Where the last scan left the first backtick run without a closer (a later one
    /// may close it).
    open_ticks: Option<usize>,
    /// The spans of matched emphasis and links, from opener to closer.
    pairs: Vec<(u32, u32)>,
    /// Open `[`s, innermost last.
    brackets: Vec<Bracket>,
    /// Each link's rendering.
    links: Vec<Link>,
    /// The page's link rewriting ([`Out::href`]), for this render.
    href: Option<crate::tokens::Href>,
    /// Decoded character references, which events point into.
    decoded: Vec<u16>,
    /// The earliest position more text could change the scan at (an unclosed bracket, a
    /// character reference that may not have ended).
    ceiling: usize,
    /// The last render's tokens up to a point no later text can change.
    resume: Resume,
}

/// The tokens of `[..at]` of the content starting at `start` in the whole text: final
/// once no delimiter run or backtick run before `at` can pair with one after it.
#[derive(Default)]
struct Resume {
    start: usize,
    at: usize,
    first: Option<(u8, u32)>,
    words: Vec<u32>,
    text: Vec<u16>,
    origins: Vec<u32>,
}

/// A `[` that may still open a link.
struct Bracket {
    /// Its event (a `Kind::Open`).
    event: usize,
    /// How many delimiter runs came before it.
    delims: usize,
    tildes: usize,
    /// No link encloses another: a link closing deactivates the brackets before it.
    active: bool,
}

/// How a link renders (Streamdown's link safety: a button, unless rehype-harden blocks
/// its URL).
enum Link {
    /// Its `href` (hardened) and title, for a page's own `a`.
    Button(String, Option<String>),
    Incomplete,
    /// A span titled `Blocked URL: ...`, the text followed by ` [blocked]`.
    Blocked(String),
}

/// No link: a `Kind::Open` that is text.
const NO_LINK: u32 = u32::MAX;

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
    /// Raw HTML `<br>` (`<br/>`, `<br />`): the element alone.
    RawBreak,
    /// A fixed replacement (an escaped character).
    Literal(&'static str),
    /// A code span's content `[start, end)`; `trim` after a hard break.
    Code { start: u32, end: u32, trim: bool },
    /// A delimiter run, `runs[index]`.
    Run { index: u32 },
    /// A `[`: link `link`'s start, or text.
    Open { link: u32 },
    /// The end of link `link` (`](destination "title")`).
    Close { link: u32 },
    /// An autolink, `<...>`: link `link` around the text `[start, end)`.
    Autolink { link: u32, start: u32, end: u32 },
    /// A character reference: `decoded[start..end]`.
    Decoded { start: u32, end: u32 },
    /// A footnote call of `refs.notes[note]`.
    Note { note: u32 },
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
    /// A `~` run that opens (1) or closes (2) a strikethrough.
    del: u8,
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
                out.open(if self.bits >> bit & 1 == 1 {
                    tag::STRONG
                } else {
                    tag::EM
                });
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
    /// to its last, each continuation line after `strip` spaces of container indent.
    pub fn render(&mut self, text: &[u16], strip: usize, out: &mut Out, stable: usize) -> Result {
        self.runs.clear();
        self.delims.clear();
        self.tildes.clear();
        self.first = None;
        self.events.clear();
        self.pairs.clear();
        self.brackets.clear();
        self.links.clear();
        self.decoded.clear();
        self.ceiling = usize::MAX;
        self.open_ticks = None;
        self.href.clone_from(&out.href);
        // The content's start in the whole text, and how much of it is as last time.
        let start = out.offset_of(text);
        let unchanged = stable.saturating_sub(start.unwrap_or(usize::MAX));
        let mut from = 0;
        if start == Some(self.resume.start)
            && self.resume.at > 0
            && self.resume.at + LOOKAHEAD <= unchanged.min(text.len())
        {
            out.extend(&self.resume.words, &self.resume.text, &self.resume.origins);
            from = self.resume.at;
            self.first = self.resume.first;
        }
        let base = (
            out.words.len() - self.resume.words.len() * usize::from(from > 0),
            out.text.len() - self.resume.text.len() * usize::from(from > 0),
        );
        self.scan(text, from)?;
        // Brackets still open may close later.
        if let Some(first) = self.brackets.first() {
            self.ceiling = self.ceiling.min(self.events[first.event].at as usize);
        }
        self.resolve(0, 0)?;
        // The next render's resume point: the latest one this render can vouch for.
        // (The next render checks that the text is the same through it and its lookahead.)
        let next =
            start.and_then(|_| self.checkpoint(text, from, text.len().saturating_sub(LOOKAHEAD)));
        self.resume.at = 0;
        let mut cursor = from;
        // mdast-util-to-hast trims the start of the first text after a hard break, wherever
        // it lands (inside a link, say).
        let mut broke = false;
        let text_after = |out: &mut Out, units: &[u16], broke: &mut bool| {
            let units = if *broke {
                &units[units
                    .iter()
                    .take_while(|&&u| u == 0x20 || u == 0x09)
                    .count()..]
            } else {
                units
            };
            if !units.is_empty() {
                *broke = false;
                out.text(units);
            }
        };
        // The token before the content: text the content joins (a list item's line
        // ending) is not the content's to replay, so no resume point then.
        let first = self.first;
        let before = base
            .0
            .checked_sub(1)
            .map(|at| (out.words[at], out.origins[at]));
        let record = |out: &Out, resume: &mut Resume, at: usize| {
            if before
                .is_some_and(|before| (out.words[base.0 - 1], out.origins[base.0 - 1]) != before)
            {
                return;
            }
            resume.words.clear();
            resume.words.extend_from_slice(&out.words[base.0..]);
            resume.origins.clear();
            resume.origins.extend_from_slice(&out.origins[base.0..]);
            resume.text.clear();
            resume.text.extend_from_slice(&out.text[base.1..]);
            resume.start = start.expect("a source slice");
            resume.at = at;
            resume.first = first.filter(|&(_, run)| (run as usize) < at);
        };
        for event in &self.events {
            if let Some(next) = next
                && cursor <= next
                && next <= event.at as usize
            {
                text_after(out, &text[cursor..next], &mut broke);
                record(out, &mut self.resume, next);
                cursor = next;
            }
            text_after(out, &text[cursor..event.at as usize], &mut broke);
            match event.kind {
                Kind::Line { hard: false } => out.ascii("\n"),
                Kind::RawBreak => {
                    broke = false;
                    out.void(tag::BR);
                }
                Kind::Line { hard: true } | Kind::Break => {
                    out.void(tag::BR);
                    out.ascii("\n");
                    broke = true;
                }
                Kind::Literal(literal) => {
                    broke = false;
                    out.ascii(literal);
                }
                Kind::Code { start, end, trim } => {
                    broke = false;
                    out.open(tag::CODE);
                    code_content(out, &text[start as usize..end as usize], trim, strip);
                    out.close();
                }
                Kind::Run { index } if self.runs[index as usize].del > 0 => {
                    broke = false;
                    if self.runs[index as usize].del == 1 {
                        out.open(tag::DEL);
                    } else {
                        out.close();
                    }
                }
                Kind::Run { index } => {
                    let run = &self.runs[index as usize];
                    run.closes.write(out, false);
                    if run.count > 0 {
                        broke = false;
                    }
                    out.repeat(run.marker, run.count);
                    run.opens.write(out, true);
                }
                Kind::Open { link: NO_LINK } => text_after(
                    out,
                    &text[event.at as usize..event.resume as usize],
                    &mut broke,
                ),
                Kind::Open { link } => self.links[link as usize].open(out),
                Kind::Close { link } => self.links[link as usize].close(out),
                Kind::Autolink { link, start, end } => {
                    self.links[link as usize].open(out);
                    text_after(out, &text[start as usize..end as usize], &mut broke);
                    self.links[link as usize].close(out);
                }
                Kind::Decoded { start, end } => {
                    text_after(out, &self.decoded[start as usize..end as usize], &mut broke)
                }
                Kind::Note { note } => {
                    broke = false;
                    let number = self.refs.call(note);
                    out.open_html(SUP);
                    if out.options.overrides & overrides::A != 0 {
                        // mdast-util-to-hast's call, as the page's own `a`.
                        let id = note_id(&self.refs.notes[note as usize].0)?;
                        let call = self.refs.calls[number as usize - 1].1;
                        let suffix = if call > 1 { format!("-{call}") } else { String::new() };
                        let p = clobber(out);
                        out.open_html(&format!(
                            r##"{ISLAND}<a href="#user-content-fn-{id}" id="{p}user-content-fnref-{id}{suffix}" data-footnote-ref="" aria-describedby="{p}footnote-label" target="_blank" rel="noopener noreferrer">"##
                        ));
                    } else {
                        out.open(tag::LINK);
                    }
                    out.ascii(&number.to_string());
                    out.close();
                    out.close();
                }
            }
            cursor = event.resume as usize;
        }
        if let Some(next) = next
            && cursor <= next
        {
            text_after(out, &text[cursor..next], &mut broke);
            record(out, &mut self.resume, next);
            cursor = next;
        }
        text_after(out, &text[cursor..], &mut broke);
        Ok(())
    }

    /// The latest position up to `limit` the next render may resume at: before every
    /// run that may still open and every backtick run that may still open a code span,
    /// inside no matched emphasis, in plain text after a unit that starts nothing (so
    /// the scan from there is the scan through there).
    fn checkpoint(&self, text: &[u16], from: usize, limit: usize) -> Option<usize> {
        let mut ceiling = limit
            .min(self.open_ticks.unwrap_or(usize::MAX))
            .min(self.ceiling);
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
                // Inside a run of `gfmAtext`, only once it has ended (not in `@`/`:`).
                && (!atext(unit) || {
                    let end = at + text[at..].iter().take_while(|&&unit| atext(unit)).count();
                    end < (at + LOOKAHEAD).min(text.len()) && !matches!(ascii(text[end]), b'@' | b':')
                })
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

    /// The autolink literal around `trigger`, if any: micromark tries one at each place of
    /// the run of `gfmAtext` before it, left to right. Returns where it ends.
    fn literal(&mut self, text: &[u16], trigger: usize, from: usize) -> Result<Option<usize>> {
        use rill_cmark::autolink_literal::{LiteralKind, scan};
        let floor = self
            .events
            .last()
            .map_or(from, |event| (event.resume as usize).max(from));
        let mut start = trigger;
        while start > 0 && atext(text[start - 1]) {
            start -= 1;
        }
        // ponytail: a run across another construct (an `_` run, an escape), or inside an
        // open `[` (micromark's `previousUnbalanced`), takes the exact path.
        if start < floor || !self.brackets.is_empty() {
            return Err(Fallback);
        }
        // ponytail: the rest of the content, per trigger (quadratic in triggers).
        let s = String::from_utf16_lossy(&text[start..]);
        let previous = text[..start].last().map(|&unit| {
            let pair = if (0xDC00..0xE000).contains(&unit) && start >= 2 {
                &text[start - 2..start]
            } else {
                &text[start - 1..start]
            };
            char::decode_utf16(pair.iter().copied())
                .last()
                .and_then(|c| c.ok())
                .unwrap_or('\u{FFFD}')
        });
        // More text could still make or grow a literal unless whitespace follows.
        let settled = |at: usize| text[at..].iter().any(|&unit| unit == 0x20 || unit == 0x0A);
        for p in start..=trigger {
            let before = if p == start {
                previous
            } else {
                Some(char::from(text[p - 1] as u8))
            };
            let Some((end, kind)) = scan(&s, p - start, before) else {
                continue;
            };
            if text[p] == u16::from(b'_') {
                return Err(Fallback); // ponytail: whether attention takes the `_` first
            }
            let end = start + s[..end].encode_utf16().count();
            if text[p..end].contains(&u16::from(b'`')) {
                return Err(Fallback); // ponytail: how code spans pair around the literal
            }
            // ponytail: backslashes in a literal (the exact path's resolution after it
            // differs, rarely matters).
            if text[p..end].contains(&u16::from(b'\\')) {
                return Err(Fallback);
            }
            let literal = String::from_utf16_lossy(&text[p..end]);
            let href = match kind {
                LiteralKind::Email => format!("mailto:{literal}"),
                LiteralKind::Www => format!("http://{literal}"),
                LiteralKind::Http => literal,
            };
            if !settled(end) {
                self.ceiling = self.ceiling.min(start);
            }
            let link = self.link(&href, None);
            self.event(
                p,
                end,
                Kind::Autolink {
                    link,
                    start: p as u32,
                    end: end as u32,
                },
            );
            return Ok(Some(end));
        }
        // mdast's autolink transform may still link what the tokenizer did not (after
        // punctuation, a character reference): the exact path, unless its patterns cannot
        // match yet (`https://` or `www.` with no domain, `a@` with no dotted one).
        let word =
            |unit: Option<&u16>| unit.is_some_and(|&unit| atext(unit) && unit != u16::from(b'+'));
        let may_link = match ascii(text[trigger]) {
            // (An escape or a reference there may decode to one.)
            b':' => {
                word(text.get(trigger + 3))
                    || matches!(
                        text.get(trigger + 3).map(|&unit| ascii(unit)),
                        Some(b'\\' | b'&')
                    )
            }
            // (Its `www(?=\.)[-.\w]+` takes `www.` alone.)
            b'.' => true,
            _ => {
                let label = |at: usize| {
                    at + text[at..]
                        .iter()
                        .take_while(|&&unit| word(Some(&unit)) && unit != u16::from(b'.'))
                        .count()
                };
                let end = label(trigger + 1);
                text[trigger + 1..]
                    .iter()
                    .take(64)
                    .any(|&unit| matches!(ascii(unit), b'\\' | b'&'))
                    || end > trigger + 1
                        && text.get(end) == Some(&u16::from(b'.'))
                        && label(end + 1) > end + 1
            }
        };
        if may_link {
            return Err(Fallback);
        }
        if !settled(trigger) {
            self.ceiling = self.ceiling.min(start);
        }
        Ok(None)
    }

    fn event(&mut self, at: usize, resume: usize, kind: Kind) {
        self.events.push(Event {
            at: at as u32,
            resume: resume as u32,
            kind,
        });
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
                    Some(b':') if bytes.get(at + 2..at + 4) == Some(&[0x2F, 0x2F]) => {
                        return Err(Fallback);
                    }
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
                            self.event(
                                at,
                                end + run,
                                Kind::Code {
                                    start: (at + run) as u32,
                                    end: end as u32,
                                    trim,
                                },
                            );
                            at = end + run;
                        }
                        None => {
                            self.open_ticks.get_or_insert(at);
                            at += run;
                            continue; // literal backticks are text
                        }
                    }
                }
                b'&' => match entity(bytes, at) {
                    Some((_, decoded)) if decoded == UNKNOWN => return Err(Fallback),
                    Some((end, decoded)) => {
                        let start = self.decoded.len() as u32;
                        self.decoded.extend(decoded.encode_utf16());
                        let kind = Kind::Decoded {
                            start,
                            end: self.decoded.len() as u32,
                        };
                        self.event(at, end, kind);
                        at = end;
                    }
                    None => {
                        // A reference not yet ended may end later.
                        if at + MAX_REFERENCE > bytes.len() {
                            self.ceiling = self.ceiling.min(at);
                        }
                        at += 1;
                        continue; // text
                    }
                },
                b'<' => {
                    if let Some((end, start, close, email)) = autolink(bytes, at) {
                        let content = String::from_utf16_lossy(&bytes[start..close]);
                        let href = if email {
                            format!("mailto:{content}")
                        } else {
                            content
                        };
                        let link = self.link(&href, None);
                        self.event(
                            at,
                            end,
                            Kind::Autolink {
                                link,
                                start: start as u32,
                                end: close as u32,
                            },
                        );
                        at = end;
                    } else if bytes
                        .get(at + 1)
                        .map(|&unit| ascii(unit))
                        .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?'))
                    {
                        if let Some(end) = raw_break(bytes, at) {
                            self.event(at, end, Kind::RawBreak);
                            at = end;
                            continue;
                        }
                        if bytes[at..].contains(&u16::from(b'>')) {
                            return Err(Fallback); // raw HTML
                        }
                        // No `>` after it: not HTML, text (a tag still streaming in: the
                        // next update renders from here again).
                        self.ceiling = self.ceiling.min(at);
                        at += 1;
                        continue;
                    } else {
                        at += 1;
                        continue; // text
                    }
                }
                b'[' => {
                    // An image (`![`, the `!` not escaped): the exact path.
                    if at > 0
                        && bytes[at - 1] == u16::from(b'!')
                        && bytes[..at - 1]
                            .iter()
                            .rev()
                            .take_while(|&&u| u == u16::from(b'\\'))
                            .count()
                            % 2
                            == 0
                    {
                        return Err(Fallback);
                    }
                    // A footnote call (`[^label]`, the footnote defined).
                    if !self.refs.notes.is_empty() && bytes.get(at + 1) == Some(&u16::from(b'^')) {
                        let label = bytes[at + 2..]
                            .iter()
                            .take_while(|&&unit| note_unit(unit))
                            .count();
                        let close = at + 2 + label;
                        if bytes.get(close) == Some(&u16::from(b']')) && (1..=200).contains(&label)
                        {
                            if let Some(note) = self.refs.note(&bytes[at + 2..close]) {
                                if !self.brackets.is_empty() {
                                    return Err(Fallback); // ponytail: a call in a link's text
                                }
                                self.event(at, close + 1, Kind::Note { note });
                                at = close + 1;
                                continue;
                            }
                        } else if bytes
                            .get(close)
                            .is_some_and(|&unit| unit != u16::from(b']'))
                        {
                            return Err(Fallback); // ponytail: other labels
                        }
                    }
                    self.brackets.push(Bracket {
                        event: self.events.len(),
                        delims: self.delims.len(),
                        tildes: self.tildes.len(),
                        active: true,
                    });
                    self.event(at, at + 1, Kind::Open { link: NO_LINK });
                    at += 1;
                }
                b']' => {
                    let Some(bracket) = self.brackets.pop() else {
                        at += 1;
                        continue; // text
                    };
                    let mut resource = if bracket.active {
                        inline_link(bytes, at + 1)
                    } else {
                        None
                    };
                    if resource.is_none() && bracket.active && !self.refs.links.is_empty() {
                        resource =
                            self.reference(bytes, self.events[bracket.event].at as usize, at)?;
                    }
                    // Not a link: the brackets are text.
                    let Some((end, destination, title)) = resource else {
                        let open = self.events[bracket.event].at as usize;
                        self.ceiling = self.ceiling.min(open);
                        at += 1;
                        continue;
                    };
                    if destination.contains(UNKNOWN) {
                        return Err(Fallback);
                    }
                    let link = self.link(&destination, title);
                    let open = self.events[bracket.event].at;
                    self.events[bracket.event].kind = Kind::Open { link };
                    // Emphasis inside the link pairs inside it (strikethrough first).
                    self.tildes_first(bracket.tildes, bracket.delims)?;
                    self.emphasis(bracket.delims)?;
                    self.delims.truncate(bracket.delims);
                    self.tildes.truncate(bracket.tildes);
                    self.pairs.push((open, end as u32));
                    self.event(at, end, Kind::Close { link });
                    for earlier in &mut self.brackets {
                        earlier.active = false;
                    }
                    at = end;
                }
                // GFM autolink literals (`www.`, `scheme://`, emails) take the exact path.
                // GFM autolink literals: `x@y.z`, `http(s)://`, `www.`.
                b'@' | b':' | b'.' => {
                    let suspect = match byte {
                        b'@' => at > 0 && atext(bytes[at - 1]),
                        b':' => bytes.get(at + 1..at + 3) == Some(&[0x2F, 0x2F]),
                        _ => www_before(bytes, at),
                    };
                    if suspect && let Some(end) = self.literal(bytes, at, from)? {
                        at = end;
                        plain = at;
                        continue;
                    }
                    at += 1;
                    continue; // text
                }
                b'*' | b'_' | b'~' => {
                    let length = count(bytes, at, unit);
                    if byte == b'~' && length > 2 {
                        at += length;
                        continue; // text
                    }
                    if self.first.is_none() {
                        self.first = Some((byte, at as u32));
                    }
                    let before = class(at.checked_sub(1).map(|at| text[at]));
                    let after = class(text.get(at + length).copied());
                    let mut left =
                        after != Class::Space && (after != Class::Punct || before != Class::Other);
                    let mut right =
                        before != Class::Space && (before != Class::Punct || after != Class::Other);
                    // GFM strikethrough makes `~` an attention marker: `*` and `_` open
                    // before it and close after it.
                    if byte != b'~' {
                        let tilde = u16::from(b'~');
                        left |= text.get(at + length) == Some(&tilde);
                        right |= at > 0 && text[at - 1] == tilde;
                    }
                    let (can_open, can_close) = if byte != b'_' {
                        (left, right)
                    } else {
                        (
                            left && (!right || before == Class::Punct),
                            right && (!left || after == Class::Punct),
                        )
                    };
                    if can_open || can_close {
                        let length32 = length as u32;
                        if byte == b'~' {
                            self.tildes.push(self.runs.len());
                        } else {
                            self.delims.push(self.runs.len());
                        }
                        let index = self.runs.len() as u32;
                        self.runs.push(Run {
                            at: at as u32,
                            end: (at + length) as u32,
                            marker: byte,
                            count: length32,
                            can_open,
                            can_close,
                            del: 0,
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
                _ => unreachable!("in SPECIAL"),
            }
            plain = at;
        }
    }

    /// CommonMark's "process emphasis" over the delimiter runs from `bottom` on.
    /// Resolves the runs from `tildes`/`delims` on, in micromark's resolver order.
    fn resolve(&mut self, tildes: usize, delims: usize) -> Result {
        if self.first.is_some_and(|(marker, _)| marker == b'~') {
            self.tildes_first(tildes, delims)?;
            return self.emphasis(delims);
        }
        let emphasis = self.pairs.len();
        self.emphasis(delims)?;
        let dels = self.pairs.len();
        self.strikethrough(tildes, None)?;
        // ponytail: a strikethrough crossing emphasis takes the exact path.
        let (emphasis, dels) = (&self.pairs[emphasis..dels], &self.pairs[dels..]);
        if dels.iter().any(|&(o, c)| {
            emphasis
                .iter()
                .any(|&(s, e)| (s < o && o < e && e < c) || (o < s && s < c && c < e))
        }) {
            return Err(Fallback);
        }
        Ok(())
    }

    /// Strikethrough resolved first: emphasis inside each pair pairs inside it.
    fn tildes_first(&mut self, tildes: usize, delims: usize) -> Result {
        self.strikethrough(tildes, Some(delims))
    }

    /// Pairs `~` runs from `tildes` on: each closer with the nearest opener of its length.
    /// With `region`, emphasis runs (from that index) between the two are resolved then.
    fn strikethrough(&mut self, tildes: usize, region: Option<usize>) -> Result {
        let mut closer_at = tildes;
        while closer_at < self.tildes.len() {
            let c = &self.runs[self.tildes[closer_at]];
            let (count, can_close) = (c.count, c.can_close);
            let found = if can_close {
                (tildes..closer_at).rev().find(|&o| {
                    self.runs[self.tildes[o]].can_open && self.runs[self.tildes[o]].count == count
                })
            } else {
                None
            };
            let Some(opener_at) = found else {
                closer_at += 1;
                continue;
            };
            // ponytail: micromark leaves the openers between them able to pair later
            // (crossing); the exact path.
            if self.tildes[opener_at + 1..closer_at]
                .iter()
                .any(|&run| self.runs[run].can_open)
            {
                return Err(Fallback);
            }
            let (opener, closer) = (self.tildes[opener_at], self.tildes[closer_at]);
            let (start, end) = (self.runs[opener].end, self.runs[closer].at);
            if let Some(bottom) = region {
                let lo = bottom
                    + self.delims[bottom..].partition_point(|&run| self.runs[run].at < start);
                let hi = lo + self.delims[lo..].partition_point(|&run| self.runs[run].at < end);
                let rest = self.delims.split_off(hi);
                let inner = self.delims.split_off(lo);
                let outer = std::mem::replace(&mut self.delims, inner);
                self.emphasis(0)?;
                self.delims = outer;
                self.delims.extend(rest);
            }
            for (run, del) in [(opener, 1), (closer, 2)] {
                self.runs[run].count = 0;
                self.runs[run].del = del;
            }
            self.pairs
                .push((self.runs[opener].at, self.runs[closer].end));
            self.tildes.drain(opener_at..=closer_at);
            closer_at = opener_at;
        }
        Ok(())
    }

    fn emphasis(&mut self, bottom: usize) -> Result {
        // ponytail: quadratic opener search; add openers_bottom if adversarial input shows up.
        let mut closer_at = bottom;
        while closer_at < self.delims.len() {
            let closer = self.delims[closer_at];
            let c = &self.runs[closer];
            if !(c.can_close && c.count > 0) {
                closer_at += 1;
                continue;
            }
            let (marker, close_count, closer_opens) = (c.marker, c.count, c.can_open);
            let found = (bottom..closer_at).rev().find(|&opener_at| {
                let o = &self.runs[self.delims[opener_at]];
                o.marker == marker
                    && o.can_open
                    && o.count > 0
                    // micromark applies the rule of three to what is left of each run.
                    && !((o.can_close || closer_opens)
                        && (o.count + close_count).is_multiple_of(3)
                        && !(o.count.is_multiple_of(3) && close_count % 3 == 0))
            });
            let Some(opener_at) = found else {
                closer_at += 1;
                continue;
            };
            let opener = self.delims[opener_at];
            let open_count = self.runs[opener].count;
            let used = if open_count >= 2 && close_count >= 2 {
                2
            } else {
                1
            };
            self.runs[opener].count -= used;
            self.runs[opener].opens.push_front(used == 2)?;
            self.runs[closer].count -= used;
            self.runs[closer].closes.push_back(used == 2)?;
            self.pairs
                .push((self.runs[opener].at, self.runs[closer].end));
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

impl Inline {
    /// A link to `destination` (decoded): how Streamdown renders it.
    /// A reference link closing at `close` (`[text][label]`, `[text][]`, `[text]`) whose
    /// label is defined: where it ends, and the destination.
    fn reference(
        &self,
        bytes: &[u16],
        open: usize,
        close: usize,
    ) -> Result<Option<(usize, String, Option<String>)>> {
        let text = &bytes[open + 1..close];
        let (label, end) = if bytes.get(close + 1) == Some(&u16::from(b'[')) {
            let Some(length) = bytes[close + 2..]
                .iter()
                .position(|&unit| unit == u16::from(b']') || unit == u16::from(b'['))
            else {
                return Err(Fallback); // ponytail: a label still open
            };
            let end = close + 2 + length;
            if bytes[end] == u16::from(b'[') {
                return Err(Fallback);
            }
            if length == 0 {
                (text, end + 1)
            } else {
                (&bytes[close + 2..end], end + 1)
            }
        } else if bytes.get(close + 1) == Some(&u16::from(b'\\')) {
            return Err(Fallback); // ponytail: an escape right after a shortcut reference
        } else {
            (text, close + 1)
        };
        let Some(label) = normalize_label(label) else {
            // A label this does not fold may name a definition; text with brackets inside
            // (no label) is text.
            return if text
                .iter()
                .any(|&unit| unit >= 0x80 || unit == u16::from(b'\\'))
            {
                Err(Fallback)
            } else {
                Ok(None)
            };
        };
        match self.refs.link(&label) {
            Some((destination, title)) => Ok(Some((
                end,
                destination.to_string(),
                title.map(str::to_owned),
            ))),
            // An undefined full reference: micromark may still read a shortcut there.
            None if end > close + 1 => Err(Fallback),
            None => Ok(None),
        }
    }

    fn link(&mut self, destination: &str, title: Option<String>) -> u32 {
        let link = if destination == "streamdown:incomplete-link" {
            Link::Incomplete
        } else {
            // micromark's normalizeUri, the sanitize schema's protocols, rehype-harden.
            let mut href = rill_url::normalize_uri(destination.into());
            if let Some(rewritten) = self.href.as_ref().and_then(|hook| hook(&href)) {
                href = rewritten.into();
            }
            let href = rill_url::protocol_allowed(&href, HREF_PROTOCOLS).then_some(href);
            match href
                .as_deref()
                .map(|href| rill_url::transform_url(href, rill_url::Resource::Link))
            {
                Some(Ok(changed)) => Link::Button(
                    changed.unwrap_or_else(|| href.unwrap_or_default().into_owned()),
                    title,
                ),
                _ => Link::Blocked(format!(
                    "Blocked URL: {}",
                    href.as_deref().unwrap_or("undefined")
                )),
            }
        };
        self.links.push(link);
        (self.links.len() - 1) as u32
    }
}

/// Streamdown's `sup`.
pub(crate) const SUP: &str = r#"<sup class="text-sm" data-streamdown="superscript">"#;

/// The protocols Streamdown's sanitize schema allows in `href`.
const HREF_PROTOCOLS: &[&str] = &[
    "http",
    "https",
    "irc",
    "ircs",
    "mailto",
    "xmpp",
    "tel",
    "streamdown",
];

impl Link {
    fn open(&self, out: &mut Out) {
        let island = out.options.overrides;
        let text_only = out.options.setup & setup::TEXT_ONLY_LINKS != 0;
        match self {
            Link::Blocked(_) | Link::Incomplete if text_only => {
                out.open_html(if island & overrides::SPAN != 0 { "\u{1}<span>" } else { "<span>" })
            }
            Link::Button(href, title) if island & overrides::A != 0 => {
                out.open_html(&a_island(href, title.as_deref()))
            }
            Link::Incomplete if island & overrides::A != 0 => {
                out.open_html(&a_island("streamdown:incomplete-link", None))
            }
            Link::Button(..) => out.open(tag::LINK),
            Link::Incomplete => out.open(tag::LINK_INCOMPLETE),
            Link::Blocked(title) => {
                let mut html = String::new();
                if island & overrides::SPAN != 0 {
                    html.push(ISLAND);
                }
                html.push_str("<span title=\"");
                escape_attribute(title, &mut html);
                html.push_str("\" class=\"text-gray-500\">");
                out.open_html(&html);
            }
        }
    }

    fn close(&self, out: &mut Out) {
        if let Link::Blocked(_) = self
            && out.options.setup & setup::TEXT_ONLY_LINKS == 0
        {
            // (React keeps it a text node of its own; the HTML is the same.)
            out.ascii(" [blocked]");
        }
        out.close();
    }
}

/// A page's own `a`: the island Streamdown's HAST gives it.
/// A footnote's id in mdast-util-to-hast's attributes (its label, lowercased already).
/// ponytail: a label normalizeUri would change takes the exact path.
pub(crate) fn note_id(label: &[u16]) -> Result<String> {
    let safe = |&unit: &u16| unit < 0x80 && (unit as u8).is_ascii_alphanumeric() || unit == u16::from(b'-') || unit == u16::from(b'_');
    if !label.iter().all(safe) {
        return Err(Fallback);
    }
    Ok(String::from_utf16_lossy(label))
}

/// rehype-sanitize's prefix for clobbered attributes ([`setup::SANITIZE`]), or none.
pub(crate) fn clobber(out: &Out) -> &'static str {
    if out.options.setup & setup::SANITIZE != 0 { "user-content-" } else { "" }
}

fn a_island(href: &str, title: Option<&str>) -> String {
    let mut html = String::from(ISLAND);
    html.push_str("<a href=\"");
    escape_attribute(href, &mut html);
    if let Some(title) = title {
        html.push_str("\" title=\"");
        escape_attribute(title, &mut html);
    }
    html.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\">");
    html
}

fn escape_attribute(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

/// The longest character reference, `&` to `;` (`&CounterClockwiseContourIntegral;`).
const MAX_REFERENCE: usize = 33;

/// A character reference at `at` (`&name;`, `&#digits;`, `&#xhex;`): where it ends and
/// what it decodes to.
fn entity(units: &[u16], at: usize) -> Option<(usize, String)> {
    let window = &units[at..units.len().min(at + MAX_REFERENCE)];
    let semi = window.iter().position(|&u| u == u16::from(b';'))?;
    let body: String = window[1..semi]
        .iter()
        .map(|&u| {
            char::from_u32(u32::from(u))
                .filter(char::is_ascii)
                .map_or('\u{80}', |c| c)
        })
        .collect();
    if !body.is_ascii() || body.is_empty() {
        return None;
    }
    let decoded = if let Some(number) = body.strip_prefix('#') {
        let (digits, radix, max) = match number.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16, 6),
            None => (number, 10, 7),
        };
        if digits.is_empty() || digits.len() > max || !digits.chars().all(|c| c.is_digit(radix)) {
            return None;
        }
        let code = u32::from_str_radix(digits, radix).ok()?;
        // micromark: NUL, surrogates and out-of-range codes become U+FFFD.
        let c = match code {
            0 => '\u{FFFD}',
            code => char::from_u32(code).unwrap_or('\u{FFFD}'),
        };
        c.to_string()
    } else {
        if !body.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        // A name outside the common ones may still be one of HTML's 2,231: the exact path
        // decides (`UNKNOWN`), which keeps their table out of the build.
        common_entity(&body).unwrap_or(UNKNOWN).to_owned()
    };
    Some((at + semi + 1, decoded))
}

/// What [`entity`] decodes a name it does not know to.
const UNKNOWN: &str = "\u{FFFF}";

/// The named character references model answers use.
fn common_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" | "AMP" => "&",
        "lt" | "LT" => "<",
        "gt" | "GT" => ">",
        "quot" | "QUOT" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "copy" | "COPY" => "©",
        "reg" | "REG" => "®",
        "trade" | "TRADE" => "™",
        "hellip" | "mldr" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "‘",
        "rsquo" | "rsquor" => "’",
        "ldquo" => "“",
        "rdquo" | "rdquor" => "”",
        "laquo" => "«",
        "raquo" => "»",
        "bull" | "bullet" => "•",
        "middot" | "centerdot" => "·",
        "times" => "×",
        "divide" | "div" => "÷",
        "deg" => "°",
        "plusmn" | "pm" => "±",
        "minus" => "−",
        "le" => "≤",
        "ge" => "≥",
        "ne" => "≠",
        "asymp" | "ap" => "≈",
        "infin" => "∞",
        "larr" | "leftarrow" => "←",
        "rarr" | "rightarrow" => "→",
        "uarr" | "uparrow" => "↑",
        "darr" | "downarrow" => "↓",
        "harr" | "leftrightarrow" => "↔",
        "rArr" | "Rightarrow" | "Implies" => "⇒",
        "lArr" | "Leftarrow" => "⇐",
        "para" => "¶",
        "sect" => "§",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        "cent" => "¢",
        "frac12" | "half" => "½",
        "frac14" => "¼",
        "frac34" => "¾",
        "sup2" => "²",
        "sup3" => "³",
        "micro" => "µ",
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "pi" => "π",
        "sigma" => "σ",
        "lambda" => "λ",
        "mu" => "μ",
        "theta" => "θ",
        "omega" => "ω",
        "Delta" => "Δ",
        "Sigma" => "Σ",
        "Omega" => "Ω",
        "check" | "checkmark" => "✓",
        "cross" => "✗",
        "star" => "☆",
        "starf" | "bigstar" => "★",
        "hearts" | "heartsuit" => "♥",
        "zwj" => "\u{200d}",
        "zwnj" => "\u{200c}",
        "shy" => "\u{ad}",
        "ensp" => "\u{2002}",
        "emsp" => "\u{2003}",
        "thinsp" | "ThinSpace" => "\u{2009}",
        "num" => "#",
        "excl" => "!",
        "dollar" => "$",
        "percnt" => "%",
        "ast" | "midast" => "*",
        "lpar" => "(",
        "rpar" => ")",
        "lsqb" | "lbrack" => "[",
        "rsqb" | "rbrack" => "]",
        "lcub" | "lbrace" => "{",
        "rcub" | "rbrace" => "}",
        "verbar" | "vert" | "VerticalLine" => "|",
        "bsol" => "\\",
        "sol" => "/",
        "colon" => ":",
        "semi" => ";",
        "equals" => "=",
        "quest" => "?",
        "commat" => "@",
        "grave" | "DiacriticalGrave" => "`",
        "Hat" => "^",
        "lowbar" | "UnderBar" => "_",
        "tilde" | "DiacriticalTilde" => "˜",
        _ => return None,
    })
}

/// `<br>`, `<br/>` or `<br />` (any case) at `at`: where it ends.
fn raw_break(units: &[u16], at: usize) -> Option<usize> {
    let name = units.get(at + 1..at + 3)?;
    if !(name[0] | 0x20 == u16::from(b'b') && name[1] | 0x20 == u16::from(b'r')) {
        return None;
    }
    let mut end = at + 3;
    while units.get(end) == Some(&0x20) {
        end += 1;
    }
    if units.get(end) == Some(&u16::from(b'/')) {
        end += 1;
    } else if end > at + 3 {
        return None;
    }
    (units.get(end) == Some(&u16::from(b'>'))).then_some(end + 1)
}

/// An autolink at `at` (`<scheme:...>` or `<email>`): where it ends, its content's
/// range, and whether it is an email.
fn autolink(units: &[u16], at: usize) -> Option<(usize, usize, usize, bool)> {
    let start = at + 1;
    let close = start + units[start..].iter().position(|&u| u == u16::from(b'>'))?;
    let content = &units[start..close];
    let byte = |u: u16| if u < 0x80 { u as u8 } else { 0x80 };
    // A scheme: a letter, then 1-31 letters, digits, `+`, `.` or `-`, then `:`.
    let scheme = content.iter().position(|&u| u == u16::from(b':')).filter(|&colon| {
        (2..=32).contains(&colon)
            && byte(content[0]).is_ascii_alphabetic()
            && content[1..colon].iter().all(|&u| matches!(byte(u), b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'.' | b'-'))
    });
    if scheme.is_some()
        && content
            .iter()
            .all(|&u| u > 0x20 && u != u16::from(b'<') && u != 0x7f)
    {
        return Some((close + 1, start, close, false));
    }
    // An email: atext before `@`, then labels of letters, digits and `-` (not first or
    // last, at most 63) joined by `.`.
    let at_sign = content.iter().position(|&u| u == u16::from(b'@'))?;
    let local = &content[..at_sign];
    if local.is_empty() || !local.iter().all(|&u| matches!(byte(u), b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'/' | b'=' | b'?' | b'^' | b'_' | b'`' | b'{' | b'|' | b'}' | b'~' | b'-')) {
        return None;
    }
    let domain = &content[at_sign + 1..];
    let labels_ok = !domain.is_empty()
        && domain.split(|&u| u == u16::from(b'.')).all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .iter()
                    .all(|&u| byte(u).is_ascii_alphanumeric() || u == u16::from(b'-'))
                && label[0] != u16::from(b'-')
                && label[label.len() - 1] != u16::from(b'-')
        });
    labels_ok.then_some((close + 1, start, close, true))
}

/// micromark's limit on nested parentheses in a destination.
const MAX_PARENS: usize = 32;

/// An inline link's `(destination "title")` at `at`: where it ends, the destination and
/// the title, unescaped and with character references decoded.
fn inline_link(units: &[u16], at: usize) -> Option<(usize, String, Option<String>)> {
    let unit = |at: usize| units.get(at).copied();
    if unit(at) != Some(u16::from(b'(')) {
        return None;
    }
    let mut at = at + 1;
    let separator = |at: &mut usize| {
        while matches!(unit(*at), Some(0x20 | 0x09)) {
            *at += 1;
        }
        if unit(*at) == Some(0x0A) {
            *at += 1;
            while matches!(unit(*at), Some(0x20 | 0x09)) {
                *at += 1;
            }
        }
    };
    separator(&mut at);
    let punct = |u: Option<u16>| u.is_some_and(|u| u < 0x80 && (u as u8).is_ascii_punctuation());
    let (raw_start, raw_end);
    if unit(at) == Some(u16::from(b'<')) {
        let mut i = at + 1;
        loop {
            match unit(i)? {
                0x0A | 0x0D | 0x3C => return None,
                0x3E => break,
                0x5C if punct(unit(i + 1)) => i += 1,
                _ => {}
            }
            i += 1;
        }
        (raw_start, raw_end) = (at + 1, i);
        at = i + 1;
    } else {
        let mut i = at;
        let mut nest = 0;
        while let Some(u) = unit(i) {
            match u {
                0..=0x20 => break,
                0x28 => {
                    if nest > MAX_PARENS {
                        return None;
                    }
                    nest += 1;
                }
                0x29 => {
                    if nest == 0 {
                        break;
                    }
                    nest -= 1;
                }
                0x5C if punct(unit(i + 1)) => i += 1,
                _ => {}
            }
            i += 1;
        }
        if nest != 0 {
            return None;
        }
        (raw_start, raw_end) = (at, i);
        at = i;
    }
    separator(&mut at);
    // A title: `"..."`, `'...'` or `(...)`.
    let mut title = None;
    if let Some(open) = unit(at).filter(|&u| matches!(u, 0x22 | 0x27 | 0x28)) {
        let close = if open == 0x28 { 0x29 } else { open };
        let mut i = at + 1;
        loop {
            let u = unit(i)?;
            if u == close {
                break;
            }
            if u == open {
                return None;
            }
            if u == 0x5C && punct(unit(i + 1)) {
                i += 1;
            }
            i += 1;
        }
        title = Some((at + 1, i));
        at = i + 1;
        separator(&mut at);
    }
    if unit(at) != Some(0x29) {
        return None;
    }
    let title = title.map(|(start, end)| unescape(&units[start..end]));
    Some((at + 1, unescape(&units[raw_start..raw_end]), title))
}

/// Backslash escapes removed and character references decoded.
pub(crate) fn unescape(units: &[u16]) -> String {
    let mut out = String::new();
    let mut at = 0;
    while at < units.len() {
        let u = units[at];
        if u == 0x5C
            && units
                .get(at + 1)
                .is_some_and(|&n| n < 0x80 && (n as u8).is_ascii_punctuation())
        {
            out.push(char::from(units[at + 1] as u8));
            at += 2;
        } else if u == 0x26
            && let Some((end, decoded)) = entity(units, at)
        {
            out.push_str(&decoded);
            at = end;
        } else {
            let end = units[at..]
                .iter()
                .position(|&u| u == 0x5C || u == 0x26)
                .map_or(units.len(), |n| at + n.max(1));
            out.push_str(&String::from_utf16_lossy(&units[at..end]));
            at = end;
        }
    }
    out
}

/// Whether `www` (any case) ends at `at`.
/// micromark's `gfmAtext`.
fn atext(unit: u16) -> bool {
    unit < 0x80
        && matches!(unit as u8, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.' | b'_')
}

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
fn code_content(out: &mut Out, raw: &[u16], after_break: bool, indent: usize) {
    // Across lines: the text without container indents, line endings as spaces (rare;
    // a copy).
    let joined: Vec<u16>;
    let raw = if raw.contains(&0x0A) {
        let mut text = Vec::with_capacity(raw.len());
        for (index, line) in raw.split(|&u| u == 0x0A).enumerate() {
            if index > 0 {
                text.push(0x20);
            }
            let skip = if index > 0 {
                line.iter().take(indent).take_while(|&&u| u == 0x20).count()
            } else {
                0
            };
            text.extend_from_slice(&line[skip..]);
        }
        joined = text;
        &joined[..]
    } else {
        raw
    };
    let space = |u: u16| u == 0x20;
    let strip = raw.len() >= 2
        && space(raw[0])
        && space(raw[raw.len() - 1])
        && raw.iter().any(|&u| !space(u));
    let mut raw = if strip { &raw[1..raw.len() - 1] } else { raw };
    if after_break {
        let lead = raw.iter().take_while(|&&u| space(u)).count();
        raw = &raw[lead..];
    }
    out.text(raw);
}

#[cfg(test)]
mod tests {
    /// Each common reference decodes as HTML's table has it.
    #[test]
    fn common_entities_are_html_entities() {
        let names = include_str!("inline.rs")
            .split("fn common_entity")
            .nth(1)
            .unwrap()
            .split("_ => return None")
            .next()
            .unwrap();
        let mut checked = 0;
        for line in names.lines().filter(|line| line.contains("=>")) {
            let (names, _) = line.split_once("=>").unwrap();
            for name in names
                .split('|')
                .map(|name| name.trim().trim_matches('"'))
                .filter(|name| !name.is_empty())
            {
                assert_eq!(
                    super::common_entity(name),
                    rill_cmark::named_entity(name),
                    "&{name};"
                );
                checked += 1;
            }
        }
        assert!(checked > 100, "{checked}");
    }
}
