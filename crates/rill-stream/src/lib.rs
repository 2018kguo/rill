//! Streamdown's DOM as HTML, written in one pass over the Markdown with no syntax trees.
//!
//! The input is the text Streamdown renders (after its streaming repair). The output is
//! the HTML of the children of Streamdown's root `<div>`: what its components make of
//! each block, parsed by the browser into the same DOM. Anything this does not reproduce
//! exactly returns [`Fallback`], and the caller renders the text the exact way.

mod diff;
mod inline;
mod session;
mod simd;
pub mod tokens;

pub use session::{Outcome, Session};

use inline::Inline;
use tokens::{Out, tag};

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
    tokens::to_html(&tokens.words, &tokens.text, &tokens.origins, &units, out);
    Ok(())
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
    if source.iter().fold(false, |any, &unit| any | matches!(unit, 0x0D | 0x09 | 0x7C | 0)) {
        // ponytail: CR, tabs (column math), NUL and tables take the exact path for now.
        return Err(Fallback);
    }
    let base = Split { source: 0, words: out.words.len(), text: out.text.len(), nodes: 0, list: false, decides: 0 };
    let mut blocks = Blocks { source, out, inline, stable, para: None, list: None, last: base, before: base, opened: 0, older: [base; 2] };
    let mut start = 0;
    while start <= source.len() {
        let end = newline(source, start);
        blocks.line(start, &source[start..end])?;
        start = end + 1;
    }
    blocks.close_paragraph()?;
    blocks.close_list()?;
    // A block opened on the unfinished last line may still turn into a continuation of
    // the block before it (`***` growing into `***a`, a lazy line): that one stays open
    // too, unless a blank line parts them and it is not a list (which takes items after
    // blank lines).
    let last = blocks.last.source;
    let last_parted = parted(source, last);
    let mut back = usize::from(
        source.last() != Some(&NL) && !source[last..].contains(&NL) && !(last_parted && !blocks.before.list),
    );
    // Lines from the one holding the repair's first unit on may still change what they
    // are: blocks opened on them keep the block before open.
    let repaired = agree.saturating_sub(at);
    if repaired < source.len() {
        let limit = source[..repaired].iter().rposition(|&unit| unit == NL).map_or(0, |at| at + 1);
        while back < 4 && blocks.back(back, base).source >= limit && blocks.back(back, base).source > 0 {
            back += 1;
        }
    }
    let mut split = blocks.back(back, base);
    if split.source > 0 && !parted(source, split.source) {
        split.decides = newline(source, split.source) + 1;
    }
    Ok(split)
}

/// Whether a blank line comes right before the line starting at `at`.
fn parted(source: &[u16], at: usize) -> bool {
    at >= 2 && source[at - 1] == NL && {
        let line = &source[..at - 1];
        let start = line.iter().rposition(|&unit| unit == NL).map_or(0, |at| at + 1);
        line[start..].iter().all(|&unit| unit == SP)
    }
}

struct Blocks<'a, 'o> {
    source: &'a [u16],
    out: &'o mut Out,
    inline: &'o mut Inline,
    /// The whole text is as it was for the last render up to here.
    stable: usize,
    /// The open paragraph: from its first content byte to the end of its last line.
    para: Option<(usize, usize)>,
    /// The open list.
    list: Option<List>,
    /// Where the last block opened (all output before it belongs to closed blocks).
    last: Split,
    /// Where the block before it opened.
    before: Split,
    /// Blocks opened so far.
    opened: u32,
    /// Where the two blocks before `before` opened.
    older: [Split; 2],
}

/// A tight list of one-paragraph items.
struct List {
    ordered: bool,
    /// The bullet, or the ordered list's delimiter.
    marker: u8,
    start: u32,
    /// Each item's content (`None`: empty so far), from its first content byte to the
    /// end of its last line.
    items: Vec<Option<(usize, usize)>>,
    /// The column the last item's content starts at.
    column: usize,
    /// Blank lines since the last item's last line.
    blank: u32,
}

