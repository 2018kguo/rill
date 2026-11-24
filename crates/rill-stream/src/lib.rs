//! Streamdown's DOM as HTML, written in one pass over the Markdown with no syntax trees.
//!
//! The input is the text Streamdown renders (after its streaming repair). The output is
//! the HTML of the children of Streamdown's root `<div>`: what its components make of
//! each block, parsed by the browser into the same DOM. Anything this does not reproduce
//! exactly returns [`Fallback`], and the caller renders the text the exact way.

mod block;
mod citations;
mod diff;
mod inline;
mod marked;
mod session;
mod simd;
pub mod tokens;

pub use session::{Outcome, Session};

use inline::Inline;
use tokens::Out;

/// The text holds something this engine does not render exactly (yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fallback;

pub type Result<T = ()> = core::result::Result<T, Fallback>;

/// Streamdown's DOM for `source` (one whole, finished document), as HTML appended to
/// `out`.
pub fn render(source: &str, out: &mut String) -> Result {
    let mut tokens = Out::default();
    let units: Vec<u16> = source.encode_utf16().collect();
    render_split(&units, &mut tokens, &mut Inline::default())?;
    tokens::to_html(
        &tokens.words,
        &tokens.text,
        &tokens.origins,
        &units,
        &tokens.strings.borrow(),
        out,
    );
    Ok(())
}

/// Where Streamdown's block split cuts `source` (byte offsets), for tests.
#[doc(hidden)]
pub fn block_cuts(source: &str) -> Vec<usize> {
    let units: Vec<u16> = source.encode_utf16().collect();
    let cuts = if marked::footnotes(&units) {
        Vec::new()
    } else {
        marked::cuts(&units).ok().flatten().unwrap_or_default()
    };
    // UTF-16 offsets of line starts to byte offsets.
    cuts.iter()
        .map(|&cut| String::from_utf16_lossy(&units[..cut]).len())
        .collect()
}

/// Where the last open block starts (text appended to the source can change only that
/// block and what follows): in the source, in the tokens, in their text, and the blocks
/// (root nodes) before it.
#[derive(Clone, Copy, Default)]
pub(crate) struct Split {
    pub source: usize,
    pub words: usize,
    pub text: usize,
    pub nodes: u32,
    /// The block opened here is a list.
    pub list: bool,
    /// Where its first line ends, when the block before it ended because of what that
    /// line is (no blank line parts them); else 0.
    pub decides: usize,
}

/// Streamdown's DOM for `source` (UTF-16) as tokens appended to `out` (see [`tokens`]),
/// and where its last open block starts.
pub(crate) fn render_split(source: &[u16], out: &mut Out, inline: &mut Inline) -> Result<Split> {
    render_split_at(source, 0, 0, usize::MAX, out, inline)
}

