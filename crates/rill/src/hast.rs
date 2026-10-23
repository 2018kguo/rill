//! HAST: the arena MDAST becomes the tree Streamdown renders, after mdast-util-to-hast
//! (remark-rehype with `allowDangerousHtml`), rehype-raw, rehype-sanitize (the default
//! schema) and rehype-harden (Streamdown's options). Each stage ports the package it
//! names; rehype-katex, and parse5 for raw HTML, stay in JavaScript (see [`Kind`]).
//!
//! The tree is an arena: nodes link by index, strings borrow the source where possible,
//! positions stay byte offsets until JSON, and every later stage edits links in place.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;

use crate::json::{json_string, json_u32};
use crate::mdast::{
    self, AlignKind, At, Error, Ix, MKind, MTree, NIL, Points, Pt, Reference, ReferenceKind,
    push_cow,
};
use crate::url::Url;

// ---------------------------------------------------------------------------
// Arena HAST
// ---------------------------------------------------------------------------

/// One side of a unist position. parse5 text merges can keep only one side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    None,
    Byte(u32),
    /// One unit past the line ending at this byte (see [`Points::past_line_ending`]).
    PastLineEnding(u32),
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Val<'s> {
    Str(Cow<'s, str>),
    Bool(bool),
    Num(i64),
    List(Vec<Cow<'s, str>>),
    /// A one-class list, `["name"]`, without allocating.
    Class(&'static str),
}

/// An element's properties, in hast's order.
pub type Props<'s> = Vec<(&'static str, Val<'s>)>;

/// An element's hast `data`, boxed because only some code elements have any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Data {
    /// `data.meta` from mdast-util-to-hast's code handler.
    pub meta: Box<str>,
}

#[derive(Clone, Debug)]
pub enum HKind<'s> {
    Root,
    Element {
        tag: &'static str,
        props: Props<'s>,
        data: Option<Box<Data>>,
    },
    Text(Cow<'s, str>),
    Raw(Cow<'s, str>),
}

#[derive(Clone, Debug)]
pub struct HNode<'s> {
    pub kind: HKind<'s>,
    pub first: Ix,
    pub last: Ix,
    pub next: Ix,
    pub start: Edge,
    pub end: Edge,
}

impl HNode<'_> {
    /// hast-util-sanitize keeps only complete positions.
    fn drop_partial_position(&mut self) {
        if self.start == Edge::None || self.end == Edge::None {
            self.start = Edge::None;
            self.end = Edge::None;
        }
    }
}

pub struct HTree<'s> {
    pub nodes: Vec<HNode<'s>>,
    /// `data.quirksMode` from `rehype-raw`.
    pub quirks: bool,
}

/// Whether `wrap` also puts newlines around the nodes, not only between them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spacing {
    Tight,
    Loose,
}

#[derive(Clone, Copy)]
struct Chain {
    first: Ix,
    last: Ix,
}

const EMPTY: Chain = Chain {
    first: NIL,
    last: NIL,
};

impl Chain {
    /// A chain of just `ix`, a fresh node whose `next` is still `NIL`. A node that may
    /// still be linked needs [`HTree::one_chain`].
    fn one(ix: Ix) -> Chain {
        Chain {
            first: ix,
            last: ix,
        }
    }
}

impl Edge {
    /// The point this edge stands for, if any.
    fn point(self, points: &Points) -> Option<Pt> {
        match self {
            Edge::None => None,
            Edge::Byte(byte) => Some(points.pt(byte)),
            Edge::PastLineEnding(byte) => Some(points.past_line_ending(byte)),
        }
    }
}

fn edges(at: At) -> (Edge, Edge) {
    match at {
        At::None => (Edge::None, Edge::None),
        At::Bytes(start, end) => (Edge::Byte(start), Edge::Byte(end)),
        At::PastLineEnding { line_ending, end } => {
            (Edge::PastLineEnding(line_ending), Edge::Byte(end))
        }
    }
}

impl<'s> HTree<'s> {
    fn push(&mut self, kind: HKind<'s>, at: At) -> Ix {
        let (start, end) = edges(at);
        let ix = self.nodes.len() as Ix;
        self.nodes.push(HNode {
            kind,
            first: NIL,
            last: NIL,
            next: NIL,
            start,
            end,
        });
        ix
    }

    fn text(&mut self, value: impl Into<Cow<'s, str>>) -> Ix {
        self.push(HKind::Text(value.into()), At::None)
    }

    fn element(&mut self, tag: &'static str, props: Props<'s>, at: At) -> Ix {
        self.push(
            HKind::Element {
                tag,
                props,
                data: None,
            },
            at,
        )
    }

    /// A new node of `kind` with `children`.
    fn parent(&mut self, kind: HKind<'s>, at: At, children: Chain) -> Ix {
        let parent = self.push(kind, at);
        self.set_children(parent, children);
        parent
    }

    /// A new element with `children`.
    fn element_with(&mut self, tag: &'static str, props: Props<'s>, at: At, children: Chain) -> Ix {
        let element = self.element(tag, props, at);
        self.set_children(element, children);
        element
    }

    /// A new element around one fresh node.
    fn element_around(&mut self, tag: &'static str, props: Props<'s>, at: At, child: Ix) -> Ix {
        self.element_with(tag, props, at, Chain::one(child))
    }

    fn set_children(&mut self, parent: Ix, chain: Chain) {
        self.nodes[parent as usize].first = chain.first;
        self.nodes[parent as usize].last = chain.last;
    }

    fn children_of(&self, parent: Ix) -> Chain {
        Chain {
            first: self.nodes[parent as usize].first,
            last: self.nodes[parent as usize].last,
        }
    }

    fn one_chain(&mut self, ix: Ix) -> Chain {
        self.nodes[ix as usize].next = NIL;
        Chain {
            first: ix,
            last: ix,
        }
    }

    fn append(&mut self, chain: &mut Chain, ix: Ix) {
        self.nodes[ix as usize].next = NIL;
        if chain.first == NIL {
            chain.first = ix;
        } else {
            self.nodes[chain.last as usize].next = ix;
        }
        chain.last = ix;
    }

    fn extend(&mut self, chain: &mut Chain, other: Chain) {
        if other.first == NIL {
            return;
        }
        if chain.first == NIL {
            *chain = other;
        } else {
            self.nodes[chain.last as usize].next = other.first;
            chain.last = other.last;
        }
    }

    fn prepend(&mut self, chain: &mut Chain, ix: Ix) {
        self.nodes[ix as usize].next = chain.first;
        if chain.first == NIL {
            chain.last = ix;
        }
        chain.first = ix;
    }

    /// `first` and the siblings after it.
    fn siblings(&self, first: Ix) -> impl Iterator<Item = Ix> + '_ {
        let mut cursor = first;
        std::iter::from_fn(move || {
            let ix = cursor;
            (ix != NIL).then(|| {
                cursor = self.nodes[ix as usize].next;
                ix
            })
        })
    }

    /// An element's tag name.
    #[must_use]
    pub fn tag_name(&self, ix: Ix) -> Option<&'static str> {
        match &self.nodes[ix as usize].kind {
            HKind::Element { tag, .. } => Some(tag),
            _ => None,
        }
    }

    /// An element's properties.
    pub fn properties_mut(&mut self, ix: Ix) -> Option<&mut Props<'s>> {
        match &mut self.nodes[ix as usize].kind {
            HKind::Element { props, .. } => Some(props),
            _ => None,
        }
    }

    /// mdast-util-to-hast `wrap`: newline text nodes between (and around, if loose).
    fn wrap(&mut self, nodes: Chain, spacing: Spacing) -> Chain {
        let loose = spacing == Spacing::Loose;
        let mut out = EMPTY;
        if loose {
            let newline = self.text("\n");
            self.append(&mut out, newline);
        }
        let mut cursor = nodes.first;
        while cursor != NIL {
            let next = self.nodes[cursor as usize].next;
            if cursor != nodes.first {
                let newline = self.text("\n");
                self.append(&mut out, newline);
            }
            self.append(&mut out, cursor);
            cursor = next;
        }
        if loose && nodes.first != NIL {
            let newline = self.text("\n");
            self.append(&mut out, newline);
        }
        out
    }
}

/// Editing for [`Plugin`]s. New nodes have no position, like nodes a rehype plugin makes.
impl<'s> HTree<'s> {
    /// A new, detached node.
    pub fn add(&mut self, kind: HKind<'s>) -> Ix {
        self.push(kind, At::None)
    }

    /// `parent`'s children, in order.
    pub fn children(&self, parent: Ix) -> impl Iterator<Item = Ix> + '_ {
        self.siblings(self.nodes[parent as usize].first)
    }

    /// `parent`'s children, in order, collected so the tree can be edited meanwhile.
    #[must_use]
    pub fn child_list(&self, parent: Ix) -> Vec<Ix> {
        self.children(parent).collect()
    }

    /// Makes `children` (detached nodes or `parent`'s own) `parent`'s children.
    pub fn replace_children(&mut self, parent: Ix, children: &[Ix]) {
        let mut chain = EMPTY;
        for &child in children {
            self.append(&mut chain, child);
        }
        self.set_children(parent, chain);
    }
}

