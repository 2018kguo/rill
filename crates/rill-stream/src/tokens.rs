//! What the renderer writes: Streamdown's DOM as a token stream rather than HTML.
//!
//! A word per token: `kind | id << 2`. `OPEN` an element of tag `id` and go into it,
//! `CLOSE` it, a `VOID` element, or `TEXT` (`id`: its length in UTF-16 units, the text
//! itself next in `text`). Adjacent text is one token, as it is one DOM text node.
//!
//! Tags are a fixed table, so a tag is one word to write, compare and send, and the DOM
//! side clones an element it parsed once. Ids from [`tag::INTERNED`] on are strings
//! interned for the whole session (see [`Strings`]): with `OPEN`, an element's open tag
//! (attributes and all); with `VOID`, a whole static subtree. Text is kept unescaped in
//! UTF-16, the unit both the DOM and the diff measure it in.

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
    /// A link, as Streamdown's link safety renders it: a button.
    pub const LINK: u32 = 15;
    /// A link whose URL has not arrived (remend's `streamdown:incomplete-link`).
    pub const LINK_INCOMPLETE: u32 = 16;
    pub const DEL: u32 = 17;
    pub const BLOCKQUOTE: u32 = 18;
    /// A code block's `<code>`, a line's span and a line's (single) token span.
    pub const CODE_BLOCK: u32 = 19;
    pub const CODE_LINE: u32 = 20;
    pub const CODE_TOKEN: u32 = 21;
    /// A table's wrapper and scroll container, then its elements; `TH`/`TD` + the
    /// column's alignment (none, left, right, center).
    pub const TABLE_WRAPPER: u32 = 22;
    pub const TABLE_SCROLL: u32 = 23;
    pub const TABLE: u32 = 24;
    pub const THEAD: u32 = 25;
    pub const TBODY: u32 = 26;
    pub const TR: u32 = 27;
    pub const TH: u32 = 28;
    pub const TD: u32 = 32;
    /// A task item's checkbox (unchecked, checked), and a list or item with tasks.
    pub const CHECKBOX: u32 = 36;
    pub const UL_TASKS: u32 = 38;
    pub const OL_TASKS: u32 = 39;
    pub const LI_TASK: u32 = 40;
    /// `INTERNED + index`: [`super::Strings`]' string `index`.
    pub const INTERNED: u32 = 64;
}

/// Each tag's open tag, as Streamdown writes it.
pub const TAGS: [&str; 41] = [
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
    r#"<button class="wrap-anywhere appearance-none text-left font-medium text-primary underline" data-incomplete="false" data-streamdown="link" type="button">"#,
    r#"<button class="wrap-anywhere appearance-none text-left font-medium text-primary underline" data-incomplete="true" data-streamdown="link" type="button">"#,
    "<del>",
    r#"<blockquote class="my-4 border-muted-foreground/30 border-l-4 pl-4 text-muted-foreground italic" data-streamdown="blockquote">"#,
    r#"<code class="[counter-increment:line_0] [counter-reset:line]">"#,
    r#"<span class="block before:content-[counter(line)] before:inline-block before:[counter-increment:line] before:w-6 before:mr-4 before:text-[13px] before:text-right before:text-muted-foreground/50 before:font-mono before:select-none">"#,
    r#"<span class="text-[var(--sdm-c,inherit)] dark:text-[var(--shiki-dark,var(--sdm-c,inherit))] bg-[var(--sdm-tbg)] dark:bg-[var(--shiki-dark-bg,var(--sdm-tbg))]" style="--sdm-c: inherit; --sdm-tbg: transparent;">"#,
    r#"<div class="my-4 flex flex-col gap-2 rounded-lg border border-border bg-sidebar p-2" data-streamdown="table-wrapper">"#,
    r#"<div class="border-collapse overflow-x-auto overflow-y-auto rounded-md border border-border bg-background" style="max-height: 300px;">"#,
    r#"<table class="w-full divide-y divide-border" data-streamdown="table">"#,
    r#"<thead class="bg-muted/80" data-streamdown="table-header">"#,
    r#"<tbody class="divide-y divide-border" data-streamdown="table-body">"#,
    r#"<tr class="border-border" data-streamdown="table-row">"#,
    r#"<th class="whitespace-nowrap px-4 py-2 text-left font-semibold text-sm" data-streamdown="table-header-cell">"#,
    r#"<th class="whitespace-nowrap px-4 py-2 text-left font-semibold text-sm" data-streamdown="table-header-cell" style="text-align: left;">"#,
    r#"<th class="whitespace-nowrap px-4 py-2 text-left font-semibold text-sm" data-streamdown="table-header-cell" style="text-align: right;">"#,
    r#"<th class="whitespace-nowrap px-4 py-2 text-left font-semibold text-sm" data-streamdown="table-header-cell" style="text-align: center;">"#,
    r#"<td class="px-4 py-2 text-sm" data-streamdown="table-cell">"#,
    r#"<td class="px-4 py-2 text-sm" data-streamdown="table-cell" style="text-align: left;">"#,
    r#"<td class="px-4 py-2 text-sm" data-streamdown="table-cell" style="text-align: right;">"#,
    r#"<td class="px-4 py-2 text-sm" data-streamdown="table-cell" style="text-align: center;">"#,
    r#"<input disabled="" type="checkbox">"#,
    r#"<input disabled="" type="checkbox" checked="">"#,
    r#"<ul class="list-inside list-disc whitespace-normal [li_&amp;]:pl-6 contains-task-list" data-streamdown="unordered-list">"#,
    r#"<ol class="list-inside list-decimal whitespace-normal [li_&amp;]:pl-6 contains-task-list" data-streamdown="ordered-list">"#,
    r#"<li class="py-1 [&amp;&gt;p]:inline task-list-item" data-streamdown="list-item">"#,
];

