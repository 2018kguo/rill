//! Streamdown's DOM as HTML, written in one pass over the Markdown with no syntax trees.
//!
//! The input is the text Streamdown renders (after its streaming repair). The output is
//! the HTML of the children of Streamdown's root `<div>`: what its components make of
//! each block, parsed by the browser into the same DOM. Anything this does not reproduce
//! exactly returns [`Fallback`], and the caller renders the text the exact way.

mod inline;
mod simd;

use inline::Inline;

/// The text holds something this engine does not render exactly (yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fallback;

pub type Result<T = ()> = core::result::Result<T, Fallback>;

/// Streamdown's DOM for `source` (one whole, finished document), appended to `out`.
pub fn render(source: &str, out: &mut String) -> Result {
    let bytes = source.as_bytes();
    if memchr::memchr3(b'\r', b'\t', b'|', bytes).is_some() || memchr::memchr(0, bytes).is_some() {
        // ponytail: CR, tabs (column math), NUL and tables take the exact path for now.
        return Err(Fallback);
    }
    // Markup is about a third again the text: one allocation up front.
    out.reserve(source.len() + source.len() / 2);
    let mut blocks = Blocks { source, out, inline: Inline::default(), para: None };
    let mut start = 0;
    while start <= source.len() {
        let end = memchr_newline(bytes, start);
        blocks.line(start, &source[start..end])?;
        start = end + 1;
    }
    blocks.close_paragraph()
}

struct Blocks<'a, 'o> {
    source: &'a str,
    out: &'o mut String,
    inline: Inline,
    /// The open paragraph: from its first content byte to the end of its last line.
    para: Option<(usize, usize)>,
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
            return self.close_paragraph();
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
                    return self.heading(level, content);
                }
            }
            b'*' | b'-' | b'_' if thematic(rest) => {
                if first == b'-' && self.para.is_some() {
                    return Err(Fallback); // setext heading
                }
                self.close_paragraph()?;
                self.out.push_str(r#"<hr class="my-6 border-border" data-streamdown="horizontal-rule">"#);
                return Ok(());
            }
            // Blocks not handled yet: quotes, lists, fences, HTML, definitions, setext.
            b'>' | b'<' | b'[' => return Err(Fallback),
            b'`' | b'~' if rest.bytes().take_while(|&b| b == first).count() >= 3 => {
                return Err(Fallback);
            }
            b'=' if self.para.is_some() && rest.trim_end_matches(' ').bytes().all(|b| b == b'=') => {
                return Err(Fallback);
            }
            b'-' | b'*' | b'+' if rest.len() == 1 || rest.as_bytes()[1] == b' ' => {
                return Err(Fallback);
            }
            b'0'..=b'9' => {
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                if digits <= 9 && matches!(rest.as_bytes().get(digits), Some(b'.' | b')')) {
                    return Err(Fallback);
                }
            }
            _ => {}
        }
        let end = offset + line.len();
        match &mut self.para {
            Some(para) => para.1 = end,
            None => self.para = Some((offset + spaces, end)),
        }
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
        let Some((start, end)) = self.para.take() else {
            return Ok(());
        };
        self.out.push_str("<p>");
        self.inline.render(self.source[start..end].trim_end_matches(' '), self.out)?;
        self.out.push_str("</p>");
        Ok(())
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

    /// Bench probe: the HTML's length, without returning it.
    #[wasm_bindgen(js_name = renderStreamLength)]
    pub fn render_stream_length(source: &str) -> usize {
        let mut out = String::with_capacity(source.len() * 2);
        super::render(source, &mut out).map_or(0, |()| out.len())
    }
}