/// Sets property `key`, replacing any value it had.
pub fn set<'s>(props: &mut Props<'s>, key: &'static str, value: Val<'s>) {
    match props.iter_mut().find(|(name, _)| *name == key) {
        Some((_, slot)) => *slot = value,
        None => props.push((key, value)),
    }
}

/// Property `key`.
#[must_use]
pub fn get<'p, 's>(props: &'p Props<'s>, key: &str) -> Option<&'p Val<'s>> {
    props
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value)
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Slices a `Cow`, keeping a borrow borrowed.
fn slice_cow<'s>(value: &Cow<'s, str>, range: std::ops::Range<usize>) -> Cow<'s, str> {
    match value {
        Cow::Borrowed(borrowed) => Cow::Borrowed(&borrowed[range]),
        Cow::Owned(owned) => Cow::Owned(owned[range].to_owned()),
    }
}

fn trim_markdown_space_start<'s>(value: &Cow<'s, str>) -> Cow<'s, str> {
    let trimmed = value.len() - value.trim_start_matches([' ', '\t']).len();
    slice_cow(value, trimmed..value.len())
}

/// `trim-lines` 3.0.1.
fn trim_lines<'s>(value: &Cow<'s, str>) -> Cow<'s, str> {
    // Only spaces and tabs next to a line ending go.
    let blank = |byte: &u8| matches!(byte, b' ' | b'\t');
    let ending = |byte: &u8| matches!(byte, b'\n' | b'\r');
    let unchanged = !value
        .as_bytes()
        .windows(2)
        .any(|pair| blank(&pair[0]) && ending(&pair[1]) || ending(&pair[0]) && blank(&pair[1]));
    if unchanged {
        return value.clone();
    }
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut last = 0;
    let mut index = 0;
    let mut first = true;
    while index < bytes.len() {
        let ending = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            b'\r' | b'\n' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        let mut line = &value[last..index];
        if !first {
            line = line.trim_start_matches([' ', '\t']);
        }
        out.push_str(line.trim_end_matches([' ', '\t']));
        out.push_str(&value[index..index + ending]);
        index += ending;
        last = index;
        first = false;
    }
    out.push_str(value[last..].trim_start_matches([' ', '\t']));
    Cow::Owned(out)
}

/// micromark-util-sanitize-uri 2.0.1 `normalizeUri`.
#[must_use]
pub fn normalize_uri(value: Cow<'_, str>) -> Cow<'_, str> {
    let keep = |byte: u8| matches!(byte, b'!' | b'#' | b'$' | b'&'..=b';' | b'=' | b'?'..=b'Z' | b'_' | b'a'..=b'z' | b'~');
    let bytes = value.as_bytes();
    let escape_ok = |index: usize| {
        bytes.get(index + 1).is_some_and(u8::is_ascii_alphanumeric)
            && bytes.get(index + 2).is_some_and(u8::is_ascii_alphanumeric)
    };
    let clean = bytes.iter().enumerate().all(|(index, &byte)| {
        (byte < 128 && byte != b'%' && keep(byte)) || (byte == b'%' && escape_ok(index))
    });
    if clean {
        return value;
    }
    let mut out = String::with_capacity(value.len() + 8);
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' && escape_ok(index) {
            out.push_str(&value[index..index + 3]);
            index += 3;
            continue;
        }
        if byte < 128 {
            if keep(byte) {
                out.push(byte as char);
            } else {
                let _ = write!(out, "%{byte:02X}");
            }
            index += 1;
            continue;
        }
        let Some(character) = value[index..].chars().next() else {
            break;
        };
        let mut buffer = [0u8; 4];
        let encoded = character.encode_utf8(&mut buffer);
        for utf8_byte in encoded.as_bytes() {
            let _ = write!(out, "%{utf8_byte:02X}");
        }
        index += character.len_utf8();
    }
    Cow::Owned(out)
}

fn js_whitespace(c: char) -> bool {
    rill_cmark::autolink_literal::unicode_whitespace(c)
}

/// The id footnote hrefs use for `id`, an uppercased identifier.
fn footnote_safe_id(id: &str) -> String {
    normalize_uri(Cow::Owned(id.to_lowercase())).into_owned()
}

/// What tells the `n`th call of one footnote apart: nothing for the first, `-n` after.
fn call_suffix(n: usize) -> String {
    if n > 1 {
        format!("-{n}")
    } else {
        String::new()
    }
}

/// What follows an unresolved reference's content when it reverts to text.
fn revert_suffix(reference: &Reference) -> String {
    match reference.kind {
        ReferenceKind::Collapsed => "][]".into(),
        ReferenceKind::Full => {
            let label = if reference.label.is_empty() {
                &reference.identifier
            } else {
                &reference.label
            };
            format!("][{label}]")
        }
        ReferenceKind::Shortcut => "]".into(),
    }
}

// ---------------------------------------------------------------------------
// mdast-util-to-hast 13.2.1
// ---------------------------------------------------------------------------

/// What a link or image reference resolves to.
#[derive(Clone)]
struct Definition<'s> {
    url: Cow<'s, str>,
    title: Option<Cow<'s, str>>,
}

/// One footnote's citations so far.
struct FootnoteCalls {
    /// The uppercased identifier.
    id: String,
    count: usize,
}

struct ToHast<'t, 's> {
    mdast: &'t MTree<'s>,
    hast: HTree<'s>,
    /// Definitions by uppercased identifier; the first of a duplicate wins.
    definitions: HashMap<String, Definition<'s>>,
    footnotes: HashMap<String, Ix>,
    /// Cited footnotes in first-citation order, which numbers them.
    footnote_calls: Vec<FootnoteCalls>,
    /// Raw HTML or links inside links: parse5 effects the native raw stage omits.
    raw_boundary: bool,
    link_depth: usize,
    math: bool,
}

