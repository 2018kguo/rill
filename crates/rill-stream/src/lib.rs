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

pub use session::{Outcome, Session};

use inline::Inline;

/// The text holds something this engine does not render exactly (yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fallback;

pub type Result<T = ()> = core::result::Result<T, Fallback>;

/// Streamdown's DOM for `source` (one whole, finished document), appended to `out`.
pub fn render(source: &str, out: &mut String) -> Result {
    render_split(source, out).map(|_| ())
}

/// [`render`], and where the last block starts: its offset in `source` and in the HTML
/// appended. Text appended to `source` can change only that block and what follows.
pub fn render_split(source: &str, out: &mut String) -> Result<(usize, usize)> {
    let bytes = source.as_bytes();
    if memchr::memchr3(b'\r', b'\t', b'|', bytes).is_some() || memchr::memchr(0, bytes).is_some() {
        // ponytail: CR, tabs (column math), NUL and tables take the exact path for now.
        return Err(Fallback);
    }
    // Markup is about a third again the text: one allocation up front.
    out.reserve(source.len() + source.len() / 2);
    let base = out.len();
    let mut blocks = Blocks { source, out, inline: Inline::default(), para: None, list: None, last: (0, base), before: (0, base) };
    let mut start = 0;
    while start <= source.len() {
        let end = memchr_newline(bytes, start);
        blocks.line(start, &source[start..end])?;
        start = end + 1;
    }
    blocks.close_paragraph()?;
    blocks.close_list()?;
    // A block opened on the unfinished last line may still turn into a continuation of
    // the block before it (`****` growing into `****#`): that one stays open too.
    let split = if !source.ends_with('\n') && !source[blocks.last.0..].contains('\n') {
        blocks.before
    } else {
        blocks.last
    };
    Ok((split.0, split.1 - base))
}