/// [`render_split`] for a `source` that starts at `at` in the whole text (the text
/// tokens' origins count from there), of which `..stable` is as it was for the last call
/// (the inline renderer resumes where that holds).
///
/// From `agree` on, the text is not the text as given but the repair's (its closers):
/// no block is final on the strength of a line holding any of it.
pub(crate) fn render_split_at(
    source: &[u16],
    at: usize,
    stable: usize,
    agree: usize,
    out: &mut Out,
    inline: &mut Inline,
) -> Result<Split> {
    out.set_source(source, at, agree);
    if source
        .iter()
        .fold(false, |any, &unit| any | matches!(unit, 0x0D | 0x09 | 0))
    {
        // ponytail: CR, tabs (column math) and NUL take the exact path for now.
        return Err(Fallback);
    }
    let base = Split {
        source: 0,
        words: out.words.len(),
        text: out.text.len(),
        nodes: 0,
        list: false,
        decides: 0,
    };
    let mut splits = Vec::new();
    out.flipped = false;
    block::render(source, out, inline, stable, &mut splits)?;
    if out.flipped {
        // The text is one block now, or no longer (the caller starts over if it rendered
        // from later than its start).
        out.words.truncate(base.words);
        out.origins.truncate(base.words);
        out.text.truncate(base.text);
        out.options.whole = !out.options.whole;
        splits.clear();
        out.flipped = false;
        block::render(source, out, inline, stable, &mut splits)?;
        if out.flipped {
            return Err(Fallback); // marked and micromark disagree on a definition
        }
        out.flipped = true;
    }
    // The block `back` blocks before the last (`base` past the last four).
    let back_of = |back: usize| {
        if back >= splits.len().min(4) {
            base
        } else {
            splits[splits.len() - 1 - back]
        }
    };
    // A block opened on the unfinished last line may still turn into a continuation of
    // the block before it (`***` growing into `***a`, a lazy line): that one stays open
    // too, unless a blank line parts them and it is not a list (which takes items after
    // blank lines).
    let last = back_of(0).source;
    let last_parted = parted(source, last);
    let mut back = usize::from(
        source.last() != Some(&NL)
            && !source[last..].contains(&NL)
            && (!last_parted || back_of(1).list),
    );
    // Lines from the one holding the repair's first unit on may still change what they
    // are: blocks opened on them keep the block before open.
    let repaired = agree.saturating_sub(at);
    if repaired < source.len() {
        let limit = source[..repaired]
            .iter()
            .rposition(|&unit| unit == NL)
            .map_or(0, |at| at + 1);
        while back < 4 && back_of(back).source >= limit && back_of(back).source > 0 {
            back += 1;
        }
    }
    // A paragraph ends before a line that a table delimiter row follows (`marked`'s
    // lookahead), so a line that may still change into or out of one (the unfinished
    // last line, if it could be a delimiter row so far; a repaired one) may move the cut
    // on the line before it: blocks from there keep the block before open.
    let line_start = |at: usize| {
        source[..at]
            .iter()
            .rposition(|&unit| unit == NL)
            .map_or(0, |at| at + 1)
    };
    let last_line = line_start(source.len());
    let mut changing = usize::MAX;
    if source.last() != Some(&NL)
        && source[last_line..]
            .iter()
            .all(|&unit| matches!(ascii(unit), b' ' | b':' | b'-' | b'|'))
    {
        changing = last_line;
    }
    if repaired < source.len() {
        changing = changing.min(line_start(repaired));
    }
    if changing != usize::MAX && changing > 0 {
        let limit = line_start(changing - 1);
        while back < 4 && back_of(back).source >= limit && back_of(back).source > 0 {
            back += 1;
        }
    }
    // `marked`'s lheading: a setext underline still to come makes one heading token of the
    // lines back to the last one it does not run across, whatever blocks they are now.
    let mut chain = last_line;
    while chain > 0 {
        let previous = line_start(chain - 1);
        if marked::setext_stop(&source[previous..chain - 1]) {
            break;
        }
        chain = previous;
    }
    while back < 4 && back_of(back).source > chain {
        back += 1;
    }
    let mut split = back_of(back);
    if split.source > 0 && !parted(source, split.source) {
        // The block's first line decides the cut, and the line after it too when that
        // makes the first a table's header.
        let first = newline(source, split.source);
        split.decides = if first < source.len() {
            newline(source, first + 1) + 1
        } else {
            first + 1
        };
    }
    Ok(split)
}

/// Whether a blank line comes right before the line starting at `at`.
fn parted(source: &[u16], at: usize) -> bool {
    at >= 2 && source[at - 1] == NL && {
        let line = &source[..at - 1];
        let start = line
            .iter()
            .rposition(|&unit| unit == NL)
            .map_or(0, |at| at + 1);
        line[start..].iter().all(|&unit| unit == SP)
    }
}

/// A list item's marker at the start of `rest`: whether ordered, the bullet or delimiter,
/// the start number, the content's offset in `rest` (`None`: the item is empty).
pub(crate) struct Marker {
    ordered: bool,
    marker: u8,
    start: u32,
    content: Option<usize>,
    /// Where the content would start, as a column from the marker.
    width: usize,
}

