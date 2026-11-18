//! What the renderer writes: Streamdown's DOM as a token stream rather than HTML.
//!
//! A word per token: `kind | id << 2`. `OPEN` an element of tag `id` and go into it,
//! `CLOSE` it, a `VOID` element, or `TEXT` (`id`: its length in UTF-16 units, the text
//! itself next in `text`). Adjacent text is one token, as it is one DOM text node.
//!
//! Tags are a fixed table, so a tag is one word to write, compare and send, and the DOM
//! side clones an element it parsed once. An ordered list starting at `n` other than 1
//! is tag `OL_START + n`. Text is kept unescaped in UTF-16, the unit both the DOM and
//! the diff measure it in.

pub mod kind {
    pub const OPEN: u32 = 0;
    pub const CLOSE: u32 = 1;
    pub const VOID: u32 = 2;
    pub const TEXT: u32 = 3;
}

/// Tag ids: indices into [`TAGS`].
pub mod tag {
    pub const P: u32 = 0;
    pub const EM: u32 = 1;
    pub const STRONG: u32 = 2;
    pub const CODE: u32 = 3;
    /// `H1 + level - 1`.
    pub const H1: u32 = 4;
    pub const UL: u32 = 10;
    pub const OL: u32 = 11;
    pub const LI: u32 = 12;
    pub const HR: u32 = 13;
    pub const BR: u32 = 14;
    /// `OL_START + start`: an ordered list with a `start` attribute.
    pub const OL_START: u32 = 16;
}

/// Each tag's open tag, as Streamdown writes it.
pub const TAGS: [&str; 15] = [
    "<p>",
    "<em>",
    r#"<span class="font-semibold" data-streamdown="strong">"#,
    r#"<code class="rounded bg-muted px-1.5 py-0.5 font-mono text-sm" data-streamdown="inline-code">"#,
    r#"<h1 class="mt-6 mb-2 font-semibold text-3xl" data-streamdown="heading-1">"#,
    r#"<h2 class="mt-6 mb-2 font-semibold text-2xl" data-streamdown="heading-2">"#,
    r#"<h3 class="mt-6 mb-2 font-semibold text-xl" data-streamdown="heading-3">"#,
    r#"<h4 class="mt-6 mb-2 font-semibold text-lg" data-streamdown="heading-4">"#,
    r#"<h5 class="mt-6 mb-2 font-semibold text-base" data-streamdown="heading-5">"#,
    r#"<h6 class="mt-6 mb-2 font-semibold text-sm" data-streamdown="heading-6">"#,
    r#"<ul class="list-inside list-disc whitespace-normal [li_&amp;]:pl-6" data-streamdown="unordered-list">"#,
    r#"<ol class="list-inside list-decimal whitespace-normal [li_&amp;]:pl-6" data-streamdown="ordered-list">"#,
    r#"<li class="py-1 [&amp;&gt;p]:inline" data-streamdown="list-item">"#,
    r#"<hr class="my-6 border-border" data-streamdown="horizontal-rule">"#,
    "<br>",
];

/// An origin for text that is not one stretch of the source.
pub const NOWHERE: u32 = u32::MAX;

/// The token stream and its text.
#[derive(Default)]
pub struct Out {
    pub words: Vec<u32>,
    pub text: Vec<u16>,
    /// Per word: for a text token that is one stretch of the source, where it starts
    /// there (in the whole text), else [`NOWHERE`].
    pub origins: Vec<u32>,
    /// The source being rendered (its address and length) and where it starts in the
    /// whole text.
    source: (usize, usize),
    source_at: u32,
}

impl Out {
    pub fn clear(&mut self) {
        self.words.clear();
        self.text.clear();
        self.origins.clear();
    }

    /// The source text slices handed to [`Out::text`] come from, starting at `at` in the
    /// whole text.
    pub fn set_source(&mut self, source: &[u16], at: usize) {
        self.source = (source.as_ptr() as usize, source.len());
        self.source_at = at as u32;
    }