/// The strings interned tokens name, kept for a whole session so that a token means the
/// same in every rendering.
#[derive(Default)]
pub struct Strings {
    pub list: Vec<String>,
    index: std::collections::HashMap<String, u32>,
}

impl Strings {
    /// The id of `string` (interning it).
    pub fn id(&mut self, string: &str) -> u32 {
        if let Some(&id) = self.index.get(string) {
            return id;
        }
        let id = tag::INTERNED + self.list.len() as u32;
        self.list.push(string.to_owned());
        self.index.insert(string.to_owned(), id);
        id
    }

    pub(crate) fn get(&self, id: u32) -> &str {
        &self.list[(id - tag::INTERNED) as usize]
    }
}

/// Streamdown's props the DOM depends on.
#[derive(Clone, Copy)]
pub struct Options {
    /// `controls`: code blocks' and tables' buttons.
    pub controls: bool,
    /// `isAnimating`: the last block's unclosed code fence marks its code blocks
    /// incomplete.
    pub animating: bool,
    /// `lineNumbers`: code lines count themselves.
    pub line_numbers: bool,
    /// `codeBlockMaxHeight` in pixels (0: none).
    pub code_max_height: u32,
    /// `dir`: each block in a `display: contents` element of this direction (0: none, 1:
    /// `ltr`, 2: `rtl`).
    pub dir: u8,
    /// The text is one block (Streamdown's, for a text with definitions or footnotes).
    pub whole: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            controls: true,
            animating: false,
            line_numbers: true,
            code_max_height: 400,
            dir: 0,
            whole: false,
        }
    }
}

/// An origin for text that is not one stretch of the source.
pub const NOWHERE: u32 = u32::MAX;

/// The token stream and its text.
#[derive(Default)]
pub struct Out {
    pub words: Vec<u32>,
    /// The text of the text tokens that are not one stretch of the source, in order.
    pub text: Vec<u16>,
    /// Per word: for a text token that is one stretch of the source (all before the
    /// render's `agree`), where it starts in the whole text, else [`NOWHERE`]. Such a
    /// token's text is not copied: it is the source's.
    pub origins: Vec<u32>,
    /// The source being rendered (its address and length), where it starts in the whole
    /// text, and where in the whole text it stops being the text as given.
    source: (usize, usize),
    source_at: usize,
    agree: usize,
    /// The session's interned strings (shared by its renderings).
    pub strings: std::rc::Rc<std::cell::RefCell<Strings>>,
    /// What the rendering depends on besides the text.
    pub options: Options,
    /// The text turned out to need [`Options::whole`] other than it was set.
    pub flipped: bool,
}

impl Out {
    pub fn clear(&mut self) {
        self.words.clear();
        self.text.clear();
        self.origins.clear();
    }

    /// The source the slices handed to [`Out::text`] come from, starting at `at` in the
    /// whole text, which is the text as given before `agree`. It must outlive the render.
    pub fn set_source(&mut self, source: &[u16], at: usize, agree: usize) {
        self.source = (source.as_ptr() as usize, source.len());
        self.source_at = at;
        self.agree = agree;
    }

    /// Where `text` starts in the whole text, when it is a slice of the source.
    pub fn offset_of(&self, text: &[u16]) -> Option<usize> {
        // Wholly inside: a buffer allocated right after the source is not part of it.
        let offset = (text.as_ptr() as usize).wrapping_sub(self.source.0) / 2;
        (offset <= self.source.1 && text.len() <= self.source.1 - offset)
            .then(|| self.source_at + offset)
    }

    /// Appends tokens as they were written before (`origins` per word, `text` the
    /// literal text).
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

    /// Adds `attribute` (` name="value"`) to the element word `at` opens (or is).
    pub fn add_attribute(&mut self, at: usize, attribute: &str) {
        let word = self.words[at];
        let id = word >> 2;
        let html = if id >= tag::INTERNED {
            self.strings.borrow().get(id).to_owned()
        } else {
            TAGS[id as usize].to_owned()
        };
        let end = html.find('>').expect("an open tag");
        let html = format!("{}{attribute}{}", &html[..end], &html[end..]);
        let id = self.strings.borrow_mut().id(&html);
        self.words[at] = word & 3 | id << 2;
    }