impl<'s> ToHast<'_, 's> {
    fn collect(&mut self, ix: Ix) {
        match &self.mdast.node(ix).kind {
            MKind::Definition(definition) => {
                self.definitions
                    .entry(definition.identifier.to_uppercase())
                    .or_insert_with(|| Definition {
                        url: definition.url.clone(),
                        title: definition.title.clone(),
                    });
            }
            MKind::FootnoteDefinition(association) => {
                self.footnotes
                    .entry(association.identifier.to_uppercase())
                    .or_insert(ix);
            }
            _ => {}
        }
        for child in self.mdast.children(ix) {
            self.collect(child);
        }
    }

    /// The definition `reference` resolves to.
    fn definition(&self, reference: &Reference) -> Option<Definition<'s>> {
        self.definitions
            .get(&reference.identifier.to_uppercase())
            .cloned()
    }

    fn defined(&self, identifier: &str) -> bool {
        self.definitions.contains_key(&identifier.to_uppercase())
    }

    /// Handlers that return an array skip the after-break trim.
    fn returns_array(&self, ix: Ix) -> bool {
        match &self.mdast.node(ix).kind {
            MKind::Break => true,
            MKind::LinkReference(reference) | MKind::ImageReference(reference) => {
                !self.defined(&reference.identifier)
            }
            _ => false,
        }
    }

    fn all(&mut self, parent: Ix) -> Chain {
        let mut values = EMPTY;
        let mut previous_break = false;
        for child in self.mdast.children(parent) {
            let result = self.one(child, parent);
            if result.first != NIL && previous_break && !self.returns_array(child) {
                let head = result.first;
                let target = match &self.hast.nodes[head as usize].kind {
                    HKind::Text(_) => Some(head),
                    HKind::Element { .. } => {
                        let first = self.hast.nodes[head as usize].first;
                        (first != NIL
                            && matches!(self.hast.nodes[first as usize].kind, HKind::Text(_)))
                        .then_some(first)
                    }
                    _ => None,
                };
                if let Some(target) = target
                    && let HKind::Text(value) = &mut self.hast.nodes[target as usize].kind
                {
                    *value = trim_markdown_space_start(value);
                }
            }
            previous_break = matches!(self.mdast.node(child).kind, MKind::Break);
            self.hast.extend(&mut values, result);
        }
        values
    }

    fn element(&mut self, tag: &'static str, ix: Ix, props: Props<'s>) -> Chain {
        let children = self.all(ix);
        Chain::one(
            self.hast
                .element_with(tag, props, self.mdast.node(ix).at, children),
        )
    }

    fn wrapped(&mut self, tag: &'static str, ix: Ix, props: Props<'s>, spacing: Spacing) -> Chain {
        let children = self.all(ix);
        let children = self.hast.wrap(children, spacing);
        Chain::one(
            self.hast
                .element_with(tag, props, self.mdast.node(ix).at, children),
        )
    }

    fn one(&mut self, ix: Ix, parent: Ix) -> Chain {
        // A copy of the shared borrow, so node fields outlive the `&mut self` calls.
        let mdast = self.mdast;
        let at = mdast.node(ix).at;
        match &mdast.node(ix).kind {
            MKind::Root => {
                let children = self.all(ix);
                let children = self.hast.wrap(children, Spacing::Tight);
                Chain::one(self.hast.parent(HKind::Root, at, children))
            }
            MKind::Blockquote => self.wrapped("blockquote", ix, Vec::new(), Spacing::Loose),
            MKind::Break => {
                let br = self.hast.element("br", Vec::new(), at);
                let mut chain = Chain::one(br);
                let newline = self.hast.text("\n");
                self.hast.append(&mut chain, newline);
                chain
            }
            MKind::Code(code) => {
                let value = if code.value.is_empty() {
                    Cow::Borrowed("")
                } else {
                    Cow::Owned(format!("{}\n", code.value))
                };
                let mut props = Vec::new();
                if let Some(lang) = code.lang.as_deref().filter(|lang| !lang.is_empty()) {
                    let first = lang
                        .split(js_whitespace)
                        .next()
                        .expect("split yields at least one piece");
                    props.push((
                        "className",
                        Val::List(vec![Cow::Owned(format!("language-{first}"))]),
                    ));
                }
                let data = code
                    .meta
                    .clone()
                    .filter(|meta| !meta.is_empty())
                    .map(|meta| Box::new(Data { meta }));
                let text = self.hast.text(value);
                let code = self.hast.parent(
                    HKind::Element {
                        tag: "code",
                        props,
                        data,
                    },
                    at,
                    Chain::one(text),
                );
                Chain::one(self.hast.element_around("pre", Vec::new(), at, code))
            }
            MKind::Delete => self.element("del", ix, Vec::new()),
            MKind::Emphasis => self.element("em", ix, Vec::new()),
            MKind::Strong => self.element("strong", ix, Vec::new()),
            MKind::Paragraph => self.element("p", ix, Vec::new()),
            MKind::Heading(depth) => {
                let tag =
                    ["h1", "h2", "h3", "h4", "h5", "h6"][usize::from((*depth).clamp(1, 6) - 1)];
                self.element(tag, ix, Vec::new())
            }
            MKind::ThematicBreak => Chain::one(self.hast.element("hr", Vec::new(), at)),
            MKind::FootnoteReference(association) => {
                let id = association.identifier.to_uppercase();
                let safe_id = footnote_safe_id(&id);
                let index = match self.footnote_calls.iter().position(|calls| calls.id == id) {
                    Some(index) => index,
                    None => {
                        self.footnote_calls.push(FootnoteCalls { id, count: 0 });
                        self.footnote_calls.len() - 1
                    }
                };
                let calls = &mut self.footnote_calls[index];
                calls.count += 1;
                let suffix = call_suffix(calls.count);
                let counter = index + 1;
                let props = vec![
                    (
                        "href",
                        Val::Str(Cow::Owned(format!("#user-content-fn-{safe_id}"))),
                    ),
                    (
                        "id",
                        Val::Str(Cow::Owned(format!("user-content-fnref-{safe_id}{suffix}"))),
                    ),
                    ("dataFootnoteRef", Val::Bool(true)),
                    ("ariaDescribedBy", Val::Class("footnote-label")),
                ];
                if self.link_depth > 0 {
                    self.raw_boundary = true;
                }
                let label = self.hast.text(Cow::Owned(counter.to_string()));
                let link = self.hast.element_around("a", props, at, label);
                Chain::one(self.hast.element_around("sup", Vec::new(), at, link))
            }
            MKind::Html(value) => {
                self.raw_boundary = true;
                Chain::one(self.hast.push(HKind::Raw(value.clone()), at))
            }
            MKind::Image(image) => {
                self.image(image.url.clone(), image.title.clone(), &image.alt, at)
            }
            MKind::ImageReference(reference) => match self.definition(reference) {
                None => {
                    let suffix = revert_suffix(reference);
                    let alt = &reference.alt;
                    Chain::one(self.hast.text(Cow::Owned(format!("![{alt}{suffix}"))))
                }
                Some(Definition { url, title }) => self.image(url, title, &reference.alt, at),
            },
            MKind::InlineCode(value) => {
                let value = if value.contains(['\n', '\r']) {
                    Cow::Owned(value.replace("\r\n", " ").replace(['\r', '\n'], " "))
                } else {
                    value.clone()
                };
                let text = self.hast.push(HKind::Text(value), at);
                Chain::one(self.hast.element_around("code", Vec::new(), at, text))
            }
            MKind::Link(link) => self.link(ix, link.url.clone(), link.title.clone()),
            MKind::LinkReference(reference) => match self.definition(reference) {
                None => {
                    let suffix = revert_suffix(reference);
                    let mut contents = self.all(ix);
                    let head = (contents.first != NIL)
                        .then(|| &mut self.hast.nodes[contents.first as usize].kind);
                    match head {
                        Some(HKind::Text(value)) => *value = Cow::Owned(format!("[{value}")),
                        _ => {
                            let open = self.hast.text("[");
                            self.hast.prepend(&mut contents, open);
                        }
                    }
                    match &mut self.hast.nodes[contents.last as usize].kind {
                        HKind::Text(value) => push_cow(value, &suffix),
                        _ => {
                            let close = self.hast.text(Cow::Owned(suffix));
                            self.hast.append(&mut contents, close);
                        }
                    }
                    contents
                }
                Some(Definition { url, title }) => self.link(ix, url, title),
            },
            MKind::List { ordered, start, .. } => {
                let tag = if *ordered { "ol" } else { "ul" };
                let mut props = Vec::new();
                if let Some(start) = start.filter(|start| *start != 1) {
                    props.push(("start", Val::Num(i64::from(start))));
                }
                let results = self.all(ix);
                let has_task = self.hast.siblings(results.first).any(|item| {
                    matches!(
                        &self.hast.nodes[item as usize].kind,
                        HKind::Element { tag: "li", props, .. }
                            if matches!(get(props, "className"), Some(Val::Class("task-list-item")))
                    )
                });
                if has_task {
                    props.push(("className", Val::Class("contains-task-list")));
                }
                let children = self.hast.wrap(results, Spacing::Loose);
                Chain::one(self.hast.element_with(tag, props, at, children))
            }
            MKind::ListItem { spread, checked } => self.list_item(ix, parent, *spread, *checked),
            MKind::Table(align) => self.table(ix, align, at),
            MKind::Text(value) => Chain::one(self.hast.push(HKind::Text(trim_lines(value)), at)),
            MKind::Math(math) => {
                self.math = true;
                let text = self.hast.text(math.value.clone());
                let class = Val::List(vec![
                    Cow::Borrowed("language-math"),
                    Cow::Borrowed("math-display"),
                ]);
                let code =
                    self.hast
                        .element_around("code", vec![("className", class)], At::None, text);
                Chain::one(self.hast.element_around("pre", Vec::new(), at, code))
            }
            MKind::InlineMath(value) => {
                self.math = true;
                let text = self.hast.text(value.clone());
                let class = Val::List(vec![
                    Cow::Borrowed("language-math"),
                    Cow::Borrowed("math-inline"),
                ]);
                Chain::one(
                    self.hast
                        .element_around("code", vec![("className", class)], at, text),
                )
            }
            MKind::Definition { .. } | MKind::FootnoteDefinition { .. } => EMPTY,
            // Rows and cells are handled by their table; the unknown handler's `div`.
            MKind::TableRow | MKind::TableCell => self.element("div", ix, Vec::new()),
        }
    }

    fn image(
        &mut self,
        url: Cow<'s, str>,
        title: Option<Cow<'s, str>>,
        alt: &str,
        at: At,
    ) -> Chain {
        let mut props = vec![
            ("src", Val::Str(normalize_uri(url))),
            ("alt", Val::Str(Cow::Owned(alt.to_owned()))),
        ];
        if let Some(title) = title {
            props.push(("title", Val::Str(title)));
        }
        Chain::one(self.hast.element("img", props, at))
    }

    fn link(&mut self, ix: Ix, url: Cow<'s, str>, title: Option<Cow<'s, str>>) -> Chain {
        // Room for the title and rehype-harden's `target` and `rel`.
        let mut props = Vec::with_capacity(4);
        props.push(("href", Val::Str(normalize_uri(url))));
        if let Some(title) = title {
            props.push(("title", Val::Str(title)));
        }
        if self.link_depth > 0 {
            self.raw_boundary = true;
        }
        self.link_depth += 1;
        let chain = self.element("a", ix, props);
        self.link_depth -= 1;
        chain
    }

    fn list_item(&mut self, ix: Ix, parent: Ix, spread: bool, checked: Option<bool>) -> Chain {
        let mut results = self.all(ix);
        let loose = match &self.mdast.node(parent).kind {
            MKind::List {
                spread: list_spread,
                ..
            } => {
                *list_spread
                    || self.mdast.children(parent).any(|item| {
                        matches!(
                            self.mdast.node(item).kind,
                            MKind::ListItem { spread: true, .. }
                        )
                    })
            }
            _ => spread,
        };
        let mut props = Vec::new();
        if let Some(checked) = checked {
            if results.first == NIL || self.hast.tag_name(results.first) != Some("p") {
                let paragraph = self.hast.element("p", Vec::new(), At::None);
                self.hast.prepend(&mut results, paragraph);
            }
            let paragraph = results.first;
            let mut children = self.hast.children_of(paragraph);
            if children.first != NIL {
                let space = self.hast.text(" ");
                self.hast.prepend(&mut children, space);
            }
            let input = self.hast.element(
                "input",
                vec![
                    ("type", Val::Str(Cow::Borrowed("checkbox"))),
                    ("checked", Val::Bool(checked)),
                    ("disabled", Val::Bool(true)),
                ],
                At::None,
            );
            self.hast.prepend(&mut children, input);
            self.hast.set_children(paragraph, children);
            props.push(("className", Val::Class("task-list-item")));
        }
        let tail_is_paragraph =
            results.last != NIL && self.hast.tag_name(results.last) == Some("p");
        let nonempty = results.first != NIL;
        let mut children = EMPTY;
        let mut cursor = results.first;
        while cursor != NIL {
            let next = self.hast.nodes[cursor as usize].next;
            let is_paragraph = self.hast.tag_name(cursor) == Some("p");
            if loose || cursor != results.first || !is_paragraph {
                let newline = self.hast.text("\n");
                self.hast.append(&mut children, newline);
            }
            if is_paragraph && !loose {
                let inner = self.hast.children_of(cursor);
                self.hast.extend(&mut children, inner);
            } else {
                self.hast.append(&mut children, cursor);
            }
            cursor = next;
        }
        if nonempty && (loose || !tail_is_paragraph) {
            let newline = self.hast.text("\n");
            self.hast.append(&mut children, newline);
        }
        Chain::one(
            self.hast
                .element_with("li", props, self.mdast.node(ix).at, children),
        )
    }

    fn table(&mut self, ix: Ix, align: &[AlignKind], at: At) -> Chain {
        let mut rows = EMPTY;
        for (index, row) in self.mdast.children(ix).enumerate() {
            let row = self.table_row(row, index, align);
            self.hast.append(&mut rows, row);
        }
        let mut content = EMPTY;
        let first_row = self.mdast.node(ix).first;
        if rows.first != NIL {
            let first = rows.first;
            let rest = self.hast.nodes[first as usize].next;
            let head_rows = self.hast.one_chain(first);
            let children = self.hast.wrap(head_rows, Spacing::Loose);
            let head = self.hast.element_with(
                "thead",
                Vec::new(),
                self.mdast.node(first_row).at,
                children,
            );
            self.hast.append(&mut content, head);
            if rest != NIL {
                let body_rows = Chain {
                    first: rest,
                    last: rows.last,
                };
                let children = self.hast.wrap(body_rows, Spacing::Loose);
                let body = self
                    .hast
                    .element_with("tbody", Vec::new(), At::None, children);
                // From the second row's start to the last row's end.
                let second = self.mdast.node(first_row).next;
                let (start, _) = edges(self.mdast.node(second).at);
                let (_, end) = edges(self.mdast.node(self.mdast.node(ix).last).at);
                if start != Edge::None && end != Edge::None {
                    self.hast.nodes[body as usize].start = start;
                    self.hast.nodes[body as usize].end = end;
                }
                self.hast.append(&mut content, body);
            }
        }
        let children = self.hast.wrap(content, Spacing::Loose);
        Chain::one(self.hast.element_with("table", Vec::new(), at, children))
    }

    fn table_row(&mut self, row: Ix, index: usize, align: &[AlignKind]) -> Ix {
        let tag = if index == 0 { "th" } else { "td" };
        let mut cells = EMPTY;
        let mut source = self.mdast.node(row).first;
        for alignment in align {
            let value = match alignment {
                AlignKind::Left => Some("left"),
                AlignKind::Right => Some("right"),
                AlignKind::Center => Some("center"),
                AlignKind::None => None,
            };
            let props = value.map_or_else(Vec::new, |value| {
                vec![("align", Val::Str(Cow::Borrowed(value)))]
            });
            let cell = if source == NIL {
                self.hast.element(tag, props, At::None)
            } else {
                let children = self.all(source);
                let cell = self
                    .hast
                    .element_with(tag, props, self.mdast.node(source).at, children);
                source = self.mdast.node(source).next;
                cell
            };
            self.hast.append(&mut cells, cell);
        }
        let children = self.hast.wrap(cells, Spacing::Loose);
        self.hast
            .element_with("tr", Vec::new(), self.mdast.node(row).at, children)
    }

    fn footer(&mut self) -> Option<Ix> {
        let mut items = EMPTY;
        // Rendering a definition can cite footnotes not seen yet, which join the order
        // and get their own item, so the length is read on every pass.
        for reference_index in 0.. {
            let Some(calls) = self.footnote_calls.get(reference_index) else {
                break;
            };
            let Some(&definition) = self.footnotes.get(&calls.id) else {
                continue;
            };
            let safe_id = footnote_safe_id(&calls.id);
            let mut content = self.all(definition);
            // Read after rendering the definition, which can cite its own footnote.
            let count = self.footnote_calls[reference_index].count;
            let mut back = EMPTY;
            for rereference in 1..=count {
                if back.first != NIL {
                    let space = self.hast.text(" ");
                    self.hast.append(&mut back, space);
                }
                let suffix = call_suffix(rereference);
                let arrow = self.hast.text("↩");
                let mut children = Chain::one(arrow);
                if rereference > 1 {
                    let number = self.hast.text(Cow::Owned(rereference.to_string()));
                    let sup = self
                        .hast
                        .element_around("sup", Vec::new(), At::None, number);
                    self.hast.append(&mut children, sup);
                }
                let link = self.hast.element_with(
                    "a",
                    vec![
                        (
                            "href",
                            Val::Str(Cow::Owned(format!("#user-content-fnref-{safe_id}{suffix}"))),
                        ),
                        ("dataFootnoteBackref", Val::Str(Cow::Borrowed(""))),
                        (
                            "ariaLabel",
                            Val::Str(Cow::Owned(format!(
                                "Back to reference {}{suffix}",
                                reference_index + 1
                            ))),
                        ),
                        ("className", Val::Class("data-footnote-backref")),
                    ],
                    At::None,
                    children,
                );
                self.hast.append(&mut back, link);
            }
            if content.last != NIL && self.hast.tag_name(content.last) == Some("p") {
                let tail = content.last;
                let mut inner = self.hast.children_of(tail);
                let tail_text =
                    (inner.last != NIL).then(|| &mut self.hast.nodes[inner.last as usize].kind);
                match tail_text {
                    Some(HKind::Text(value)) => push_cow(value, " "),
                    _ => {
                        let space = self.hast.text(" ");
                        self.hast.append(&mut inner, space);
                    }
                }
                self.hast.extend(&mut inner, back);
                self.hast.set_children(tail, inner);
            } else {
                self.hast.extend(&mut content, back);
            }
            let children = self.hast.wrap(content, Spacing::Loose);
            let li = self.hast.element_with(
                "li",
                vec![(
                    "id",
                    Val::Str(Cow::Owned(format!("user-content-fn-{safe_id}"))),
                )],
                self.mdast.node(definition).at,
                children,
            );
            self.hast.append(&mut items, li);
        }
        if items.first == NIL {
            return None;
        }
        let label_text = self.hast.text("Footnotes");
        let label = self.hast.element_around(
            "h2",
            vec![
                ("className", Val::Class("sr-only")),
                ("id", Val::Str(Cow::Borrowed("footnote-label"))),
            ],
            At::None,
            label_text,
        );
        let list_children = self.hast.wrap(items, Spacing::Loose);
        let list = self
            .hast
            .element_with("ol", Vec::new(), At::None, list_children);
        let mut children = Chain::one(label);
        let newline = self.hast.text("\n");
        self.hast.append(&mut children, newline);
        self.hast.append(&mut children, list);
        let newline = self.hast.text("\n");
        self.hast.append(&mut children, newline);
        Some(self.hast.element_with(
            "section",
            vec![
                ("dataFootnotes", Val::Bool(true)),
                ("className", Val::Class("footnotes")),
            ],
            At::None,
            children,
        ))
    }
}