    /// Where `text` starts in the whole text, when it is a slice of the source.
    pub fn offset_of(&self, text: &[u16]) -> Option<usize> {
        let offset = (text.as_ptr() as usize).wrapping_sub(self.source.0) / 2;
        (offset <= self.source.1).then(|| self.source_at as usize + offset)
    }

    /// Appends tokens as they were written before (`origins` per word).
    pub fn extend(&mut self, words: &[u32], text: &[u16], origins: &[u32]) {
        self.words.extend_from_slice(words);
        self.text.extend_from_slice(text);
        self.origins.extend_from_slice(origins);
    }

    #[inline]
    fn word(&mut self, word: u32) {
        self.words.push(word);
        self.origins.push(NOWHERE);
    }

    #[inline]
    pub fn open(&mut self, tag: u32) {
        self.word(kind::OPEN | tag << 2);
    }

    #[inline]
    pub fn close(&mut self) {
        self.word(kind::CLOSE);
    }

    #[inline]
    pub fn void(&mut self, tag: u32) {
        self.word(kind::VOID | tag << 2);
    }

    /// Appends text, to the text token before when there is one.
    #[inline]
    pub fn text(&mut self, text: &[u16]) {
        if text.is_empty() {
            return;
        }
        // Where `text` is in the source, when it is a slice of it.
        let offset = (text.as_ptr() as usize).wrapping_sub(self.source.0) / 2;
        let origin = if offset < self.source.1 { self.source_at + offset as u32 } else { NOWHERE };
        self.text.extend_from_slice(text);
        self.grow(text.len() as u32, origin);
    }

    /// Appends ASCII text.
    pub fn ascii(&mut self, text: &str) {
        self.text.extend(text.bytes().map(u16::from));
        self.grow(text.len() as u32, NOWHERE);
    }

    /// Appends `byte` (ASCII) `count` times.
    pub fn repeat(&mut self, byte: u8, count: u32) {
        if count == 0 {
            return;
        }
        self.text.extend(std::iter::repeat_n(u16::from(byte), count as usize));
        self.grow(count, NOWHERE);
    }

    fn grow(&mut self, units: u32, origin: u32) {
        match self.words.last_mut() {
            Some(last) if *last & 3 == kind::TEXT => {
                // Still one stretch only when this text follows the token's own.
                let before = self.origins.last_mut().expect("parallel");
                if *before != NOWHERE && (origin == NOWHERE || *before + (*last >> 2) != origin) {
                    *before = NOWHERE;
                }
                *last += units << 2;
            }
            _ => {
                self.words.push(kind::TEXT | units << 2);
                self.origins.push(origin);
            }
        }
    }
}

/// The HTML of `words` (their text from `text[0]`), appended to `out`.
pub fn to_html(words: &[u32], text: &[u16], out: &mut String) {
    let mut open: Vec<u32> = Vec::new();
    let mut at = 0;
    for &word in words {
        let id = word >> 2;
        match word & 3 {
            kind::OPEN => {
                if id >= tag::OL_START {
                    out.push_str(&TAGS[tag::OL as usize][..TAGS[tag::OL as usize].len() - 1]);
                    out.push_str(&format!(" start=\"{}\">", id - tag::OL_START));
                } else {
                    out.push_str(TAGS[id as usize]);
                }
                open.push(if id >= tag::OL_START { tag::OL } else { id });
            }
            kind::CLOSE => {
                let id = open.pop().expect("balanced");
                let name = &TAGS[id as usize][1..];
                let name = &name[..name.find([' ', '>']).expect("a tag")];
                out.push_str("</");
                out.push_str(name);
                out.push('>');
            }
            kind::VOID => out.push_str(TAGS[id as usize]),
            _ => {
                let end = at + id as usize;
                for c in char::decode_utf16(text[at..end].iter().copied()) {
                    match c.unwrap_or(char::REPLACEMENT_CHARACTER) {
                        '&' => out.push_str("&amp;"),
                        '<' => out.push_str("&lt;"),
                        '>' => out.push_str("&gt;"),
                        c => out.push(c),
                    }
                }
                at = end;
            }
        }
    }
}