    /// Opens an element with the open tag `html`.
    pub fn open_html(&mut self, html: &str) {
        let id = self.strings.borrow_mut().id(html);
        self.open(id);
    }

    /// A static subtree, `html`.
    pub fn subtree(&mut self, html: &str) {
        let id = self.strings.borrow_mut().id(html);
        self.void(id);
    }

    /// Appends text, to the text token before when there is one.
    #[inline]
    pub fn text(&mut self, text: &[u16]) {
        if text.is_empty() {
            return;
        }
        let units = text.len() as u32;
        if let Some(origin) = self.offset_of(text)
            && origin + text.len() <= self.agree
        {
            match self.last_text() {
                // It follows the source token before: that one grows.
                Some(Some(before))
                    if before + (self.words.last().expect("a token") >> 2) as usize == origin =>
                {
                    *self.words.last_mut().expect("a token") += units << 2;
                    return;
                }
                None => {
                    self.words.push(kind::TEXT | units << 2);
                    self.origins.push(origin as u32);
                    return;
                }
                _ => {}
            }
        }
        self.literal(units, |literal| literal.extend_from_slice(text));
    }

    /// Appends ASCII text.
    pub fn ascii(&mut self, text: &str) {
        self.literal(text.len() as u32, |literal| {
            literal.extend(text.bytes().map(u16::from))
        });
    }

    /// Appends `byte` (ASCII) `count` times.
    pub fn repeat(&mut self, byte: u8, count: u32) {
        if count == 0 {
            return;
        }
        self.literal(count, |literal| {
            literal.extend(std::iter::repeat_n(u16::from(byte), count as usize))
        });
    }

    /// The last token's origin when it is text (`None` when it is not text).
    fn last_text(&self) -> Option<Option<usize>> {
        let &last = self.words.last()?;
        (last & 3 == kind::TEXT).then(|| {
            let origin = *self.origins.last().expect("parallel");
            (origin != NOWHERE).then_some(origin as usize)
        })
    }

    /// Appends `units` units of literal text that `write` writes, to the text token
    /// before when there is one (copying a source token's text first).
    fn literal(&mut self, units: u32, write: impl FnOnce(&mut Vec<u16>)) {
        match self.last_text() {
            Some(origin) => {
                if let Some(origin) = origin {
                    let length = (self.words.last().expect("a token") >> 2) as usize;
                    // SAFETY: the source outlives the render (`set_source`), and the
                    // token is a stretch of it.
                    let source = unsafe {
                        std::slice::from_raw_parts(self.source.0 as *const u16, self.source.1)
                    };
                    let start = origin - self.source_at;
                    self.text.extend_from_slice(&source[start..start + length]);
                    *self.origins.last_mut().expect("parallel") = NOWHERE;
                }
                *self.words.last_mut().expect("a token") += units << 2;
            }
            None => {
                self.words.push(kind::TEXT | units << 2);
                self.origins.push(NOWHERE);
            }
        }
        write(&mut self.text);
    }
}

/// The HTML of `words` (the literal text from `text[0]`, source text from `source`, the
/// whole text), appended to `out`.
pub fn to_html(
    words: &[u32],
    text: &[u16],
    origins: &[u32],
    source: &[u16],
    strings: &Strings,
    out: &mut String,
) {
    let mut open: Vec<u32> = Vec::new();
    let mut at = 0;
    for (index, &word) in words.iter().enumerate() {
        let id = word >> 2;
        match word & 3 {
            kind::OPEN => {
                out.push_str(if id >= tag::INTERNED {
                    strings.get(id)
                } else {
                    TAGS[id as usize]
                });
                open.push(id);
            }
            kind::CLOSE => {
                let id = open.pop().expect("balanced");
                let tag = if id >= tag::INTERNED {
                    strings.get(id)
                } else {
                    TAGS[id as usize]
                };
                out.push_str("</");
                out.push_str(&tag[1..tag.find([' ', '>']).expect("a tag")]);
                out.push('>');
            }
            kind::VOID => out.push_str(if id >= tag::INTERNED {
                strings.get(id)
            } else {
                TAGS[id as usize]
            }),
            _ => {
                let units = if origins[index] == NOWHERE {
                    at += id as usize;
                    &text[at - id as usize..at]
                } else {
                    let origin = origins[index] as usize;
                    &source[origin..origin + id as usize]
                };
                for c in char::decode_utf16(units.iter().copied()) {
                    match c.unwrap_or(char::REPLACEMENT_CHARACTER) {
                        '&' => out.push_str("&amp;"),
                        '<' => out.push_str("&lt;"),
                        '>' => out.push_str("&gt;"),
                        c => out.push(c),
                    }
                }
            }
        }
    }
}