/// HAST plus what the native raw stage must leave to JavaScript.
pub struct Converted<'s> {
    pub tree: HTree<'s>,
    pub root: Ix,
    pub raw_boundary: bool,
    pub math: bool,
}

pub fn to_hast<'s>(mdast: &MTree<'s>) -> Converted<'s> {
    let mut state = ToHast {
        mdast,
        hast: HTree {
            // Prose and chat Markdown make 1.3 to 1.8 HAST nodes per MDAST node.
            nodes: Vec::with_capacity(mdast.nodes.len() * 7 / 4 + 16),
            quirks: false,
        },
        definitions: HashMap::new(),
        footnotes: HashMap::new(),
        footnote_calls: Vec::new(),
        raw_boundary: false,
        link_depth: 0,
        math: false,
    };
    state.collect(MTree::ROOT);
    let root = state.one(MTree::ROOT, MTree::ROOT).first;
    if let Some(section) = state.footer() {
        let mut children = state.hast.children_of(root);
        let newline = state.hast.text("\n");
        state.hast.append(&mut children, newline);
        state.hast.append(&mut children, section);
        state.hast.set_children(root, children);
    }
    Converted {
        root,
        raw_boundary: state.raw_boundary,
        math: state.math,
        tree: state.hast,
    }
}

// ---------------------------------------------------------------------------
// rehype-raw 7.0.0 (parse5 7.3.0 via hast-util-raw 9.1.0) property round trip
// ---------------------------------------------------------------------------