/// A list item's marker at the start of `rest`: whether ordered, the bullet or delimiter,
/// the start number, the content's offset in `rest` (`None`: the item is empty).
struct Marker {
    ordered: bool,
    marker: u8,
    start: u32,
    content: Option<usize>,
    /// Where the content would start, as a column from the marker.
    width: usize,
}

fn list_marker(rest: &[u16]) -> Result<Option<Marker>> {
    let (ordered, marker, start, after) = match ascii(rest[0]) {
        first @ (b'-' | b'*' | b'+') => (false, first, 0, 1),
        b'0'..=b'9' => {
            let digits = rest.iter().take_while(|&&unit| ascii(unit).is_ascii_digit()).count();
            match rest.get(digits).map(|&unit| ascii(unit)) {
                Some(delimiter @ (b'.' | b')')) if digits <= 9 => {
                    let start = rest[..digits].iter().fold(0, |value, &unit| value * 10 + u32::from(unit - 0x30));
                    (true, delimiter, start, digits + 1)
                }
                _ => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    let spaces = rest[after..].iter().take_while(|&&unit| unit == SP).count();
    if after + spaces == rest.len() {
        return Ok(Some(Marker { ordered, marker, start, content: None, width: after + 1 }));
    }
    if spaces == 0 {
        return Ok(None);
    }
    if spaces > 4 {
        return Err(Fallback); // indented code in the item
    }
    Ok(Some(Marker { ordered, marker, start, content: Some(after + spaces), width: after + spaces }))
}

/// Whether `rest` (a line without its indent) starts a block other than a paragraph.
fn starts_block(rest: &[u16]) -> Result<bool> {
    let first = ascii(rest[0]);
    Ok(match first {
        b'#' => atx(rest).is_some(),
        b'>' | b'<' => true,
        b'`' | b'~' => run(rest, rest[0]) >= 3,
        b'*' | b'-' | b'_' if thematic(rest) => true,
        _ => list_marker(rest)?.is_some(),
    })
}

const NL: u16 = b'\n' as u16;
const SP: u16 = b' ' as u16;

/// A unit as an ASCII byte, or 0x80 (no markup) when it is not ASCII.
#[inline(always)]
fn ascii(unit: u16) -> u8 {
    if unit < 0x80 { unit as u8 } else { 0x80 }
}

/// How many units `text` starts with that equal `unit`.
fn run(text: &[u16], unit: u16) -> usize {
    text.iter().take_while(|&&u| u == unit).count()
}

fn newline(text: &[u16], from: usize) -> usize {
    simd::find_unit(&text[from..], NL).map_or(text.len(), |at| from + at)
}

/// Leading spaces (tabs were ruled out).
fn indent(line: &[u16]) -> usize {
    run(line, SP)
}

fn trim_end_spaces(text: &[u16]) -> &[u16] {
    let end = text.iter().rposition(|&unit| unit != SP).map_or(0, |at| at + 1);
    &text[..end]
}

impl<'a> Blocks<'a, '_> {
    /// Where the block `back` blocks before the last opened (`base` past the ones kept).
    fn back(&self, back: usize, base: Split) -> Split {
        let opened = self.opened as usize;
        if back >= opened.min(4) {
            return base;
        }
        [self.last, self.before, self.older[1], self.older[0]][back]
    }

    fn line(&mut self, offset: usize, line: &'a [u16]) -> Result {
        let spaces = indent(line);
        let rest = &line[spaces..];
        if rest.is_empty() {
            if let Some(list) = &mut self.list {
                list.blank += 1;
            }
            return self.close_paragraph();
        }
        if self.list.is_some() && self.list_line(offset, spaces, rest)? {
            return Ok(());
        }
        if spaces < 4 && let Some(level) = setext(rest) && self.para.is_some() {
            return self.close_paragraph_as(Some(level));
        }
        if spaces >= 4 {
            let Some(para) = &mut self.para else {
                return Err(Fallback); // indented code
            };
            para.1 = offset + line.len();
            return Ok(());
        }
        let first = ascii(rest[0]);
        match first {
            b'#' => {
                if let Some((level, content)) = atx(rest) {
                    self.close_paragraph()?;
                    self.close_list()?;
                    self.open_block(offset);
                    return self.heading(level, content);
                }
            }
            b'*' | b'-' | b'_' if thematic(rest) => {
                self.close_paragraph()?;
                self.close_list()?;
                self.open_block(offset);
                self.out.void(tag::HR);
                return Ok(());
            }
            // Blocks not handled yet: quotes, lists, fences, HTML, definitions, setext.
            b'>' | b'<' | b'[' => return Err(Fallback),
            b'`' | b'~' if run(rest, rest[0]) >= 3 => {
                return Err(Fallback);
            }
            _ => {
                if let Some(marker) = list_marker(rest)? {
                    // An empty item, or an ordered one not starting at 1, does not
                    // interrupt a paragraph.
                    let interrupts = marker.content.is_some() && (!marker.ordered || marker.start == 1);
                    if self.para.is_none() || interrupts {
                        self.close_paragraph()?;
                        return self.open_item(offset, spaces, rest, marker);
                    }
                }
            }
        }
        self.close_list()?;
        let end = offset + line.len();
        match &mut self.para {
            Some(para) => para.1 = end,
            None => {
                self.open_block(offset);
                self.para = Some((offset + spaces, end));
            }
        }
        Ok(())
    }

    fn open_block(&mut self, offset: usize) {
        self.older = [self.older[1], self.before];
        self.before = self.last;
        self.last = Split { source: offset, words: self.out.words.len(), text: self.out.text.len(), nodes: self.opened, list: false, decides: 0 };
        self.opened += 1;
    }

    /// A line while a list is open: `true` when the list took it.
    fn list_line(&mut self, offset: usize, spaces: usize, rest: &'a [u16]) -> Result<bool> {
        let list = self.list.as_mut().expect("open list");
        if spaces < list.column && matches!(ascii(rest[0]), b'*' | b'-' | b'_') && thematic(rest) {
            // A thematic break wins over an item.
            self.close_list()?;
            return Ok(false);
        }
        let marker = list_marker(rest)?;
        if list.blank > 0 {
            // An empty item two blank lines back ends the list (Streamdown's blocks).
            let ended = list.blank >= 2 && list.items.last().is_some_and(Option::is_none);
            if ended {
                self.close_list()?;
                return Ok(false);
            }
            if spaces >= list.column {
                return Err(Fallback); // a loose item: a block after a blank line inside it
            }
            // Items apart from each other by blank lines stay one tight list.
            if let Some(marker) = marker
                && marker.ordered == list.ordered
                && marker.marker == list.marker
            {
                return self.open_item(offset, spaces, rest, marker).map(|()| true);
            }
            self.close_list()?;
            return Ok(false);
        }
        let end = offset + spaces + rest.len();
        let item = list.items.last_mut().expect("an item");
        if spaces >= list.column {
            // The item's own continuation.
            match item {
                Some(content) => {
                    if starts_block(rest)? || setext(rest).is_some() {
                        return Err(Fallback);
                    }
                    content.1 = end;
                }
                None => {
                    if starts_block(rest)? {
                        return Err(Fallback);
                    }
                    *item = Some((offset + spaces, end));
                }
            }
            return Ok(true);
        }
        if let Some(marker) = marker {
            if marker.ordered == list.ordered && marker.marker == list.marker {
                return self.open_item(offset, spaces, rest, marker).map(|()| true);
            }
            self.close_list()?;
            return Ok(false);
        }
        // A lazy line continues the item's paragraph, unless it starts a block.
        if let Some(content) = item
            && !starts_block(rest)?
            && setext(rest).is_none()
        {
            content.1 = end;
            return Ok(true);
        }
        self.close_list()?;
        Ok(false)
    }

    fn open_item(&mut self, offset: usize, spaces: usize, rest: &[u16], marker: Marker) -> Result {
        let content = marker.content.map(|at| {
            let start = offset + spaces + at;
            (start, offset + spaces + rest.len())
        });
        if let Some(at) = marker.content
            && starts_block(&rest[at..])?
        {
            return Err(Fallback); // a nested block
        }
        let same = self
            .list
            .as_ref()
            .is_some_and(|list| list.ordered == marker.ordered && list.marker == marker.marker);
        if !same {
            self.close_list()?;
            self.open_block(offset);
            self.last.list = true;
            self.list = Some(List {
                ordered: marker.ordered,
                marker: marker.marker,
                start: marker.start,
                items: Vec::new(),
                column: 0,
                blank: 0,
            });
        }
        let list = self.list.as_mut().expect("open list");
        list.blank = 0;
        list.items.push(content);
        list.column = spaces + marker.width;
        Ok(())
    }

    fn close_list(&mut self) -> Result {
        let Some(list) = self.list.take() else {
            return Ok(());
        };
        self.out.open(match (list.ordered, list.start) {
            (false, _) => tag::UL,
            (true, 1) => tag::OL,
            (true, start) => tag::OL_START + start,
        });
        self.out.ascii("\n");
        for item in list.items {
            self.out.open(tag::LI);
            if let Some((start, end)) = item {
                self.inline.render(trim_end_spaces(&self.source[start..end]), self.out, self.stable)?;
            }
            self.out.close();
            self.out.ascii("\n");
        }
        self.out.close();
        Ok(())
    }

    fn heading(&mut self, level: u8, content: &[u16]) -> Result {
        self.out.open(tag::H1 + u32::from(level) - 1);
        self.inline.render(content, self.out, self.stable)?;
        self.out.close();
        Ok(())
    }

    fn close_paragraph(&mut self) -> Result {
        self.close_paragraph_as(None)
    }

    /// Closes the paragraph, as a setext heading of `level` when given.
    fn close_paragraph_as(&mut self, level: Option<u8>) -> Result {
        let Some((start, end)) = self.para.take() else {
            return Ok(());
        };
        let content = trim_end_spaces(&self.source[start..end]);
        match level {
            Some(level) => self.heading(level, content),
            None => {
                self.out.open(tag::P);
                self.inline.render(content, self.out, self.stable)?;
                self.out.close();
                Ok(())
            }
        }
    }
}



/// An ATX heading's level and content (`rest` starts at the first `#`).
fn atx(rest: &[u16]) -> Option<(u8, &[u16])> {
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
    let hashes = content.iter().rev().take_while(|&&unit| unit == u16::from(b'#')).count();
    if hashes == content.len() {
        content = &[];
    } else if hashes > 0 && content[content.len() - hashes - 1] == SP {
        content = trim_end_spaces(&content[..content.len() - hashes]);
    }
    Some((level as u8, content))
}

/// A setext underline's heading level: `=`s (1) or `-`s (2), then only spaces.
fn setext(rest: &[u16]) -> Option<u8> {
    let level = match ascii(*rest.first()?) {
        b'=' => 1,
        b'-' => 2,
        _ => return None,
    };
    trim_end_spaces(rest).iter().all(|&unit| unit == rest[0]).then_some(level)
}

/// Three or more of one of `*-_`, with only spaces between.
fn thematic(rest: &[u16]) -> bool {
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
mod wasm {
    use wasm_bindgen::prelude::*;

    /// Streamdown's DOM for `source`, or `undefined` to render it the exact way.
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
        /// equals the text as given.
        header: [u32; 11],
    }

    #[wasm_bindgen]
    impl Stream {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self { session: crate::Session::default(), input: Vec::new(), header: [0; 11] }
        }
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
        /// Where the header is in [`memory`], as a `u32` index (it stays put).
        #[wasm_bindgen(js_name = headerAt)]
        pub fn header_at(&self) -> usize {
            self.header.as_ptr() as usize / 4
        }
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
            ];
            outcome as u32
        }
    }

    /// Bench probe: the HTML's length, without returning it.
    #[wasm_bindgen(js_name = renderStreamLength)]
    pub fn render_stream_length(source: &str) -> usize {
        let mut out = String::with_capacity(source.len() * 2);
        super::render(source, &mut out).map_or(0, |()| out.len())
    }
}