pub(crate) fn list_marker(rest: &[u16]) -> Result<Option<Marker>> {
    let (ordered, marker, start, after) = match ascii(rest[0]) {
        first @ (b'-' | b'*' | b'+') => (false, first, 0, 1),
        b'0'..=b'9' => {
            let digits = rest
                .iter()
                .take_while(|&&unit| ascii(unit).is_ascii_digit())
                .count();
            match rest.get(digits).map(|&unit| ascii(unit)) {
                Some(delimiter @ (b'.' | b')')) if digits <= 9 => {
                    let start = rest[..digits]
                        .iter()
                        .fold(0, |value, &unit| value * 10 + u32::from(unit - 0x30));
                    (true, delimiter, start, digits + 1)
                }
                _ => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    let spaces = rest[after..].iter().take_while(|&&unit| unit == SP).count();
    if after + spaces == rest.len() {
        return Ok(Some(Marker {
            ordered,
            marker,
            start,
            content: None,
            width: after + 1,
        }));
    }
    if spaces == 0 {
        return Ok(None);
    }
    if spaces > 4 {
        return Err(Fallback); // indented code in the item
    }
    Ok(Some(Marker {
        ordered,
        marker,
        start,
        content: Some(after + spaces),
        width: after + spaces,
    }))
}

/// Whether a line starting with `[` may start a link definition (`[label]:`): its
/// label closes on the line with `]:`, or not on the line at all.
pub(crate) fn may_define(line: &[u16]) -> bool {
    let mut at = 1;
    while at < line.len() {
        match line[at] {
            0x5C => at += 1,
            0x5B => return false,
            0x5D => return line.get(at + 1) == Some(&u16::from(b':')),
            _ => {}
        }
        at += 1;
    }
    true
}

pub(crate) const NL: u16 = b'\n' as u16;
pub(crate) const SP: u16 = b' ' as u16;

/// A unit as an ASCII byte, or 0x80 (no markup) when it is not ASCII.
#[inline(always)]
pub(crate) fn ascii(unit: u16) -> u8 {
    if unit < 0x80 { unit as u8 } else { 0x80 }
}

/// How many units `text` starts with that equal `unit`.
pub(crate) fn run(text: &[u16], unit: u16) -> usize {
    text.iter().take_while(|&&u| u == unit).count()
}

pub(crate) fn newline(text: &[u16], from: usize) -> usize {
    simd::find_unit(&text[from..], NL).map_or(text.len(), |at| from + at)
}

fn trim_end_spaces(text: &[u16]) -> &[u16] {
    let end = text
        .iter()
        .rposition(|&unit| unit != SP)
        .map_or(0, |at| at + 1);
    &text[..end]
}

/// An ATX heading's level and content (`rest` starts at the first `#`).
pub(crate) fn atx(rest: &[u16]) -> Option<(u8, &[u16])> {
    let level = run(rest, u16::from(b'#'));
    if level > 6 {
        return None;
    }
    let after = &rest[level..];
    if !(after.is_empty() || after[0] == SP) {
        return None;
    }
    let mut content = trim_end_spaces(&after[run(after, SP)..]);
    // A closing sequence: `#`s alone, or after a space.
    let hashes = content
        .iter()
        .rev()
        .take_while(|&&unit| unit == u16::from(b'#'))
        .count();
    if hashes == content.len() {
        content = &[];
    } else if hashes > 0 && content[content.len() - hashes - 1] == SP {
        content = trim_end_spaces(&content[..content.len() - hashes]);
    }
    Some((level as u8, content))
}

/// A setext underline's heading level: `=`s (1) or `-`s (2), then only spaces.
pub(crate) fn setext(rest: &[u16]) -> Option<u8> {
    let level = match ascii(*rest.first()?) {
        b'=' => 1,
        b'-' => 2,
        _ => return None,
    };
    trim_end_spaces(rest)
        .iter()
        .all(|&unit| unit == rest[0])
        .then_some(level)
}

/// Three or more of one of `*-_`, with only spaces between.
pub(crate) fn thematic(rest: &[u16]) -> bool {
    let marker = rest[0];
    let mut count = 0;
    for &unit in rest {
        if unit == marker {
            count += 1;
        } else if unit != SP {
            return false;
        }
    }
    count >= 3
}

#[cfg(feature = "wasm")]
pub mod wasm {
    use wasm_bindgen::prelude::*;

    /// Streamdown's DOM for `source`, or `undefined` to render it the exact way.
    #[cfg(feature = "probe")]
    #[wasm_bindgen(js_name = renderStream)]
    pub fn render_stream(source: &str) -> Option<String> {
        let mut out = String::with_capacity(source.len() * 2);
        super::render(source, &mut out).ok().map(|()| out)
    }

    /// The open tags of the token stream's tag ids (see [`crate::tokens`]), one a line.
    #[wasm_bindgen(js_name = streamTags)]
    pub fn stream_tags() -> String {
        crate::tokens::TAGS.join("\n")
    }

    /// The module's memory.
    #[wasm_bindgen(js_name = streamMemory)]
    pub fn memory() -> JsValue {
        wasm_bindgen::memory()
    }

    /// A streaming answer (see [`crate::Session`]).
    #[wasm_bindgen]
    pub struct Stream {
        session: crate::Session,
        input: Vec<u16>,
        /// After each update: the outcome; the edit's opcodes, as a `u32` index and count;
        /// the rendering's tokens, the same; its text, as a `u16` index and count; the
        /// first text unit the edit refers to; the root nodes that became final; the
        /// tokens' origins, as a `u32` index (one a token); how far the rendered text
        /// equals the text as given; how many strings are interned.
        header: [u32; 12],
    }

    impl Default for Stream {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Stream {
        /// A page's own link rewriting (see [`crate::tokens::Out::href`]), for a page's
        /// build of this module to set.
        pub fn set_href(&mut self, href: crate::tokens::Href) {
            self.session.set_href(href);
        }
    }

    #[wasm_bindgen]
    impl Stream {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self {
                session: crate::Session::default(),
                input: Vec::new(),
                header: [0; 12],
            }
        }
        /// Streamdown's `controls` and `isAnimating` (before the first update).
        #[wasm_bindgen(js_name = setOptions)]
        pub fn set_options(&mut self, controls: bool, animating: bool) {
            let options = crate::tokens::Options {
                controls,
                animating,
                ..self.session.options()
            };
            self.session.set_options(options);
        }
        /// Streamdown's `lineNumbers` and `codeBlockMaxHeight` (0: none), and `dir` (0:
        /// none, 1: `ltr`, 2: `rtl`), before the first update.
        /// The tags a page's `components` override (`overrides::*` bits), before the
        /// first update.
        #[wasm_bindgen(js_name = setOverrides)]
        pub fn set_overrides(&mut self, overrides: u8) {
            let options = crate::tokens::Options {
                overrides,
                ..self.session.options()
            };
            self.session.set_options(options);
        }
        /// A page's own pipeline (`setup::*` bits), before the first update.
        #[wasm_bindgen(js_name = setSetup)]
        pub fn set_setup(&mut self, setup: u8) {
            let options = crate::tokens::Options {
                setup,
                ..self.session.options()
            };
            self.session.set_options(options);
        }
        #[wasm_bindgen(js_name = setLayout)]
        pub fn set_layout(&mut self, line_numbers: bool, code_max_height: u32, dir: u8) {
            let options = crate::tokens::Options {
                line_numbers,
                code_max_height,
                dir,
                ..self.session.options()
            };
            self.session.set_options(options);
        }
        #[cfg(feature = "probe")]
        /// 0: patch, 1: reset, 2: fall back (see [`crate::Outcome`]).
        pub fn update(&mut self, appended: &str, reset: bool) -> u32 {
            let outcome = self.session.update(appended, reset);
            self.fill(outcome)
        }
        /// An input buffer of at least `length` UTF-16 units in [`memory`], as a `u16`
        /// index; it stays put until a larger one is asked for.
        #[wasm_bindgen(js_name = inputAt)]
        pub fn input_at(&mut self, length: usize) -> usize {
            if self.input.len() < length {
                self.input = vec![0; length];
            }
            self.input.as_ptr() as usize / 2
        }
        /// [`Stream::update`] with the text written to the input buffer's first `length`
        /// units: no string crosses into the module, nor is converted.
        #[wasm_bindgen(js_name = updateInput)]
        pub fn update_input(&mut self, length: usize, reset: bool) -> u32 {
            let outcome = self.session.update_units(&self.input[..length], reset);
            self.fill(outcome)
        }
        /// See [`crate::Session::repaired`].
        pub fn repaired(&self) -> bool {
            self.session.repaired()
        }
        /// See [`crate::Session::whole`].
        pub fn whole(&self) -> bool {
            self.session.whole()
        }
        /// Where the header is in [`memory`], as a `u32` index (it stays put).
        #[wasm_bindgen(js_name = headerAt)]
        pub fn header_at(&self) -> usize {
            self.header.as_ptr() as usize / 4
        }
        /// The interned strings from `from` on, each ended by a NUL.
        pub fn strings(&self, from: usize) -> String {
            let strings = self.session.strings();
            let mut joined = String::new();
            for string in &strings.list[from..] {
                joined.push_str(string);
                joined.push('\0');
            }
            joined
        }
        #[cfg(feature = "probe")]
        /// The last update's whole HTML.
        #[wasm_bindgen(getter)]
        pub fn html(&self) -> String {
            self.session.html()
        }
    }

    impl Stream {
        fn fill(&mut self, outcome: crate::Outcome) -> u32 {
            let program = &self.session.program;
            let rendering = self.session.rendering();
            let text_from = if outcome == crate::Outcome::Patch {
                program.text_from.min(rendering.text.len() as u32)
            } else {
                0
            };
            self.header = [
                outcome as u32,
                program.ops.as_ptr() as u32 / 4,
                program.ops.len() as u32,
                rendering.words.as_ptr() as u32 / 4,
                rendering.words.len() as u32,
                rendering.text.as_ptr() as u32 / 2,
                rendering.text.len() as u32,
                text_from,
                self.session.final_nodes,
                rendering.origins.as_ptr() as u32 / 4,
                self.session.agree as u32,
                self.session.strings().list.len() as u32,
            ];
            outcome as u32
        }
    }

    /// Bench probe: the HTML's length, without returning it.
    #[cfg(feature = "probe")]
    #[wasm_bindgen(js_name = renderStreamLength)]
    pub fn render_stream_length(source: &str) -> usize {
        let mut out = String::with_capacity(source.len() * 2);
        super::render(source, &mut out).map_or(0, |()| out.len())
    }
}