/// Whether splitting `value` on ASCII whitespace gives just `value`.
fn one_token(value: &str) -> bool {
    !value.is_empty() && !value.bytes().any(|byte| byte.is_ascii_whitespace())
}

/// hast-util-to-parse5 attribute serialization then hastscript property parsing.
fn round_trip<'s>(key: &str, value: Val<'s>) -> Option<Val<'s>> {
    let space_separated = matches!(key, "className" | "ariaDescribedBy");
    let attribute: Cow<'s, str> = match value {
        Val::Class(name) if space_separated => return Some(Val::Class(name)),
        Val::Class(name) => return Some(Val::Str(Cow::Borrowed(name))),
        // Joining then splitting gives the list back.
        Val::List(list) if space_separated && list.iter().all(|item| one_token(item)) => {
            return Some(Val::List(list));
        }
        Val::Bool(false) => return None,
        Val::Bool(true) => Cow::Borrowed(""),
        Val::Num(number) => Cow::Owned(number.to_string()),
        Val::Str(string) => string,
        Val::List(mut list) if list.len() == 1 => list.pop().expect("the guard saw one item"),
        Val::List(list) => Cow::Owned(list.join(" ")),
    };
    Some(match key {
        "checked" | "disabled" => Val::Bool(true),
        "start" => match attribute.trim().parse::<i64>() {
            Ok(number) => Val::Num(number),
            Err(_) => Val::Str(attribute),
        },
        _ if space_separated => {
            if one_token(&attribute) {
                Val::List(vec![attribute])
            } else {
                Val::List(
                    attribute
                        .split(|c: char| c.is_ascii_whitespace())
                        .filter(|part| !part.is_empty())
                        .map(|part| Cow::Owned(part.to_owned()))
                        .collect(),
                )
            }
        }
        _ => Val::Str(attribute),
    })
}

// ---------------------------------------------------------------------------
// rehype-sanitize 6.0.0 (hast-util-sanitize 5.0.2) with the default schema
// ---------------------------------------------------------------------------

enum Allowed {
    Any,
    Values(&'static [&'static str]),
    /// `['disabled', true]`: only the boolean `true`.
    True,
    /// `/^language-./` plus the listed values.
    Language(&'static [&'static str]),
}

const ARIA: [&str; 3] = ["ariaDescribedBy", "ariaLabel", "ariaLabelledBy"];
const GLOBAL: &[&str] = &[
    "abbr",
    "accept",
    "acceptCharset",
    "accessKey",
    "action",
    "align",
    "alt",
    "axis",
    "border",
    "cellPadding",
    "cellSpacing",
    "char",
    "charOff",
    "charSet",
    "checked",
    "clear",
    "colSpan",
    "color",
    "cols",
    "compact",
    "coords",
    "dateTime",
    "dir",
    "encType",
    "frame",
    "hSpace",
    "headers",
    "height",
    "hrefLang",
    "htmlFor",
    "id",
    "isMap",
    "itemProp",
    "label",
    "lang",
    "maxLength",
    "media",
    "method",
    "multiple",
    "name",
    "noHref",
    "noShade",
    "noWrap",
    "open",
    "prompt",
    "readOnly",
    "rev",
    "rowSpan",
    "rows",
    "rules",
    "scope",
    "selected",
    "shape",
    "size",
    "span",
    "start",
    "summary",
    "tabIndex",
    "title",
    "useMap",
    "vAlign",
    "value",
    "width",
];
const TAG_NAMES: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "input",
    "ins",
    "kbd",
    "li",
    "ol",
    "p",
    "picture",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "source",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "tt",
    "ul",
    "var",
];

fn specific(tag: &str, key: &str) -> Option<Allowed> {
    let aria = ARIA.contains(&key);
    Some(match (tag, key) {
        ("a", _) if aria => Allowed::Any,
        ("a", "dataFootnoteBackref" | "dataFootnoteRef" | "href") => Allowed::Any,
        ("a", "className") => Allowed::Values(&["data-footnote-backref"]),
        ("blockquote" | "del" | "ins" | "q", "cite") => Allowed::Any,
        ("code", "className") => Allowed::Language(&[]),
        ("div", "itemScope" | "itemType") => Allowed::Any,
        ("dl" | "summary" | "table", _) if aria => Allowed::Any,
        ("h2", "className") => Allowed::Values(&["sr-only"]),
        ("img", _) if aria => Allowed::Any,
        ("img", "longDesc" | "src") => Allowed::Any,
        ("input", "disabled") => Allowed::True,
        ("input", "type") => Allowed::Values(&["checkbox"]),
        ("li", "className") => Allowed::Values(&["task-list-item"]),
        ("ol" | "ul", _) if aria => Allowed::Any,
        ("ol" | "ul", "className") => Allowed::Values(&["contains-task-list"]),
        ("section", "dataFootnotes") => Allowed::Any,
        ("section", "className") => Allowed::Values(&["footnotes"]),
        ("source", "srcSet") => Allowed::Any,
        _ => return None,
    })
}

fn safe_protocol(key: &str, url: &str) -> bool {
    let protocols: &[&str] = match key {
        "cite" | "longDesc" | "src" => &["http", "https"],
        "href" => &["http", "https", "irc", "ircs", "mailto", "xmpp"],
        _ => return true,
    };
    // JavaScript indices are UTF-16, but only their order and the ASCII prefix matter.
    let Some(colon) = url.find(':') else {
        return true;
    };
    let before = |c: char| url.find(c).is_some_and(|at| colon > at);
    if before('/') || before('?') || before('#') {
        return true;
    }
    protocols
        .iter()
        .any(|protocol| colon == protocol.len() && url.starts_with(protocol))
}

fn primitive_ok(allowed: &Allowed, key: &str, value: &Val<'_>) -> bool {
    let string: Cow<'_, str> = match value {
        Val::Str(string) => Cow::Borrowed(string.as_ref()),
        Val::Num(number) => Cow::Owned(number.to_string()),
        Val::Bool(boolean) => Cow::Borrowed(if *boolean { "true" } else { "false" }),
        Val::List(_) | Val::Class(_) => return false,
    };
    if !safe_protocol(key, &string) {
        return false;
    }
    match allowed {
        Allowed::Any => true,
        Allowed::True => matches!(value, Val::Bool(true)),
        Allowed::Values(values) => {
            matches!(value, Val::Str(string) if values.contains(&string.as_ref()))
        }
        Allowed::Language(values) => {
            let language = string
                .strip_prefix("language-")
                .and_then(|rest| rest.chars().next())
                .is_some_and(|first| !matches!(first, '\n' | '\r' | '\u{2028}' | '\u{2029}'));
            language || matches!(value, Val::Str(string) if values.contains(&string.as_ref()))
        }
    }
}

fn clobbered(key: &str) -> bool {
    matches!(key, "ariaDescribedBy" | "ariaLabelledBy" | "id" | "name")
}

fn clobber(value: Val<'_>) -> Val<'_> {
    match value {
        Val::Str(string) => Val::Str(Cow::Owned(format!("user-content-{string}"))),
        Val::Num(number) => Val::Str(Cow::Owned(format!("user-content-{number}"))),
        Val::Bool(boolean) => Val::Str(Cow::Owned(format!("user-content-{boolean}"))),
        Val::List(list) => Val::List(
            list.into_iter()
                .map(|item| Cow::Owned(format!("user-content-{item}")))
                .collect(),
        ),
        Val::Class(name) => Val::List(vec![Cow::Owned(format!("user-content-{name}"))]),
    }
}

/// `Err` hands the value back so the `*` definitions can try it.
fn sanitize_value<'s>(allowed: &Allowed, key: &str, value: Val<'s>) -> Result<Val<'s>, Val<'s>> {
    if let Val::Class(name) = value {
        return Ok(
            if primitive_ok(allowed, key, &Val::Str(Cow::Borrowed(name))) {
                Val::Class(name)
            } else {
                Val::List(Vec::new())
            },
        );
    }
    match value {
        Val::List(list) => Ok(Val::List(
            list.into_iter()
                .filter(|item| primitive_ok(allowed, key, &Val::Str(Cow::Borrowed(item))))
                .collect(),
        )),
        primitive if primitive_ok(allowed, key, &primitive) => Ok(primitive),
        primitive => Err(primitive),
    }
}

fn sanitize_props<'s>(
    tag: &str,
    props: Props<'s>,
    code_classes: &'static [&'static str],
) -> Props<'s> {
    // Collecting into the same element type reuses the input's allocation.
    let mut result: Props<'s> = props
        .into_iter()
        .filter_map(|(key, value)| {
            let allowed = match (tag, key) {
                ("code", "className") => Some(Allowed::Language(code_classes)),
                _ => specific(tag, key),
            };
            let value = match allowed {
                Some(allowed) => sanitize_value(&allowed, key, value),
                None => Err(value),
            };
            let safe = match value {
                Ok(safe) => safe,
                Err(value) if GLOBAL.contains(&key) => {
                    sanitize_value(&Allowed::Any, key, value).ok()?
                }
                Err(_) => return None,
            };
            Some((key, if clobbered(key) { clobber(safe) } else { safe }))
        })
        .collect();
    if tag == "input" {
        if get(&result, "disabled").is_none() {
            result.push(("disabled", Val::Bool(true)));
        }
        if get(&result, "type").is_none() {
            result.push(("type", Val::Str(Cow::Borrowed("checkbox"))));
        }
    }
    result
}