struct Blocks<'a, 'o> {
    source: &'a str,
    out: &'o mut String,
    inline: Inline,
    /// The open paragraph: from its first content byte to the end of its last line.
    para: Option<(usize, usize)>,
    /// The open list.
    list: Option<List>,
    /// Where the last block opened: its source offset and the output length then (all
    /// HTML before it belongs to blocks that are closed).
    last: (usize, usize),
    /// Where the block before it opened.
    before: (usize, usize),
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

fn list_marker(rest: &str) -> Result<Option<Marker>> {
    let bytes = rest.as_bytes();
    let (ordered, marker, start, after) = match bytes[0] {
        b'-' | b'*' | b'+' => (false, bytes[0], 0, 1),
        b'0'..=b'9' => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            match bytes.get(digits) {
                Some(&delimiter @ (b'.' | b')')) if digits <= 9 => {
                    (true, delimiter, rest[..digits].parse().expect("digits"), digits + 1)
                }
                _ => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    let spaces = bytes[after..].iter().take_while(|&&b| b == b' ').count();
    if after + spaces == bytes.len() {
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
fn starts_block(rest: &str) -> Result<bool> {
    let first = rest.as_bytes()[0];
    Ok(match first {
        b'#' => atx(rest).is_some(),
        b'>' | b'<' => true,
        b'`' | b'~' => rest.bytes().take_while(|&b| b == first).count() >= 3,
        b'*' | b'-' | b'_' if thematic(rest) => true,
        _ => list_marker(rest)?.is_some(),
    })
}

fn memchr_newline(bytes: &[u8], from: usize) -> usize {
    memchr::memchr(b'\n', &bytes[from..]).map_or(bytes.len(), |at| from + at)
}

/// Leading spaces (tabs were ruled out).
fn indent(line: &str) -> usize {
    line.bytes().take_while(|&b| b == b' ').count()
}

impl<'a> Blocks<'a, '_> {
    fn line(&mut self, offset: usize, line: &'a str) -> Result {
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
        let first = rest.as_bytes()[0];
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
                self.out.push_str(r#"<hr class="my-6 border-border" data-streamdown="horizontal-rule">"#);
                return Ok(());
            }
            // Blocks not handled yet: quotes, lists, fences, HTML, definitions, setext.
            b'>' | b'<' | b'[' => return Err(Fallback),
            b'`' | b'~' if rest.bytes().take_while(|&b| b == first).count() >= 3 => {
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
        self.before = self.last;
        self.last = (offset, self.out.len());
    }

    /// A line while a list is open: `true` when the list took it.
    fn list_line(&mut self, offset: usize, spaces: usize, rest: &'a str) -> Result<bool> {
        let list = self.list.as_mut().expect("open list");
        if spaces < list.column && matches!(rest.as_bytes()[0], b'*' | b'-' | b'_') && thematic(rest) {
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

    fn open_item(&mut self, offset: usize, spaces: usize, rest: &str, marker: Marker) -> Result {
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
        let (tag, class, data) = if list.ordered {
            ("ol", "list-inside list-decimal whitespace-normal [li_&amp;]:pl-6", "ordered-list")
        } else {
            ("ul", "list-inside list-disc whitespace-normal [li_&amp;]:pl-6", "unordered-list")
        };
        let out = &mut *self.out;
        out.push('<');
        out.push_str(tag);
        out.push_str(" class=\"");
        out.push_str(class);
        out.push_str("\" data-streamdown=\"");
        out.push_str(data);
        out.push('"');
        if list.ordered && list.start != 1 {
            out.push_str(" start=\"");
            out.push_str(&list.start.to_string());
            out.push('"');
        }
        out.push_str(">\n");
        for item in list.items {
            self.out.push_str(r#"<li class="py-1 [&amp;&gt;p]:inline" data-streamdown="list-item">"#);
            if let Some((start, end)) = item {
                self.inline.render(self.source[start..end].trim_end_matches(' '), self.out)?;
            }
            self.out.push_str("</li>\n");
        }
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push('>');
        Ok(())
    }

    fn heading(&mut self, level: u8, content: &str) -> Result {
        let (class, data) = HEADINGS[usize::from(level - 1)];
        let tag = [b'h', b'0' + level];
        let tag = core::str::from_utf8(&tag).expect("ascii");
        self.out.push('<');
        self.out.push_str(tag);
        self.out.push_str(" class=\"");
        self.out.push_str(class);
        self.out.push_str("\" data-streamdown=\"");
        self.out.push_str(data);
        self.out.push_str("\">");
        self.inline.render(content, self.out)?;
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push('>');
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
        let content = self.source[start..end].trim_end_matches(' ');
        match level {
            Some(level) => self.heading(level, content),
            None => {
                self.out.push_str("<p>");
                self.inline.render(content, self.out)?;
                self.out.push_str("</p>");
                Ok(())
            }
        }
    }
}

const HEADINGS: [(&str, &str); 6] = [
    ("mt-6 mb-2 font-semibold text-3xl", "heading-1"),
    ("mt-6 mb-2 font-semibold text-2xl", "heading-2"),
    ("mt-6 mb-2 font-semibold text-xl", "heading-3"),
    ("mt-6 mb-2 font-semibold text-lg", "heading-4"),
    ("mt-6 mb-2 font-semibold text-base", "heading-5"),
    ("mt-6 mb-2 font-semibold text-sm", "heading-6"),
];

/// An ATX heading's level and content (`rest` starts at the first `#`).
fn atx(rest: &str) -> Option<(u8, &str)> {
    let level = rest.bytes().take_while(|&b| b == b'#').count();
    if level > 6 {
        return None;
    }
    let after = &rest[level..];
    if !(after.is_empty() || after.starts_with(' ')) {
        return None;
    }
    let mut content = after.trim_matches(' ');
    // A closing sequence: `#`s alone, or after a space.
    let hashes = content.bytes().rev().take_while(|&b| b == b'#').count();
    if hashes == content.len() {
        content = "";
    } else if hashes > 0 && content.as_bytes()[content.len() - hashes - 1] == b' ' {
        content = content[..content.len() - hashes].trim_end_matches(' ');
    }
    Some((level as u8, content))
}

/// A setext underline's heading level: `=`s (1) or `-`s (2), then only spaces.
fn setext(rest: &str) -> Option<u8> {
    let marker = *rest.as_bytes().first()?;
    let level = match marker {
        b'=' => 1,
        b'-' => 2,
        _ => return None,
    };
    let trimmed = rest.trim_end_matches(' ');
    trimmed.bytes().all(|b| b == marker).then_some(level)
}

/// Three or more of one of `*-_`, with only spaces between.
fn thematic(rest: &str) -> bool {
    let marker = rest.as_bytes()[0];
    let mut count = 0;
    for b in rest.bytes() {
        if b == marker {
            count += 1;
        } else if b != b' ' {
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

    /// The module's memory.
    #[wasm_bindgen(js_name = streamMemory)]
    pub fn memory() -> JsValue {
        wasm_bindgen::memory()
    }

    /// A streaming answer (see [`crate::Session`]).
    #[wasm_bindgen]
    pub struct Stream(crate::Session);

    #[wasm_bindgen]
    impl Stream {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self(crate::Session::default())
        }
        /// 0: patch, 1: reset, 2: fall back (see [`crate::Outcome`]).
        pub fn update(&mut self, appended: &str, reset: bool) -> u32 {
            self.0.update(appended, reset) as u32
        }
        /// After a reset: the whole HTML.
        #[wasm_bindgen(getter)]
        pub fn html(&self) -> String {
            let mut html = self.0.addition.clone();
            html.push_str(&self.0.pending);
            html
        }
        /// After a patch: where the edit's opcodes (see `diff::op`) are in [`memory`], as
        /// a `u32` index and count. Read them before the next call into the module.
        #[wasm_bindgen(getter, js_name = opsAt)]
        pub fn ops_at(&self) -> usize {
            self.0.program.ops.as_ptr() as usize / 4
        }
        #[wasm_bindgen(getter, js_name = opsLength)]
        pub fn ops_length(&self) -> usize {
            self.0.program.ops.len()
        }
        /// After a patch: the edit's strings.
        #[wasm_bindgen(getter)]
        pub fn text(&self) -> String {
            self.0.program.text.clone()
        }
        /// Root nodes that became final this update.
        #[wasm_bindgen(getter, js_name = finalNodes)]
        pub fn final_nodes(&self) -> u32 {
            self.0.final_nodes
        }
    }

    /// Bench probe: the HTML's length, without returning it.
    #[wasm_bindgen(js_name = renderStreamLength)]
    pub fn render_stream_length(source: &str) -> usize {
        let mut out = String::with_capacity(source.len() * 2);
        super::render(source, &mut out).map_or(0, |()| out.len())
    }
}