// ---------------------------------------------------------------------------
// rehype-harden 1.1.8 with Streamdown's options
// ---------------------------------------------------------------------------

fn path_relative(url: &str) -> bool {
    url.starts_with('/') || url.starts_with("./") || url.starts_with("../")
}

/// The base rehype-harden resolves relative URLs against.
const HARDEN_BASE: &str = "http://example.com";

/// The two kinds of element rehype-harden checks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resource {
    Link,
    Image,
}

/// Why `transformUrl` returned null.
struct Blocked;

/// `transformUrl`; `Ok(None)` when the URL comes back unchanged.
fn transform_url(url: &str, resource: Resource) -> Result<Option<String>, Blocked> {
    let image = resource == Resource::Image;
    if url.is_empty() {
        return Err(Blocked);
    }
    if url.starts_with('#')
        && !image
        && Url::parse_with_base(url, HARDEN_BASE).is_some_and(|test| test.hash() == url)
    {
        return Ok(None);
    }
    if url.starts_with("data:") {
        return if image && url.starts_with("data:image/") {
            Ok(None)
        } else {
            Err(Blocked)
        };
    }
    if let Some(after) = url.strip_prefix("blob:") {
        let parsed = Url::parse(url).ok_or(Blocked)?;
        return if parsed.scheme() == "blob" && !after.is_empty() && after != "invalid" {
            Ok(None)
        } else {
            Err(Blocked)
        };
    }
    // A path-relative URL has no scheme, so only the base can parse it; skipping the
    // absolute attempt matters in WebAssembly, where a failed `new URL` throws.
    let parsed = if path_relative(url) {
        Url::parse_with_base(url, HARDEN_BASE).ok_or(Blocked)?
    } else {
        Url::parse(url).ok_or(Blocked)?
    };
    if matches!(parsed.scheme(), "javascript" | "data" | "file" | "vbscript") {
        return Err(Blocked);
    }
    let result = if matches!(parsed.scheme(), "http" | "https") && path_relative(url) {
        format!("{}{}{}", parsed.pathname(), parsed.search(), parsed.hash())
    } else {
        parsed.into_href()
    };
    Ok((result != url).then_some(result))
}

/// What rehype-harden puts in place of a link or image whose URL it rejects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlockPolicy {
    /// A span marking the block (`[blocked]` after a link's text, `[Image blocked: alt]`).
    #[default]
    Indicator,
    /// A bare span of the link's text or the image's alt (none without alt).
    TextOnly,
    /// Nothing.
    Remove,
}

/// rehype-harden's configurable options. The rest stay at Streamdown's values: any link
/// or image prefix, any protocol, data images allowed, no default origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Harden {
    pub link_policy: BlockPolicy,
    pub image_policy: BlockPolicy,
    pub blocked_link_class: String,
    pub blocked_image_class: String,
}

impl Default for Harden {
    fn default() -> Self {
        Harden {
            link_policy: BlockPolicy::Indicator,
            image_policy: BlockPolicy::Indicator,
            blocked_link_class: "text-gray-500".to_owned(),
            blocked_image_class: "inline-block bg-gray-200 dark:bg-gray-700 text-gray-600 \
                                  dark:text-gray-400 px-3 py-1 rounded text-sm"
                .to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Fused walk: rehype-raw (raw-free trees), sanitize, harden
// ---------------------------------------------------------------------------

struct Finisher<'a, 's> {
    tree: &'a mut HTree<'s>,
    /// `false`: KaTeX runs first, so sanitize and harden wait for JavaScript too.
    sanitize: bool,
    code_classes: &'static [&'static str],
    /// `None` with math: KaTeX runs before sanitize and harden, in JavaScript.
    harden: Option<&'a Harden>,
    /// How many `table` elements enclose the current one.
    table_depth: usize,
}

fn table_section(tag: &str) -> bool {
    matches!(tag, "table" | "thead" | "tbody" | "tfoot" | "tr")
}

impl<'s> Finisher<'_, 's> {
    /// Removes text parse5 foster-parents out of table structure, in document order.
    fn take_foster(&mut self, element: Ix, out: &mut Vec<Ix>) {
        let mut kept = EMPTY;
        let mut cursor = self.tree.nodes[element as usize].first;
        while cursor != NIL {
            let next = self.tree.nodes[cursor as usize].next;
            match &self.tree.nodes[cursor as usize].kind {
                HKind::Text(_) => out.push(cursor),
                HKind::Element { tag, .. } if table_section(tag) => {
                    self.take_foster(cursor, out);
                    self.tree.append(&mut kept, cursor);
                }
                _ => self.tree.append(&mut kept, cursor),
            }
            cursor = next;
        }
        self.tree.set_children(element, kept);
    }

    /// Appends `ix`, merging adjacent text with parse5's location rule: the merged node
    /// keeps the first start and takes the last end.
    fn push_merged(&mut self, level: &mut Chain, ix: Ix) {
        let mergeable = level.last != NIL
            && matches!(self.tree.nodes[ix as usize].kind, HKind::Text(_))
            && matches!(self.tree.nodes[level.last as usize].kind, HKind::Text(_));
        if !mergeable {
            self.tree.append(level, ix);
            return;
        }
        let end = self.tree.nodes[ix as usize].end;
        let HKind::Text(value) = &mut self.tree.nodes[ix as usize].kind else {
            unreachable!("mergeable nodes are text");
        };
        let value = std::mem::take(value);
        let into = &mut self.tree.nodes[level.last as usize];
        let HKind::Text(existing) = &mut into.kind else {
            unreachable!("mergeable nodes are text");
        };
        push_cow(existing, &value);
        into.end = end;
    }

    fn children(&mut self, parent: Ix) {
        // parse5: fostered table text and adjacent text merge at this level.
        let mut level = EMPTY;
        let mut cursor = self.tree.nodes[parent as usize].first;
        let mut fostered = Vec::new();
        while cursor != NIL {
            let next = self.tree.nodes[cursor as usize].next;
            if self.tree.tag_name(cursor) == Some("table") {
                fostered.clear();
                self.take_foster(cursor, &mut fostered);
                for &text in &fostered {
                    self.push_merged(&mut level, text);
                }
            }
            self.push_merged(&mut level, cursor);
            cursor = next;
        }
        // Then every node becomes final, splicing replacements into `out`.
        let mut out = EMPTY;
        let mut cursor = level.first;
        while cursor != NIL {
            let next = self.tree.nodes[cursor as usize].next;
            match self.tree.nodes[cursor as usize].kind {
                HKind::Text(_) => self.text(cursor, &mut out),
                HKind::Element { .. } => self.element(cursor, &mut out),
                // `raw_boundary` keeps raw nodes out of this walk.
                HKind::Raw(_) | HKind::Root => {}
            }
            cursor = next;
        }
        self.tree.set_children(parent, out);
    }

    fn text(&mut self, ix: Ix, out: &mut Chain) {
        if self.sanitize {
            self.tree.nodes[ix as usize].drop_partial_position();
        }
        self.tree.append(out, ix);
    }

    fn element(&mut self, ix: Ix, out: &mut Chain) {
        let HKind::Element { tag, props, data } = &mut self.tree.nodes[ix as usize].kind else {
            unreachable!("children() sends only elements here");
        };
        let tag = *tag;
        *data = None;
        *props = std::mem::take(props)
            .into_iter()
            .filter_map(|(key, value)| round_trip(key, value).map(|value| (key, value)))
            .collect();
        let table = usize::from(tag == "table");
        self.table_depth += table;
        self.children(ix);
        self.table_depth -= table;
        if !self.sanitize {
            // KaTeX first: sanitize and harden run in JavaScript after it.
            self.tree.append(out, ix);
            return;
        }
        let mut safe = TAG_NAMES.contains(&tag);
        if safe && matches!(tag, "tbody" | "td" | "th" | "thead" | "tfoot" | "tr") {
            safe = self.table_depth > 0;
        }
        if !safe {
            if tag != "script" {
                let children = self.tree.children_of(ix);
                self.tree.extend(out, children);
            }
            return;
        }
        self.tree.nodes[ix as usize].drop_partial_position();
        let props = self.tree.properties_mut(ix).expect("an element");
        *props = sanitize_props(tag, std::mem::take(props), self.code_classes);
        if self
            .harden
            .is_none_or(|harden| self.harden(ix, tag, harden))
        {
            self.tree.append(out, ix);
        }
    }

    /// `rehype-harden` for one sanitized element whose children are final; `false`
    /// removes it.
    fn harden(&mut self, ix: Ix, tag: &'static str, harden: &Harden) -> bool {
        let (key, resource) = match tag {
            "a" => ("href", Resource::Link),
            "img" => ("src", Resource::Image),
            _ => return true,
        };
        let props = self.tree.properties_mut(ix).expect("an element");
        let transformed = match get(props, key) {
            Some(Val::Str(url)) => transform_url(url, resource),
            _ => Err(Blocked),
        };
        let changed = match transformed {
            Ok(changed) => changed,
            Err(Blocked) => return self.blocked(ix, resource, harden),
        };
        if let Some(changed) = changed {
            set(props, key, Val::Str(Cow::Owned(changed)));
        }
        if resource == Resource::Link {
            set(props, "target", Val::Str(Cow::Borrowed("_blank")));
            set(props, "rel", Val::Str(Cow::Borrowed("noopener noreferrer")));
        }
        true
    }

    /// harden's `resolveLinkBlockPolicy` / `resolveImageBlockPolicy`: the replacement is a
    /// new, unpositioned span.
    fn blocked(&mut self, ix: Ix, resource: Resource, harden: &Harden) -> bool {
        let policy = match resource {
            Resource::Link => harden.link_policy,
            Resource::Image => harden.image_policy,
        };
        if policy == BlockPolicy::Remove {
            return false;
        }
        let shown = |value: Option<&Val<'_>>| match value {
            Some(Val::Str(text)) => text.to_string(),
            Some(Val::Num(number)) => number.to_string(),
            Some(Val::Bool(boolean)) => boolean.to_string(),
            _ => "undefined".to_owned(),
        };
        let props = std::mem::take(self.tree.properties_mut(ix).expect("an element"));
        let (props, children): (Props<'s>, Option<Ix>) = match (resource, policy) {
            (Resource::Link, BlockPolicy::TextOnly) => (Vec::new(), None),
            (Resource::Link, _) => {
                let title = format!("Blocked URL: {}", shown(get(&props, "href")));
                let suffix = self.tree.text(Cow::Borrowed(" [blocked]"));
                (
                    vec![
                        ("title", Val::Str(Cow::Owned(title))),
                        (
                            "class",
                            Val::Str(Cow::Owned(harden.blocked_link_class.clone())),
                        ),
                    ],
                    Some(suffix),
                )
            }
            (Resource::Image, policy) => {
                // `String(node.properties.alt || fallback)`.
                let alt = match get(&props, "alt") {
                    Some(Val::Str(alt)) if !alt.is_empty() => Some(alt.to_string()),
                    Some(Val::Num(number)) if *number != 0 => Some(number.to_string()),
                    Some(Val::Bool(true)) => Some("true".to_owned()),
                    _ => None,
                };
                let (text, props) = if policy == BlockPolicy::TextOnly {
                    let Some(alt) = alt else {
                        return false;
                    };
                    (alt, Vec::new())
                } else {
                    let alt = alt.unwrap_or_else(|| "No description".to_owned());
                    (
                        format!("[Image blocked: {alt}]"),
                        vec![(
                            "class",
                            Val::Str(Cow::Owned(harden.blocked_image_class.clone())),
                        )],
                    )
                };
                let label = self.tree.text(Cow::Owned(text));
                self.tree.set_children(ix, Chain::one(label));
                (props, None)
            }
        };
        if let Some(suffix) = children {
            let mut children = self.tree.children_of(ix);
            self.tree.append(&mut children, suffix);
            self.tree.set_children(ix, children);
        }
        let node = &mut self.tree.nodes[ix as usize];
        node.kind = HKind::Element {
            tag: "span",
            props,
            data: None,
        };
        node.start = Edge::None;
        node.end = Edge::None;
        true
    }
}

// ---------------------------------------------------------------------------
// HAST JSON
// ---------------------------------------------------------------------------

struct Json<'a, 's> {
    tree: &'a HTree<'s>,
    points: &'a Points,
    out: String,
}

impl Json<'_, '_> {
    fn point(&mut self, edge: Edge) {
        let Some(pt) = edge.point(self.points) else {
            return;
        };
        self.out.push_str("{\"line\":");
        json_u32(&mut self.out, pt.line);
        self.out.push_str(",\"column\":");
        json_u32(&mut self.out, pt.column);
        self.out.push_str(",\"offset\":");
        json_u32(&mut self.out, pt.offset);
        self.out.push('}');
    }

    fn position(&mut self, start: Edge, end: Edge) {
        if start == Edge::None && end == Edge::None {
            return;
        }
        self.out.push_str(",\"position\":{");
        if start != Edge::None {
            self.out.push_str("\"start\":");
            self.point(start);
            if end != Edge::None {
                self.out.push(',');
            }
        }
        if end != Edge::None {
            self.out.push_str("\"end\":");
            self.point(end);
        }
        self.out.push('}');
    }

    fn children(&mut self, ix: Ix) {
        self.out.push('[');
        let tree = self.tree;
        for (index, child) in tree.children(ix).enumerate() {
            if index > 0 {
                self.out.push(',');
            }
            self.node(child);
        }
        self.out.push(']');
    }

    fn value(&mut self, value: &Val<'_>) {
        match value {
            Val::Str(string) => json_string(&mut self.out, string),
            Val::Bool(boolean) => self.out.push_str(if *boolean { "true" } else { "false" }),
            Val::Num(number) => {
                let _ = write!(self.out, "{number}");
            }
            Val::Class(name) => {
                self.out.push_str("[\"");
                self.out.push_str(name);
                self.out.push_str("\"]");
            }
            Val::List(list) => {
                self.out.push('[');
                for (index, item) in list.iter().enumerate() {
                    if index > 0 {
                        self.out.push(',');
                    }
                    json_string(&mut self.out, item);
                }
                self.out.push(']');
            }
        }
    }

    fn node(&mut self, ix: Ix) {
        let tree = self.tree;
        let node = &tree.nodes[ix as usize];
        match &node.kind {
            HKind::Root => {
                self.out.push_str("{\"type\":\"root\",\"children\":");
                self.children(ix);
                if tree.quirks {
                    self.out.push_str(",\"data\":{\"quirksMode\":false}");
                }
            }
            HKind::Element { tag, props, data } => {
                self.out.push_str("{\"type\":\"element\",\"tagName\":\"");
                self.out.push_str(tag);
                self.out.push_str("\",\"properties\":{");
                for (index, (key, value)) in props.iter().enumerate() {
                    if index > 0 {
                        self.out.push(',');
                    }
                    self.out.push('"');
                    self.out.push_str(key);
                    self.out.push_str("\":");
                    self.value(value);
                }
                self.out.push_str("},\"children\":");
                self.children(ix);
                if let Some(data) = data {
                    self.out.push_str(",\"data\":{\"meta\":");
                    json_string(&mut self.out, &data.meta);
                    self.out.push('}');
                }
            }
            HKind::Text(value) => {
                self.out.push_str("{\"type\":\"text\",\"value\":");
                json_string(&mut self.out, value);
            }
            HKind::Raw(value) => {
                self.out.push_str("{\"type\":\"raw\",\"value\":");
                json_string(&mut self.out, value);
            }
        }
        self.position(node.start, node.end);
        self.out.push('}');
    }
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// rehype-sanitize's schema: the default, plus `className` values `<code>` may keep beyond
/// `language-*`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sanitize {
    /// For example `["math", "math-display", "math-inline"]`, so rehype-katex can still
    /// find remark-math's code after sanitizing.
    pub code_classes: &'static [&'static str],
}

/// The pipeline's settings. The default is Streamdown's.
#[derive(Default)]
pub struct Options {
    pub harden: Harden,
    pub sanitize: Sanitize,
    /// Run rehype-katex after rehype-sanitize instead of before it (Streamdown's order):
    /// math documents then come back sanitized, with only KaTeX and harden left to run.
    pub katex_after_sanitize: bool,
    pub plugins: Vec<Box<dyn Plugin>>,
}

/// What a [`Plugin`] may read besides the tree.
pub struct Context<'a> {
    /// The Markdown, with NUL replaced.
    pub source: &'a str,
    pub points: &'a Points,
}

impl Context<'_> {
    /// A node edge's offset in UTF-16 units, as JavaScript counts it.
    #[must_use]
    pub fn offset(&self, edge: Edge) -> Option<u32> {
        edge.point(self.points).map(|pt| pt.offset)
    }
}

/// A transform at a fixed point of the pipeline: rill's remark or rehype plugin.
///
/// The hooks see a tree only when rill finishes it: a document with raw HTML stops before
/// `hast_unsanitized` (parse5 runs in JavaScript), and with math `hast` waits for KaTeX.
/// Run the JavaScript form of a plugin in those tails (see [`Kind`]).
pub trait Plugin {
    /// After parsing, before mdast-util-to-hast: where remark plugins run.
    fn mdast(&self, _tree: &mut MTree<'_>) -> Result<(), Error> {
        Ok(())
    }

    /// Before rehype-sanitize: a rehype plugin placed ahead of it.
    fn hast_unsanitized(&self, _tree: &mut HTree<'_>, _root: Ix, _context: &Context<'_>) {}

    /// The final tree: a rehype plugin after the chain.
    fn hast(&self, _tree: &mut HTree<'_>, _root: Ix, _context: &Context<'_>) {}
}

/// How far [`render`] took the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    /// Final HAST: Streamdown's whole rehype chain ran.
    Complete,
    /// HAST before `rehype-raw`: raw HTML (or a link in a link) needs parse5. Run
    /// rehype-raw, rehype-katex, rehype-sanitize and rehype-harden on it in JavaScript.
    Raw,
    /// HAST after `rehype-raw` with math: Streamdown runs KaTeX before sanitizing, so
    /// rehype-katex, rehype-sanitize and rehype-harden run in JavaScript.
    Math,
}

impl Kind {
    /// The name the JavaScript package uses: `"complete"`, `"raw"` or `"math"`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Complete => "complete",
            Kind::Raw => "raw",
            Kind::Math => "math",
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A rendered document: how far rill took it and the HAST as JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub kind: Kind,
    /// The HAST root, as JSON.
    pub tree: String,
}

/// Markdown → Streamdown's HAST, as JSON.
pub fn render(source: &str, options: &Options) -> Result<Rendered, Error> {
    let source = mdast::without_nul(source);
    let built = build(&source, options)?;
    let tree = built.json(String::with_capacity(built.json_capacity(&source)));
    Ok(Rendered {
        kind: built.kind,
        tree,
    })
}

/// `{"kind":"complete"|"raw"|"math","tree":HAST}`.
pub fn render_json(source: &str, options: &Options) -> Result<String, Error> {
    let source = mdast::without_nul(source);
    let built = build(&source, options)?;
    let mut out = String::with_capacity(built.json_capacity(&source) + 32);
    out.push_str("{\"kind\":\"");
    out.push_str(built.kind.as_str());
    out.push_str("\",\"tree\":");
    let mut out = built.json(out);
    out.push('}');
    Ok(out)
}

// ---------------------------------------------------------------------------
// Opcode transfer: the same HAST as `render_json` without JSON text. Text that borrows
// the source crosses as UTF-16 offsets for JavaScript to slice from the string it holds;
// generated strings go through one table.
// ---------------------------------------------------------------------------

pub mod op {
    pub const ROOT: u32 = 0;
    pub const ELEMENT: u32 = 1;
    pub const TEXT: u32 = 2;
    pub const RAW: u32 = 3;
    /// An unpositioned `"\n"` text node (`wrap` emits many).
    pub const NEWLINE: u32 = 4;
    pub const SOURCE: u32 = 0;
    pub const TABLE: u32 = 1;
    pub const STRING: u32 = 0;
    pub const TRUE: u32 = 1;
    pub const FALSE: u32 = 2;
    pub const NUMBER: u32 = 3;
    pub const LIST: u32 = 4;
}

pub struct Encoded {
    pub kind: Kind,
    pub ops: Vec<u32>,
    pub strings: String,
}

struct Encoder<'a, 's> {
    tree: &'a HTree<'s>,
    points: &'a Points,
    source: &'s str,
    ops: Vec<u32>,
    strings: String,
    units: u32,
    interned: HashMap<&'static str, (u32, u32)>,
}

impl Encoder<'_, '_> {
    fn table(&mut self, value: &str) -> (u32, u32) {
        let start = self.units;
        self.strings.push_str(value);
        // Every byte that starts a character is one unit; four-byte ones are two.
        self.units += value
            .bytes()
            .map(|byte| u32::from(byte & 0xC0 != 0x80) + u32::from(byte >= 0xF0))
            .sum::<u32>();
        (start, self.units)
    }

    fn string(&mut self, value: &str) {
        let offset = (value.as_ptr() as usize).checked_sub(self.source.as_ptr() as usize);
        if let Some(start) = offset.filter(|start| start + value.len() <= self.source.len()) {
            self.ops.push(op::SOURCE);
            self.ops.push(self.points.units(start as u32));
            self.ops
                .push(self.points.units((start + value.len()) as u32));
            return;
        }
        let (start, end) = self.table(value);
        self.ops.extend([op::TABLE, start, end]);
    }

    fn name(&mut self, value: &'static str) {
        let (start, end) = match self.interned.get(value) {
            Some(range) => *range,
            None => {
                let range = self.table(value);
                self.interned.insert(value, range);
                range
            }
        };
        self.ops.extend([op::TABLE, start, end]);
    }

    fn point(&mut self, edge: Edge) {
        let Some(pt) = edge.point(self.points) else {
            return;
        };
        self.ops.extend([pt.line, pt.column, pt.offset]);
    }

    fn position(&mut self, start: Edge, end: Edge) {
        let flags = u32::from(start != Edge::None) | (u32::from(end != Edge::None) << 1);
        self.ops.push(flags);
        self.point(start);
        self.point(end);
    }

    /// The child count, then the children: the count is patched in after them.
    fn children(&mut self, ix: Ix) {
        let count_slot = self.ops.len();
        self.ops.push(0);
        let mut count = 0;
        let tree = self.tree;
        for child in tree.children(ix) {
            self.node(child);
            count += 1;
        }
        self.ops[count_slot] = count;
    }

    fn value(&mut self, value: &Val<'_>) {
        match value {
            Val::Str(string) => {
                self.ops.push(op::STRING);
                self.string(string);
            }
            Val::Bool(true) => self.ops.push(op::TRUE),
            Val::Bool(false) => self.ops.push(op::FALSE),
            Val::Num(number) => self.ops.extend([op::NUMBER, *number as u32]),
            Val::Class(name) => {
                self.ops.extend([op::LIST, 1]);
                self.name(name);
            }
            Val::List(list) => {
                self.ops.extend([op::LIST, list.len() as u32]);
                for item in list {
                    self.string(item);
                }
            }
        }
    }

    fn node(&mut self, ix: Ix) {
        let tree = self.tree;
        let node = &tree.nodes[ix as usize];
        match &node.kind {
            HKind::Root => {
                self.ops.extend([op::ROOT, u32::from(tree.quirks)]);
                self.children(ix);
            }
            HKind::Element { tag, props, data } => {
                self.ops.push(op::ELEMENT);
                self.name(tag);
                self.ops.push(props.len() as u32);
                for (key, value) in props {
                    self.name(key);
                    self.value(value);
                }
                match data {
                    Some(data) => {
                        self.ops.push(1);
                        self.string(&data.meta);
                    }
                    None => self.ops.push(0),
                }
                self.children(ix);
            }
            HKind::Text(value)
                if value == "\n" && node.start == Edge::None && node.end == Edge::None =>
            {
                self.ops.push(op::NEWLINE);
                return;
            }
            HKind::Text(value) => {
                self.ops.push(op::TEXT);
                self.string(value);
            }
            HKind::Raw(value) => {
                self.ops.push(op::RAW);
                self.string(value);
            }
        }
        self.position(node.start, node.end);
    }
}

/// A rendered document before serialization, with the MDAST's `points` for its positions.
struct Built<'s> {
    kind: Kind,
    points: Points,
    tree: HTree<'s>,
    root: Ix,
}

impl Built<'_> {
    /// The HAST as JSON, appended to `out`.
    fn json(&self, out: String) -> String {
        let mut json = Json {
            tree: &self.tree,
            points: &self.points,
            out,
        };
        json.node(self.root);
        json.out
    }

    /// Room for the JSON: the text, plus up to about 110 bytes of syntax and position per
    /// node.
    fn json_capacity(&self, source: &str) -> usize {
        source.len() + self.tree.nodes.len() * 112
    }
}

fn build<'s>(source: &'s str, options: &Options) -> Result<Built<'s>, Error> {
    let mut mdast = mdast::parse_arena(source)?;
    for plugin in &options.plugins {
        plugin.mdast(&mut mdast)?;
    }
    let Converted {
        mut tree,
        root,
        raw_boundary,
        math,
    } = to_hast(&mdast);
    // HAST owns or borrows the source for every string, so only the positions stay.
    let MTree { points, .. } = mdast;
    if raw_boundary {
        return Ok(Built {
            kind: Kind::Raw,
            points,
            tree,
            root,
        });
    }
    let context = Context {
        source,
        points: &points,
    };
    for plugin in &options.plugins {
        plugin.hast_unsanitized(&mut tree, root, &context);
    }
    if tree.nodes[root as usize].first == NIL {
        // parse5 locates a fragment root only through its child tokens.
        tree.nodes[root as usize].start = Edge::None;
        tree.nodes[root as usize].end = Edge::None;
    }
    tree.quirks = true;
    let sanitize = !math || options.katex_after_sanitize;
    Finisher {
        tree: &mut tree,
        sanitize,
        code_classes: options.sanitize.code_classes,
        harden: (!math).then_some(&options.harden),
        table_depth: 0,
    }
    .children(root);
    if !math {
        for plugin in &options.plugins {
            plugin.hast(&mut tree, root, &context);
        }
    }
    Ok(Built {
        kind: if math { Kind::Math } else { Kind::Complete },
        points,
        tree,
        root,
    })
}

/// The HAST as opcodes over `source` with NUL already replaced (see
/// [`mdast::without_nul`]); offsets index that string in UTF-16 units.
pub fn render_ops(source: &str, options: &Options) -> Result<Encoded, Error> {
    let Built {
        kind,
        points,
        tree,
        root,
    } = build(source, options)?;
    let mut encoder = Encoder {
        tree: &tree,
        points: &points,
        source,
        // 7 to 12 opcodes per node, about 10 typically.
        ops: Vec::with_capacity(tree.nodes.len() * 11),
        // Generated strings run to about half the source.
        strings: String::with_capacity(source.len() / 2),
        units: 0,
        interned: HashMap::new(),
    };
    encoder.node(root);
    Ok(Encoded {
        kind,
        ops: encoder.ops,
        strings: encoder.strings,
    })
}
