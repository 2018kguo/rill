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
    self, AlignKind, At, Error, NIL, NodeId, Point, Points, Reference, ReferenceKind, Syntax,
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
pub enum Value<'s> {
    Str(Cow<'s, str>),
    Bool(bool),
    Num(i64),
    List(Vec<Cow<'s, str>>),
    /// A one-class list, `["name"]`, without allocating.
    Class(&'static str),
}

/// An element's properties, in hast's order.
pub type Props<'s> = Vec<(&'static str, Value<'s>)>;

/// An element's hast `data`, boxed because only some code elements have any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Data {
    /// `data.meta` from mdast-util-to-hast's code handler.
    pub meta: Box<str>,
}

#[derive(Clone, Debug)]
pub enum NodeKind<'s> {
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
pub struct Node<'s> {
    pub kind: NodeKind<'s>,
    pub first: NodeId,
    pub last: NodeId,
    pub next: NodeId,
    pub start: Edge,
    pub end: Edge,
}

impl Node<'_> {
    /// hast-util-sanitize keeps only complete positions.
    fn drop_partial_position(&mut self) {
        if self.start == Edge::None || self.end == Edge::None {
            self.start = Edge::None;
            self.end = Edge::None;
        }
    }
}

pub struct Tree<'s> {
    pub nodes: Vec<Node<'s>>,
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
    first: NodeId,
    last: NodeId,
}

const EMPTY: Chain = Chain {
    first: NIL,
    last: NIL,
};

impl Chain {
    /// A chain of just `ix`, a fresh node whose `next` is still `NIL`. A node that may
    /// still be linked needs [`Tree::one_chain`].
    fn one(ix: NodeId) -> Chain {
        Chain {
            first: ix,
            last: ix,
        }
    }
}

impl Edge {
    /// The point this edge stands for, if any.
    fn point(self, points: &Points) -> Option<Point> {
        match self {
            Edge::None => None,
            Edge::Byte(byte) => Some(points.point(byte)),
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

impl<'s> Tree<'s> {
    fn push(&mut self, kind: NodeKind<'s>, at: At) -> NodeId {
        let (start, end) = edges(at);
        let ix = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            kind,
            first: NIL,
            last: NIL,
            next: NIL,
            start,
            end,
        });
        ix
    }

    fn text(&mut self, value: impl Into<Cow<'s, str>>) -> NodeId {
        self.push(NodeKind::Text(value.into()), At::None)
    }

    fn element(&mut self, tag: &'static str, props: Props<'s>, at: At) -> NodeId {
        self.push(
            NodeKind::Element {
                tag,
                props,
                data: None,
            },
            at,
        )
    }

    /// A new node of `kind` with `children`.
    fn parent(&mut self, kind: NodeKind<'s>, at: At, children: Chain) -> NodeId {
        let parent = self.push(kind, at);
        self.set_children(parent, children);
        parent
    }

    /// A new element with `children`.
    fn element_with(
        &mut self,
        tag: &'static str,
        props: Props<'s>,
        at: At,
        children: Chain,
    ) -> NodeId {
        let element = self.element(tag, props, at);
        self.set_children(element, children);
        element
    }

    /// A new element around one fresh node.
    fn element_around(
        &mut self,
        tag: &'static str,
        props: Props<'s>,
        at: At,
        child: NodeId,
    ) -> NodeId {
        self.element_with(tag, props, at, Chain::one(child))
    }

    fn set_children(&mut self, parent: NodeId, chain: Chain) {
        self.nodes[parent as usize].first = chain.first;
        self.nodes[parent as usize].last = chain.last;
    }

    fn children_of(&self, parent: NodeId) -> Chain {
        Chain {
            first: self.nodes[parent as usize].first,
            last: self.nodes[parent as usize].last,
        }
    }

    fn one_chain(&mut self, ix: NodeId) -> Chain {
        self.nodes[ix as usize].next = NIL;
        Chain {
            first: ix,
            last: ix,
        }
    }

    fn append(&mut self, chain: &mut Chain, ix: NodeId) {
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

    fn prepend(&mut self, chain: &mut Chain, ix: NodeId) {
        self.nodes[ix as usize].next = chain.first;
        if chain.first == NIL {
            chain.last = ix;
        }
        chain.first = ix;
    }

    /// `first` and the siblings after it.
    fn siblings(&self, first: NodeId) -> impl Iterator<Item = NodeId> + '_ {
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
    pub fn tag_name(&self, ix: NodeId) -> Option<&'static str> {
        match &self.nodes[ix as usize].kind {
            NodeKind::Element { tag, .. } => Some(tag),
            _ => None,
        }
    }

    /// An element's properties.
    pub fn properties_mut(&mut self, ix: NodeId) -> Option<&mut Props<'s>> {
        match &mut self.nodes[ix as usize].kind {
            NodeKind::Element { props, .. } => Some(props),
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
impl<'s> Tree<'s> {
    /// A new, detached node.
    pub fn add(&mut self, kind: NodeKind<'s>) -> NodeId {
        self.push(kind, At::None)
    }

    /// `parent`'s children, in order.
    pub fn children(&self, parent: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.siblings(self.nodes[parent as usize].first)
    }

    /// `parent`'s children, in order, collected so the tree can be edited meanwhile.
    #[must_use]
    pub fn child_list(&self, parent: NodeId) -> Vec<NodeId> {
        self.children(parent).collect()
    }

    /// Makes `children` (detached nodes or `parent`'s own) `parent`'s children.
    pub fn replace_children(&mut self, parent: NodeId, children: &[NodeId]) {
        let mut chain = EMPTY;
        for &child in children {
            self.append(&mut chain, child);
        }
        self.set_children(parent, chain);
    }
}

/// Sets property `key`, replacing any value it had.
pub fn set<'s>(props: &mut Props<'s>, key: &'static str, value: Value<'s>) {
    match props.iter_mut().find(|(name, _)| *name == key) {
        Some((_, slot)) => *slot = value,
        None => props.push((key, value)),
    }
}

/// Property `key`.
#[must_use]
pub fn get<'p, 's>(props: &'p Props<'s>, key: &str) -> Option<&'p Value<'s>> {
    props
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value)
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Slices a `Cow`, keeping a borrow borrowed.
pub(crate) fn slice_cow<'s>(value: &Cow<'s, str>, range: std::ops::Range<usize>) -> Cow<'s, str> {
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

pub(crate) fn js_whitespace(c: char) -> bool {
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
    mdast: &'t mdast::Tree<'s>,
    hast: Tree<'s>,
    /// Definitions by uppercased identifier; the first of a duplicate wins.
    definitions: HashMap<String, Definition<'s>>,
    footnotes: HashMap<String, NodeId>,
    /// Cited footnotes in first-citation order, which numbers them.
    footnote_calls: Vec<FootnoteCalls>,
    /// Raw HTML or links inside links: parse5 effects the native raw stage omits.
    raw_boundary: bool,
    /// Raw HTML that `pair_simple_raw` can turn into elements instead.
    simple_raw: bool,
    link_depth: usize,
    /// rehype-katex runs (`plugins.math`), so ```math blocks are math too.
    katex: bool,
    /// remarkCodeMeta runs (see [`Options::code_meta`]).
    code_meta: bool,
    /// The tree holds math for rehype-katex.
    math: bool,
    /// A comment still streaming is simple too (see [`Options::close_open_raw`]).
    open_comments: bool,
    /// Whether each list converted so far is loose.
    loose_lists: HashMap<NodeId, bool>,
}

impl<'s> ToHast<'_, 's> {
    fn collect(&mut self, ix: NodeId) {
        match &self.mdast.node(ix).kind {
            mdast::NodeKind::Definition(definition) => {
                self.definitions
                    .entry(definition.identifier.to_uppercase())
                    .or_insert_with(|| Definition {
                        url: definition.url.clone(),
                        title: definition.title.clone(),
                    });
            }
            mdast::NodeKind::FootnoteDefinition(association) => {
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
    fn returns_array(&self, ix: NodeId) -> bool {
        match &self.mdast.node(ix).kind {
            mdast::NodeKind::Break => true,
            mdast::NodeKind::LinkReference(reference)
            | mdast::NodeKind::ImageReference(reference) => !self.defined(&reference.identifier),
            _ => false,
        }
    }

    fn all(&mut self, parent: NodeId) -> Chain {
        let mut values = EMPTY;
        let mut previous_break = false;
        for child in self.mdast.children(parent) {
            let result = self.one(child, parent);
            if result.first != NIL && previous_break && !self.returns_array(child) {
                let head = result.first;
                let target = match &self.hast.nodes[head as usize].kind {
                    NodeKind::Text(_) => Some(head),
                    NodeKind::Element { .. } => {
                        let first = self.hast.nodes[head as usize].first;
                        (first != NIL
                            && matches!(self.hast.nodes[first as usize].kind, NodeKind::Text(_)))
                        .then_some(first)
                    }
                    _ => None,
                };
                if let Some(target) = target
                    && let NodeKind::Text(value) = &mut self.hast.nodes[target as usize].kind
                {
                    *value = trim_markdown_space_start(value);
                }
            }
            previous_break = matches!(self.mdast.node(child).kind, mdast::NodeKind::Break);
            self.hast.extend(&mut values, result);
        }
        values
    }

    fn element(&mut self, tag: &'static str, ix: NodeId, props: Props<'s>) -> Chain {
        let children = self.all(ix);
        Chain::one(
            self.hast
                .element_with(tag, props, self.mdast.node(ix).at, children),
        )
    }

    fn wrapped(
        &mut self,
        tag: &'static str,
        ix: NodeId,
        props: Props<'s>,
        spacing: Spacing,
    ) -> Chain {
        let children = self.all(ix);
        let children = self.hast.wrap(children, spacing);
        Chain::one(
            self.hast
                .element_with(tag, props, self.mdast.node(ix).at, children),
        )
    }

    fn one(&mut self, ix: NodeId, parent: NodeId) -> Chain {
        // A copy of the shared borrow, so node fields outlive the `&mut self` calls.
        let mdast = self.mdast;
        let at = mdast.node(ix).at;
        match &mdast.node(ix).kind {
            mdast::NodeKind::Root => {
                let children = self.all(ix);
                let children = self.hast.wrap(children, Spacing::Tight);
                Chain::one(self.hast.parent(NodeKind::Root, at, children))
            }
            mdast::NodeKind::Blockquote => {
                self.wrapped("blockquote", ix, Vec::new(), Spacing::Loose)
            }
            mdast::NodeKind::Break => {
                let br = self.hast.element("br", Vec::new(), at);
                let mut chain = Chain::one(br);
                let newline = self.hast.text("\n");
                self.hast.append(&mut chain, newline);
                chain
            }
            mdast::NodeKind::Code(code) => {
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
                    // rehype-katex renders ```math blocks too (`language-math`).
                    self.math |= self.katex && first == "math";
                    props.push((
                        "className",
                        Value::List(vec![Cow::Owned(format!("language-{first}"))]),
                    ));
                }
                let meta = code.meta.clone().filter(|meta| !meta.is_empty());
                if let Some(meta) = meta.as_ref().filter(|_| self.code_meta) {
                    // Streamdown's remarkCodeMeta: the meta string as a `metastring` property.
                    props.push(("metastring", Value::Str(Cow::Owned(meta.to_string()))));
                }
                let data = meta.map(|meta| Box::new(Data { meta }));
                let text = self.hast.text(value);
                let code = self.hast.parent(
                    NodeKind::Element {
                        tag: "code",
                        props,
                        data,
                    },
                    at,
                    Chain::one(text),
                );
                Chain::one(self.hast.element_around("pre", Vec::new(), at, code))
            }
            mdast::NodeKind::Delete => self.element("del", ix, Vec::new()),
            mdast::NodeKind::Emphasis => self.element("em", ix, Vec::new()),
            mdast::NodeKind::Strong => self.element("strong", ix, Vec::new()),
            mdast::NodeKind::Paragraph => self.element("p", ix, Vec::new()),
            mdast::NodeKind::Heading(depth) => {
                let tag =
                    ["h1", "h2", "h3", "h4", "h5", "h6"][usize::from((*depth).clamp(1, 6) - 1)];
                self.element(tag, ix, Vec::new())
            }
            mdast::NodeKind::ThematicBreak => Chain::one(self.hast.element("hr", Vec::new(), at)),
            mdast::NodeKind::FootnoteReference(association) => {
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
                        Value::Str(Cow::Owned(format!("#user-content-fn-{safe_id}"))),
                    ),
                    (
                        "id",
                        Value::Str(Cow::Owned(format!("user-content-fnref-{safe_id}{suffix}"))),
                    ),
                    ("dataFootnoteRef", Value::Bool(true)),
                    ("ariaDescribedBy", Value::Class("footnote-label")),
                ];
                if self.link_depth > 0 {
                    self.raw_boundary = true;
                }
                let label = self.hast.text(Cow::Owned(counter.to_string()));
                let link = self.hast.element_around("a", props, at, label);
                Chain::one(self.hast.element_around("sup", Vec::new(), at, link))
            }
            mdast::NodeKind::Html(value) => {
                // Flow HTML (a block of its own) can restructure what follows it.
                let flow = matches!(
                    self.mdast.node(parent).kind,
                    mdast::NodeKind::Root
                        | mdast::NodeKind::Blockquote
                        | mdast::NodeKind::ListItem { .. }
                        | mdast::NodeKind::FootnoteDefinition(_)
                );
                // A comment ends where micromark's does and restructures nothing.
                // One still streaming, only as the text's last node (in a container,
                // what follows the raw node can join the comment).
                let last = matches!(self.mdast.node(parent).kind, mdast::NodeKind::Root)
                    && self.mdast.nodes[ix as usize].next == mdast::NIL;
                let simple = SimpleRaw::parse(value)
                    .filter(|_| (self.open_comments && last) || !SimpleRaw::open_comment(value));
                let flow_ok = matches!(
                    simple,
                    Some(SimpleRaw::Comment | SimpleRaw::WrapOpen | SimpleRaw::WrapClose)
                );
                // `<listing>` closes an open `p`: only where none is open.
                let listing = simple.is_some_and(|raw| raw.listing(value));
                if simple.is_some() && (if flow { flow_ok } else { !listing }) {
                    self.simple_raw = true;
                } else {
                    self.raw_boundary = true;
                }
                Chain::one(self.hast.push(NodeKind::Raw(value.clone()), at))
            }
            mdast::NodeKind::Image(image) => {
                self.image(image.url.clone(), image.title.clone(), &image.alt, at)
            }
            mdast::NodeKind::ImageReference(reference) => match self.definition(reference) {
                None => {
                    let suffix = revert_suffix(reference);
                    let alt = &reference.alt;
                    Chain::one(self.hast.text(Cow::Owned(format!("![{alt}{suffix}"))))
                }
                Some(Definition { url, title }) => self.image(url, title, &reference.alt, at),
            },
            mdast::NodeKind::InlineCode(value) => {
                let value = if value.contains(['\n', '\r']) {
                    Cow::Owned(value.replace("\r\n", " ").replace(['\r', '\n'], " "))
                } else {
                    value.clone()
                };
                let text = self.hast.push(NodeKind::Text(value), at);
                Chain::one(self.hast.element_around("code", Vec::new(), at, text))
            }
            mdast::NodeKind::Link(link) => self.link(ix, link.url.clone(), link.title.clone()),
            mdast::NodeKind::LinkReference(reference) => match self.definition(reference) {
                None => {
                    let suffix = revert_suffix(reference);
                    let mut contents = self.all(ix);
                    let head = (contents.first != NIL)
                        .then(|| &mut self.hast.nodes[contents.first as usize].kind);
                    match head {
                        Some(NodeKind::Text(value)) => *value = Cow::Owned(format!("[{value}")),
                        _ => {
                            let open = self.hast.text("[");
                            self.hast.prepend(&mut contents, open);
                        }
                    }
                    match &mut self.hast.nodes[contents.last as usize].kind {
                        NodeKind::Text(value) => push_cow(value, &suffix),
                        _ => {
                            let close = self.hast.text(Cow::Owned(suffix));
                            self.hast.append(&mut contents, close);
                        }
                    }
                    contents
                }
                Some(Definition { url, title }) => self.link(ix, url, title),
            },
            mdast::NodeKind::List { ordered, start, .. } => {
                let tag = if *ordered { "ol" } else { "ul" };
                let mut props = Vec::new();
                if let Some(start) = start.filter(|start| *start != 1) {
                    props.push(("start", Value::Num(i64::from(start))));
                }
                let results = self.all(ix);
                let has_task = self.hast.siblings(results.first).any(|item| {
                    matches!(
                        &self.hast.nodes[item as usize].kind,
                        NodeKind::Element { tag: "li", props, .. }
                            if matches!(get(props, "className"), Some(Value::Class("task-list-item")))
                    )
                });
                if has_task {
                    props.push(("className", Value::Class("contains-task-list")));
                }
                let children = self.hast.wrap(results, Spacing::Loose);
                Chain::one(self.hast.element_with(tag, props, at, children))
            }
            mdast::NodeKind::ListItem { spread, checked } => {
                self.list_item(ix, parent, *spread, *checked)
            }
            mdast::NodeKind::Table(align) => self.table(ix, align, at),
            mdast::NodeKind::Text(value) => {
                Chain::one(self.hast.push(NodeKind::Text(trim_lines(value)), at))
            }
            mdast::NodeKind::Math(math) => {
                self.math = true;
                let text = self.hast.text(math.value.clone());
                let class = Value::List(vec![
                    Cow::Borrowed("language-math"),
                    Cow::Borrowed("math-display"),
                ]);
                let code =
                    self.hast
                        .element_around("code", vec![("className", class)], At::None, text);
                Chain::one(self.hast.element_around("pre", Vec::new(), at, code))
            }
            mdast::NodeKind::InlineMath(value) => {
                self.math = true;
                let text = self.hast.text(value.clone());
                let class = Value::List(vec![
                    Cow::Borrowed("language-math"),
                    Cow::Borrowed("math-inline"),
                ]);
                Chain::one(
                    self.hast
                        .element_around("code", vec![("className", class)], at, text),
                )
            }
            mdast::NodeKind::Definition { .. } | mdast::NodeKind::FootnoteDefinition { .. } => {
                EMPTY
            }
            // Rows and cells are handled by their table; the unknown handler's `div`.
            mdast::NodeKind::TableRow | mdast::NodeKind::TableCell => {
                self.element("div", ix, Vec::new())
            }
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
            ("src", Value::Str(normalize_uri(url))),
            ("alt", Value::Str(Cow::Owned(alt.to_owned()))),
        ];
        if let Some(title) = title {
            props.push(("title", Value::Str(title)));
        }
        Chain::one(self.hast.element("img", props, at))
    }

    fn link(&mut self, ix: NodeId, url: Cow<'s, str>, title: Option<Cow<'s, str>>) -> Chain {
        // Room for the title and rehype-harden's `target` and `rel`.
        let mut props = Vec::with_capacity(4);
        props.push(("href", Value::Str(normalize_uri(url))));
        if let Some(title) = title {
            props.push(("title", Value::Str(title)));
        }
        if self.link_depth > 0 {
            self.raw_boundary = true;
        }
        self.link_depth += 1;
        let chain = self.element("a", ix, props);
        self.link_depth -= 1;
        chain
    }

    fn list_item(
        &mut self,
        ix: NodeId,
        parent: NodeId,
        spread: bool,
        checked: Option<bool>,
    ) -> Chain {
        let mut results = self.all(ix);
        let loose = match &self.mdast.node(parent).kind {
            mdast::NodeKind::List {
                spread: list_spread,
                ..
            } => {
                // mdast-util-to-hast's `listLoose` scans the list for every item: once
                // per list here.
                let mdast = self.mdast;
                *self.loose_lists.entry(parent).or_insert_with(|| {
                    *list_spread
                        || mdast.children(parent).any(|item| {
                            matches!(
                                mdast.node(item).kind,
                                mdast::NodeKind::ListItem { spread: true, .. }
                            )
                        })
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
                    ("type", Value::Str(Cow::Borrowed("checkbox"))),
                    ("checked", Value::Bool(checked)),
                    ("disabled", Value::Bool(true)),
                ],
                At::None,
            );
            self.hast.prepend(&mut children, input);
            self.hast.set_children(paragraph, children);
            props.push(("className", Value::Class("task-list-item")));
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

    fn table(&mut self, ix: NodeId, align: &[AlignKind], at: At) -> Chain {
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

    fn table_row(&mut self, row: NodeId, index: usize, align: &[AlignKind]) -> NodeId {
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
                vec![("align", Value::Str(Cow::Borrowed(value)))]
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

    fn footer(&mut self) -> Option<NodeId> {
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
                            Value::Str(Cow::Owned(format!(
                                "#user-content-fnref-{safe_id}{suffix}"
                            ))),
                        ),
                        ("dataFootnoteBackref", Value::Str(Cow::Borrowed(""))),
                        (
                            "ariaLabel",
                            Value::Str(Cow::Owned(format!(
                                "Back to reference {}{suffix}",
                                reference_index + 1
                            ))),
                        ),
                        ("className", Value::Class("data-footnote-backref")),
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
                    Some(NodeKind::Text(value)) => push_cow(value, " "),
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
                    Value::Str(Cow::Owned(format!("user-content-fn-{safe_id}"))),
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
                ("className", Value::Class("sr-only")),
                ("id", Value::Str(Cow::Borrowed("footnote-label"))),
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
                ("dataFootnotes", Value::Bool(true)),
                ("className", Value::Class("footnotes")),
            ],
            At::None,
            children,
        ))
    }
}

/// HAST plus what the native raw stage must leave to JavaScript.
pub struct Converted<'s> {
    pub tree: Tree<'s>,
    pub root: NodeId,
    pub raw_boundary: bool,
    pub math: bool,
}

/// mdast-util-to-hast. `katex`: rehype-katex runs later, so ```math blocks count as math.
/// With Streamdown's remarkCodeMeta.
pub fn to_hast<'s>(mdast: &mdast::Tree<'s>, katex: bool) -> Converted<'s> {
    convert(
        mdast,
        &Options {
            math: katex,
            ..Options::default()
        },
    )
}

fn convert<'s>(mdast: &mdast::Tree<'s>, options: &Options) -> Converted<'s> {
    let close_open_raw = options.close_open_raw;
    let mut state = ToHast {
        mdast,
        hast: Tree {
            // Prose and chat Markdown make 1.3 to 1.8 HAST nodes per MDAST node.
            nodes: Vec::with_capacity(mdast.nodes.len() * 7 / 4 + 16),
            quirks: false,
        },
        definitions: HashMap::new(),
        footnotes: HashMap::new(),
        footnote_calls: Vec::new(),
        raw_boundary: false,
        simple_raw: false,
        link_depth: 0,
        katex: options.math,
        code_meta: options.code_meta,
        math: false,
        open_comments: close_open_raw,
        loose_lists: HashMap::new(),
    };
    state.collect(mdast::Tree::ROOT);
    let root = state.one(mdast::Tree::ROOT, mdast::Tree::ROOT).first;
    if let Some(section) = state.footer() {
        let mut children = state.hast.children_of(root);
        let newline = state.hast.text("\n");
        state.hast.append(&mut children, newline);
        state.hast.append(&mut children, section);
        state.hast.set_children(root, children);
    }
    if state.simple_raw
        && !state.raw_boundary
        && !pair_simple_raw(&mut state.hast, root, close_open_raw)
    {
        state.raw_boundary = true;
    }
    Converted {
        root,
        raw_boundary: state.raw_boundary,
        math: state.math,
        tree: state.hast,
    }
}

// ---------------------------------------------------------------------------
// Simple raw HTML: what rehype-raw (parse5) makes of the raw HTML chat answers carry
// most, without parse5. Anything else keeps the raw boundary and runs in JavaScript.
// ---------------------------------------------------------------------------

/// Phrasing elements whose properly nested pair among siblings parse5 makes exactly an
/// element around what lies between. The plain ones are "any other start tag" and "any
/// other end tag" in body. The formatting ones (b, i, u, s, em, strong, small, strike,
/// big, tt) run the adoption agency algorithm on their end tag, which, with only
/// phrasing content opened since the start tag, finds no furthest block and just
/// closes the element; nothing they leave in the active formatting list is ever
/// reconstructed, because each is closed before its parent is.
const PAIRED_TAGS: [&str; 24] = [
    "abbr", "cite", "del", "dfn", "ins", "kbd", "mark", "q", "samp", "span", "sub", "sup", "time",
    "var", "b", "i", "u", "s", "em", "strong", "small", "strike", "big", "tt",
];

/// One inline raw HTML node parse5 turns into something plain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SimpleRaw {
    /// A start tag of `br` or `img` (void: parse5 opens and closes it at once).
    Void(&'static str),
    /// A start tag of a [`PAIRED_TAGS`] element.
    Open(&'static str),
    /// Its end tag.
    Close(&'static str),
    /// A comment parse5 ends where micromark does; sanitize removes it.
    Comment,
    /// A start tag of an element sanitize replaces with its children (see
    /// [`wrapper_tag`]): parse5 opens it, and its end tag among the same siblings
    /// closes it, so its content stays where it is.
    WrapOpen,
    /// Its end tag.
    WrapClose,
}

impl SimpleRaw {
    /// A comment with no end, which parse5 ends where the raw node ends.
    fn open_comment(value: &str) -> bool {
        value.strip_prefix("<!--").is_some_and(|body| {
            !body.starts_with('>')
                && !body.starts_with("->")
                && !body.contains("--!>")
                && !body.contains("-->")
        })
    }

    fn parse(value: &str) -> Option<SimpleRaw> {
        if let Some(body) = value
            .strip_prefix("<!--")
            .and_then(|rest| rest.strip_suffix("-->"))
        {
            // parse5 ends a comment early at a leading `>` or `->` and at `--!>`.
            let plain = !body.starts_with('>')
                && !body.starts_with("->")
                && !body.contains("--!>")
                && !body.contains("-->");
            return plain.then_some(SimpleRaw::Comment);
        }
        // A comment still streaming (flow HTML runs to the end of its container): parse5
        // ends it with the raw node, but leaves the root without a position, so only
        // `Options::close_open_raw` takes it.
        if Self::open_comment(value) {
            return Some(SimpleRaw::Comment);
        }
        let inner = value.strip_prefix('<')?.strip_suffix('>')?;
        if let Some(name) = inner.strip_prefix('/') {
            if wrapper_tag(name) {
                return Some(SimpleRaw::WrapClose);
            }
            return paired_tag(name).map(SimpleRaw::Close);
        }
        let tag = StartTag::parse(inner)?;
        if wrapper_tag(tag.name) {
            // parse5 ignores `/>` here too: the element stays open.
            return (!tag.self_closing).then_some(SimpleRaw::WrapOpen);
        }
        raw_props(value)?;
        if let Some(void) = ["br", "img"]
            .into_iter()
            .find(|known| known.eq_ignore_ascii_case(tag.name))
        {
            return Some(SimpleRaw::Void(void));
        }
        // parse5 ignores `/>` on other elements: the element stays open.
        if tag.self_closing {
            return None;
        }
        paired_tag(tag.name).map(SimpleRaw::Open)
    }
}

impl SimpleRaw {
    /// Whether this is `<listing>` or `</listing>` (`value` is its raw node's).
    fn listing(self, value: &str) -> bool {
        matches!(self, SimpleRaw::WrapOpen | SimpleRaw::WrapClose)
            && wrapper_name(value).eq_ignore_ascii_case("listing")
    }
}

/// The tag name of a wrapper's start or end tag.
fn wrapper_name(value: &str) -> &str {
    let inner = value.trim_start_matches('<').trim_start_matches('/');
    let end = inner
        .bytes()
        .position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-'))
        .unwrap_or(inner.len());
    &inner[..end]
}

/// Tag names parse5 7.3.0 knows (`TAG_NAMES` in common/html.js; any other is "any other
/// start tag" and "any other end tag" in body), sorted.
const PARSE5_TAGS: &[&str] = &[
    "a",
    "address",
    "annotation-xml",
    "applet",
    "area",
    "article",
    "aside",
    "b",
    "base",
    "basefont",
    "bgsound",
    "big",
    "blockquote",
    "body",
    "br",
    "button",
    "caption",
    "center",
    "code",
    "col",
    "colgroup",
    "dd",
    "desc",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "font",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hgroup",
    "hr",
    "html",
    "i",
    "iframe",
    "image",
    "img",
    "input",
    "keygen",
    "label",
    "li",
    "link",
    "listing",
    "main",
    "malignmark",
    "marquee",
    "math",
    "menu",
    "meta",
    "mglyph",
    "mi",
    "mn",
    "mo",
    "ms",
    "mtext",
    "nav",
    "nobr",
    "noembed",
    "noframes",
    "noscript",
    "object",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "plaintext",
    "pre",
    "rb",
    "rp",
    "rt",
    "rtc",
    "ruby",
    "s",
    "script",
    "search",
    "section",
    "select",
    "small",
    "source",
    "span",
    "strike",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "svg",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "tt",
    "u",
    "ul",
    "var",
    "wbr",
    "xmp",
];

/// An element whose start and end tags parse5 handles as an ordinary element's and that
/// sanitize replaces with its children (not in its `tagNames`, not in `strip`): a name
/// parse5 does not know, or `listing` (opened like `pre`: it closes an open `p`, and the
/// newline it would skip arrives as text, which parse5 does not skip). The Rust Book's
/// `<Listing>` around its code listings is one.
fn wrapper_tag(name: &str) -> bool {
    if !name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    lower == "listing"
        || (PARSE5_TAGS.binary_search(&lower.as_str()).is_err()
            && TAG_NAMES.binary_search(&lower.as_str()).is_err())
}

/// parse5's whitespace in tags.
fn tag_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0c')
}

/// A start tag micromark accepted as HTML (`<` and `>` stripped), read as parse5's
/// tokenizer does when nothing in it is unusual: no carriage returns, attribute names of
/// letters, digits, `-`, `_`, `.` and `:`, each name once, and character references only
/// ones that end with `;` (see [`attribute_value`]). Anything else is `None` and stays
/// raw.
struct StartTag<'v> {
    name: &'v str,
    attributes: Vec<(&'v str, &'v str)>,
    self_closing: bool,
}

impl<'v> StartTag<'v> {
    fn parse(inner: &'v str) -> Option<StartTag<'v>> {
        if inner.contains(['\r', '\0']) {
            return None;
        }
        let bytes = inner.as_bytes();
        let name_end = bytes
            .iter()
            .position(|&byte| tag_space(byte) || byte == b'/')
            .unwrap_or(bytes.len());
        let name = &inner[..name_end];
        // Letters and digits, and `-` after the first (custom element names).
        if !name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return None;
        }
        let mut tag = StartTag {
            name,
            attributes: Vec::new(),
            self_closing: false,
        };
        let mut i = name_end;
        loop {
            while i < bytes.len() && tag_space(bytes[i]) {
                i += 1;
            }
            if i == bytes.len() {
                return Some(tag);
            }
            if bytes[i] == b'/' {
                // micromark allows `/` only right before `>`.
                tag.self_closing = true;
                return (i + 1 == bytes.len()).then_some(tag);
            }
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || matches!(bytes[i], b'-' | b'_' | b'.' | b':'))
            {
                i += 1;
            }
            let attribute = &inner[start..i];
            if attribute.is_empty()
                || attribute.bytes().any(|byte| byte.is_ascii_uppercase())
                || tag.attributes.iter().any(|(known, _)| *known == attribute)
            {
                return None;
            }
            let mut j = i;
            while j < bytes.len() && tag_space(bytes[j]) {
                j += 1;
            }
            if bytes.get(j) != Some(&b'=') {
                tag.attributes.push((attribute, ""));
                continue;
            }
            j += 1;
            while j < bytes.len() && tag_space(bytes[j]) {
                j += 1;
            }
            let value = match bytes.get(j) {
                Some(&quote @ (b'"' | b'\'')) => {
                    let close = j + 1 + bytes[j + 1..].iter().position(|&byte| byte == quote)?;
                    let value = &inner[j + 1..close];
                    i = close + 1;
                    // parse5 wants whitespace, `/` or `>` after a quoted value.
                    if i < bytes.len() && !tag_space(bytes[i]) && bytes[i] != b'/' {
                        return None;
                    }
                    value
                }
                Some(_) => {
                    let end = bytes[j..]
                        .iter()
                        .position(|&byte| tag_space(byte))
                        .map_or(bytes.len(), |offset| j + offset);
                    let value = &inner[j..end];
                    if value.contains(['"', '\'', '<', '=', '`', '/']) {
                        return None;
                    }
                    i = end;
                    value
                }
                None => return None,
            };
            tag.attributes.push((attribute, value));
        }
    }
}

/// An attribute value as parse5 decodes its character references, for the ones that end
/// with `;` and name a character (numeric ones outside controls and surrogates); `None`
/// for any other `&`, whose decoding in attributes has rules of its own.
fn attribute_value(value: &str) -> Option<Cow<'_, str>> {
    if !value.contains('&') {
        return Some(Cow::Borrowed(value));
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after.find(';')?;
        let name = &after[..end];
        if let Some(number) = name.strip_prefix('#') {
            let (digits, radix) = match number.strip_prefix(['x', 'X']) {
                Some(hex) => (hex, 16),
                None => (number, 10),
            };
            if digits.is_empty() || digits.len() > 7 || !digits.chars().all(|c| c.is_digit(radix)) {
                return None;
            }
            let code = u32::from_str_radix(digits, radix).ok()?;
            if !matches!(code, 0x09 | 0x0a | 0x20..=0x7e | 0xa0..=0xd7ff | 0xe000..=0x10ffff) {
                return None;
            }
            out.push(char::from_u32(code)?);
        } else {
            if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
                return None;
            }
            out.push_str(rill_cmark::named_entity(name)?);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(Cow::Owned(out))
}

/// The HAST property hastscript makes of attribute `name` on an element rehype-raw
/// creates, for the attributes rehype-sanitize can keep; `Some(None)` for one it drops
/// whatever the value, `None` for one this does not model (the tag then stays raw).
fn raw_property(name: &str) -> Option<Option<&'static str>> {
    Some(Some(match name {
        "class" => "className",
        "alt" => "alt",
        "src" => "src",
        "href" => "href",
        "id" => "id",
        "title" => "title",
        "lang" => "lang",
        "dir" => "dir",
        "align" => "align",
        "name" => "name",
        "cite" => "cite",
        "longdesc" => "longDesc",
        "abbr" => "abbr",
        "axis" => "axis",
        "label" => "label",
        "summary" => "summary",
        "scope" => "scope",
        "shape" => "shape",
        "rules" => "rules",
        "rev" => "rev",
        "prompt" => "prompt",
        "media" => "media",
        "method" => "method",
        "action" => "action",
        "frame" => "frame",
        "clear" => "clear",
        "color" => "color",
        "char" => "char",
        "charoff" => "charOff",
        "charset" => "charSet",
        "cellpadding" => "cellPadding",
        "cellspacing" => "cellSpacing",
        "datetime" => "dateTime",
        "enctype" => "encType",
        "hreflang" => "hrefLang",
        "usemap" => "useMap",
        "valign" => "vAlign",
        "type" => "type",
        "metastring" => "metastring",
        "srcset" => "srcSet",
        "aria-label" => "ariaLabel",
        "data-footnote-ref" => "dataFootnoteRef",
        "data-footnote-backref" => "dataFootnoteBackref",
        "data-footnotes" => "dataFootnotes",
        // Numbers (when JavaScript's `Number` reads one).
        "width" => "width",
        "height" => "height",
        "border" => "border",
        "colspan" => "colSpan",
        "rowspan" => "rowSpan",
        "cols" => "cols",
        "rows" => "rows",
        "size" => "size",
        "span" => "span",
        "start" => "start",
        "tabindex" => "tabIndex",
        "maxlength" => "maxLength",
        "hspace" => "hSpace",
        // Booleans, comma-separated lists and the like.
        "checked" | "compact" | "ismap" | "multiple" | "nohref" | "noshade" | "nowrap" | "open"
        | "readonly" | "selected" | "itemscope" | "disabled" | "value" | "accept"
        | "accept-charset" | "accesskey" | "coords" | "for" | "headers" | "itemprop"
        | "itemtype" | "aria-describedby" | "aria-labelledby" => return None,
        _ if name.starts_with("aria-") || name.starts_with("data-") => return None,
        _ => return Some(None),
    }))
}

/// Properties hastscript reads as numbers.
fn numeric_property(key: &str) -> bool {
    matches!(
        key,
        "width"
            | "height"
            | "border"
            | "colSpan"
            | "rowSpan"
            | "cols"
            | "rows"
            | "size"
            | "span"
            | "start"
            | "tabIndex"
            | "maxLength"
            | "hSpace"
    )
}

/// The properties of a raw start tag's element, as strings for [`round_trip`] (which
/// reads them as hastscript does), or `None` for one this does not model.
fn raw_props<'s>(value: &str) -> Option<Props<'s>> {
    let inner = value.strip_prefix('<')?.strip_suffix('>')?;
    let tag = StartTag::parse(inner)?;
    let mut props = Vec::new();
    for (name, value) in tag.attributes {
        let Some(key) = raw_property(name)? else {
            continue;
        };
        let value = attribute_value(value)?;
        // `Number(value)`: digits are a number, a value that can be no number is a
        // string; anything between (signs, dots, exponents, hex) stays raw.
        if numeric_property(key)
            && !value.is_empty()
            && !(value.len() <= 15 && value.bytes().all(|byte| byte.is_ascii_digit()))
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'+' | b'-' | b'.' | b' ' | b'\t' | b'\n' | b'\x0c')
            })
        {
            return None;
        }
        props.push((key, Value::Str(Cow::Owned(value.into_owned()))));
    }
    Some(props)
}

fn paired_tag(name: &str) -> Option<&'static str> {
    PAIRED_TAGS
        .into_iter()
        .find(|known| known.eq_ignore_ascii_case(name))
}

/// The properties of the start tag at raw node `ix`, which [`SimpleRaw::parse`] read.
fn raw_node_props<'s>(tree: &Tree<'s>, ix: NodeId) -> Props<'s> {
    match &tree.nodes[ix as usize].kind {
        NodeKind::Raw(value) => raw_props(value).expect("SimpleRaw::parse read the tag"),
        _ => unreachable!("a raw node"),
    }
}

/// The simple raw node at `ix`, if it is one.
fn simple_raw_at(tree: &Tree<'_>, ix: NodeId) -> Option<SimpleRaw> {
    match &tree.nodes[ix as usize].kind {
        NodeKind::Raw(value) => SimpleRaw::parse(value),
        _ => None,
    }
}

/// Turns the simple raw HTML under `parent` into elements, as parse5 would. Returns
/// `false`, leaving the tree as it was, when a start or end tag has no partner among
/// its siblings (parse5 would then close or ignore it by rules this does not model).
///
/// With `close_open`, start tags of [`closes_with_parent`] elements left open in a
/// paragraph, table cell, list item or heading end with it, as parse5 ends them (what a
/// streaming answer holds while `<sup>1` waits for its `</sup>`). Those elements get no
/// position, which only [`crate::markup`] (`Options::close_open_raw`) can ignore.
fn pair_simple_raw(tree: &mut Tree<'_>, parent: NodeId, close_open: bool) -> bool {
    if !raw_tags_pair(tree, parent, close_open) {
        return false;
    }
    rewrite_simple_raw(tree, parent);
    true
}

/// [`PAIRED_TAGS`] elements outside the HTML parser's formatting elements: parse5 closes
/// them with their parent and never reopens them later (the adoption agency's
/// reconstruction is for formatting elements only).
fn closes_with_parent(tag: &str) -> bool {
    matches!(
        tag,
        "abbr"
            | "cite"
            | "del"
            | "dfn"
            | "ins"
            | "kbd"
            | "mark"
            | "q"
            | "samp"
            | "span"
            | "sub"
            | "sup"
            | "time"
            | "var"
    )
}

/// Parents whose end tag pops every element still open inside them.
fn closes_open_children(tag: Option<&str>) -> bool {
    matches!(
        tag,
        Some("p" | "td" | "th" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
    )
}

/// An element [`raw_tags_pair`] has seen open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Opened {
    Tag(&'static str),
    /// A wrapper, by the raw node of its start tag.
    Wrap(NodeId),
}

fn raw_value<'t>(tree: &'t Tree<'_>, ix: NodeId) -> &'t str {
    match &tree.nodes[ix as usize].kind {
        NodeKind::Raw(value) => value,
        _ => unreachable!("a raw node"),
    }
}

fn raw_tags_pair(tree: &Tree<'_>, parent: NodeId, close_open: bool) -> bool {
    let mut open = Vec::new();
    for child in tree.children(parent) {
        match simple_raw_at(tree, child) {
            Some(SimpleRaw::Open(tag)) => open.push(Opened::Tag(tag)),
            Some(SimpleRaw::Close(tag)) => {
                if open.pop() != Some(Opened::Tag(tag)) {
                    return false;
                }
            }
            Some(SimpleRaw::WrapOpen) => open.push(Opened::Wrap(child)),
            Some(SimpleRaw::WrapClose) => match open.pop() {
                Some(Opened::Wrap(start))
                    if wrapper_name(raw_value(tree, start))
                        .eq_ignore_ascii_case(wrapper_name(raw_value(tree, child))) => {}
                _ => return false,
            },
            Some(SimpleRaw::Void(_) | SimpleRaw::Comment) => {}
            None => {
                if !raw_tags_pair(tree, child, close_open) {
                    return false;
                }
            }
        }
    }
    // A wrapper left open closes with its parent: parse5 pops it with the parent's end
    // tag (or at the end), and it is neither a scope boundary nor reopened, so what
    // follows it among the siblings is just inside it.
    let closes = close_open && closes_open_children(tree.tag_name(parent));
    open.iter().all(|opened| match opened {
        Opened::Wrap(_) => true,
        Opened::Tag(tag) => closes && closes_with_parent(tag),
    })
}

fn rewrite_simple_raw(tree: &mut Tree<'_>, parent: NodeId) {
    // The element each open tag started, with the children gathered for it so far; the
    // first entry gathers `parent`'s own.
    let mut levels = vec![(parent, EMPTY)];
    let mut cursor = tree.nodes[parent as usize].first;
    while cursor != NIL {
        let next = tree.nodes[cursor as usize].next;
        let mut keep = Some(cursor);
        match simple_raw_at(tree, cursor) {
            Some(SimpleRaw::Open(tag)) => {
                let element = tree.element(tag, raw_node_props(tree, cursor), At::None);
                tree.nodes[element as usize].start = tree.nodes[cursor as usize].start;
                levels.push((element, EMPTY));
                keep = None;
            }
            Some(SimpleRaw::Close(_)) => {
                let (element, children) = levels.pop().expect("raw_tags_pair checked the nesting");
                tree.set_children(element, children);
                tree.nodes[element as usize].end = tree.nodes[cursor as usize].end;
                keep = Some(element);
            }
            Some(SimpleRaw::Void(tag)) => {
                let props = raw_node_props(tree, cursor);
                tree.nodes[cursor as usize].kind = NodeKind::Element {
                    tag,
                    props,
                    data: None,
                };
            }
            // Kept raw: it separates the text around it, as parse5's comment node does
            // (and a wrapper's tags, which put the text on each side in another
            // parent), and sanitize then drops it.
            Some(SimpleRaw::Comment | SimpleRaw::WrapOpen | SimpleRaw::WrapClose) => {}
            None => rewrite_simple_raw(tree, cursor),
        }
        if let Some(node) = keep {
            let (_, children) = levels.last_mut().expect("the parent's level stays");
            tree.append(children, node);
        }
        cursor = next;
    }
    // Elements still open end with the parent (see `pair_simple_raw`).
    while levels.len() > 1 {
        let (element, children) = levels.pop().expect("more than the parent's level");
        tree.set_children(element, children);
        let (_, outer) = levels.last_mut().expect("the parent's level stays");
        tree.append(outer, element);
    }
    let (_, children) = levels.pop().expect("only the parent's level is left");
    tree.set_children(parent, children);
}

// ---------------------------------------------------------------------------
// rehype-raw 7.0.0 (parse5 7.3.0 via hast-util-raw 9.1.0) property round trip
// ---------------------------------------------------------------------------

/// Whether splitting `value` on ASCII whitespace gives just `value`.
fn one_token(value: &str) -> bool {
    !value.is_empty() && !value.bytes().any(|byte| byte.is_ascii_whitespace())
}

/// hast-util-to-parse5 attribute serialization then hastscript property parsing.
fn round_trip<'s>(key: &str, value: Value<'s>) -> Option<Value<'s>> {
    let space_separated = matches!(key, "className" | "ariaDescribedBy");
    let attribute: Cow<'s, str> = match value {
        Value::Class(name) if space_separated => return Some(Value::Class(name)),
        Value::Class(name) => return Some(Value::Str(Cow::Borrowed(name))),
        // Joining then splitting gives the list back.
        Value::List(list) if space_separated && list.iter().all(|item| one_token(item)) => {
            return Some(Value::List(list));
        }
        Value::Bool(false) => return None,
        Value::Bool(true) => Cow::Borrowed(""),
        Value::Num(number) => Cow::Owned(number.to_string()),
        Value::Str(string) => string,
        Value::List(mut list) if list.len() == 1 => list.pop().expect("the guard saw one item"),
        Value::List(list) => Cow::Owned(list.join(" ")),
    };
    Some(match key {
        "checked" | "disabled" => Value::Bool(true),
        _ if numeric_property(key) => match attribute.trim().parse::<i64>() {
            Ok(number) => Value::Num(number),
            Err(_) => Value::Str(attribute),
        },
        _ if space_separated => {
            if one_token(&attribute) {
                Value::List(vec![attribute])
            } else {
                Value::List(
                    attribute
                        .split(|c: char| c.is_ascii_whitespace())
                        .filter(|part| !part.is_empty())
                        .map(|part| Cow::Owned(part.to_owned()))
                        .collect(),
                )
            }
        }
        _ => Value::Str(attribute),
    })
}

// ---------------------------------------------------------------------------
// rehype-sanitize 6.0.0 (hast-util-sanitize 5.0.2) with Streamdown's schema: the default
// with no `clobberPrefix`, `tel:` and `streamdown:` links, `streamdown:` sources (remend's
// sentinels for incomplete links and images), and `metastring` on `<code>`.
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
/// Sorted, for binary search.
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
/// Sorted, for binary search.
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

fn specific(tag: &str, key: &str, schema: Schema) -> Option<Allowed> {
    let aria = ARIA.contains(&key);
    Some(match (tag, key) {
        ("a", _) if aria => Allowed::Any,
        ("a", "dataFootnoteBackref" | "dataFootnoteRef" | "href") => Allowed::Any,
        ("a", "className") => Allowed::Values(&["data-footnote-backref"]),
        ("blockquote" | "del" | "ins" | "q", "cite") => Allowed::Any,
        ("code", "className") => Allowed::Language(&[]),
        ("code", "metastring") if schema == Schema::Streamdown => Allowed::Any,
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

fn safe_protocol(key: &str, url: &str, schema: Schema) -> bool {
    let protocols: &[&str] = match key {
        "cite" | "longDesc" => &["http", "https"],
        "src" if schema == Schema::Default => &["http", "https"],
        "href" if schema == Schema::Default => &["http", "https", "irc", "ircs", "mailto", "xmpp"],
        "src" => &["http", "https", "streamdown"],
        "href" => &[
            "http",
            "https",
            "irc",
            "ircs",
            "mailto",
            "xmpp",
            "tel",
            "streamdown",
        ],
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

fn primitive_ok(allowed: &Allowed, key: &str, value: &Value<'_>, schema: Schema) -> bool {
    let string: Cow<'_, str> = match value {
        Value::Str(string) => Cow::Borrowed(string.as_ref()),
        Value::Num(number) => Cow::Owned(number.to_string()),
        Value::Bool(boolean) => Cow::Borrowed(if *boolean { "true" } else { "false" }),
        Value::List(_) | Value::Class(_) => return false,
    };
    if !safe_protocol(key, &string, schema) {
        return false;
    }
    match allowed {
        Allowed::Any => true,
        Allowed::True => matches!(value, Value::Bool(true)),
        Allowed::Values(values) => {
            matches!(value, Value::Str(string) if values.contains(&string.as_ref()))
        }
        Allowed::Language(values) => {
            let language = string
                .strip_prefix("language-")
                .and_then(|rest| rest.chars().next())
                .is_some_and(|first| !matches!(first, '\n' | '\r' | '\u{2028}' | '\u{2029}'));
            language || matches!(value, Value::Str(string) if values.contains(&string.as_ref()))
        }
    }
}

/// `Err` hands the value back so the `*` definitions can try it.
fn sanitize_value<'s>(
    allowed: &Allowed,
    key: &str,
    value: Value<'s>,
    schema: Schema,
) -> Result<Value<'s>, Value<'s>> {
    if let Value::Class(name) = value {
        return Ok(
            if primitive_ok(allowed, key, &Value::Str(Cow::Borrowed(name)), schema) {
                Value::Class(name)
            } else {
                Value::List(Vec::new())
            },
        );
    }
    match value {
        Value::List(list) => Ok(Value::List(
            list.into_iter()
                .filter(|item| primitive_ok(allowed, key, &Value::Str(Cow::Borrowed(item)), schema))
                .collect(),
        )),
        primitive if primitive_ok(allowed, key, &primitive, schema) => Ok(primitive),
        primitive => Err(primitive),
    }
}

fn sanitize_props<'s>(tag: &str, props: Props<'s>, sanitize: &Sanitize) -> Props<'s> {
    let schema = sanitize.schema;
    // Collecting into the same element type reuses the input's allocation.
    let mut result: Props<'s> = props
        .into_iter()
        .filter_map(|(key, value)| {
            let allowed = match (tag, key) {
                ("code", "className") => Some(Allowed::Language(sanitize.code_classes)),
                _ => specific(tag, key, schema),
            };
            let value = match allowed {
                Some(allowed) => sanitize_value(&allowed, key, value, schema),
                None => Err(value),
            };
            let safe = match value {
                Ok(safe) => safe,
                Err(value) if GLOBAL.binary_search(&key).is_ok() => {
                    sanitize_value(&Allowed::Any, key, value, schema).ok()?
                }
                Err(_) => return None,
            };
            Some((
                key,
                if schema == Schema::Default && clobbered(key) {
                    clobber(safe)
                } else {
                    safe
                },
            ))
        })
        .collect();
    if tag == "input" {
        if get(&result, "disabled").is_none() {
            result.push(("disabled", Value::Bool(true)));
        }
        if get(&result, "type").is_none() {
            result.push(("type", Value::Str(Cow::Borrowed("checkbox"))));
        }
    }
    result
}

/// The properties the default schema's `clobber` names.
fn clobbered(key: &str) -> bool {
    matches!(key, "ariaDescribedBy" | "ariaLabelledBy" | "id" | "name")
}

/// The default schema's `clobberPrefix`, before each value.
fn clobber(value: Value<'_>) -> Value<'_> {
    match value {
        Value::Str(string) => Value::Str(Cow::Owned(format!("user-content-{string}"))),
        Value::Num(number) => Value::Str(Cow::Owned(format!("user-content-{number}"))),
        Value::Bool(boolean) => Value::Str(Cow::Owned(format!("user-content-{boolean}"))),
        Value::List(list) => Value::List(
            list.into_iter()
                .map(|item| Cow::Owned(format!("user-content-{item}")))
                .collect(),
        ),
        Value::Class(name) => Value::List(vec![Cow::Owned(format!("user-content-{name}"))]),
    }
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
    tree: &'a mut Tree<'s>,
    sanitize: &'a Sanitize,
    harden: &'a Harden,
    /// How many `table` elements enclose the current one.
    table_depth: usize,
}

fn table_section(tag: &str) -> bool {
    matches!(tag, "table" | "thead" | "tbody" | "tfoot" | "tr")
}

impl<'s> Finisher<'_, 's> {
    /// Removes text parse5 foster-parents out of table structure, in document order.
    fn take_foster(&mut self, element: NodeId, out: &mut Vec<NodeId>) {
        let mut kept = EMPTY;
        let mut cursor = self.tree.nodes[element as usize].first;
        while cursor != NIL {
            let next = self.tree.nodes[cursor as usize].next;
            match &self.tree.nodes[cursor as usize].kind {
                NodeKind::Text(_) => out.push(cursor),
                NodeKind::Element { tag, .. } if table_section(tag) => {
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
    fn push_merged(&mut self, level: &mut Chain, ix: NodeId) {
        let mergeable = level.last != NIL
            && matches!(self.tree.nodes[ix as usize].kind, NodeKind::Text(_))
            && matches!(self.tree.nodes[level.last as usize].kind, NodeKind::Text(_));
        if !mergeable {
            self.tree.append(level, ix);
            return;
        }
        let end = self.tree.nodes[ix as usize].end;
        let NodeKind::Text(value) = &mut self.tree.nodes[ix as usize].kind else {
            unreachable!("mergeable nodes are text");
        };
        let value = std::mem::take(value);
        let into = &mut self.tree.nodes[level.last as usize];
        let NodeKind::Text(existing) = &mut into.kind else {
            unreachable!("mergeable nodes are text");
        };
        push_cow(existing, &value);
        into.end = end;
    }

    fn children(&mut self, parent: NodeId) {
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
                NodeKind::Text(_) => self.text(cursor, &mut out),
                NodeKind::Element { .. } => self.element(cursor, &mut out),
                // `raw_boundary` keeps raw nodes out of this walk.
                NodeKind::Raw(_) | NodeKind::Root => {}
            }
            cursor = next;
        }
        self.tree.set_children(parent, out);
    }

    fn text(&mut self, ix: NodeId, out: &mut Chain) {
        self.tree.nodes[ix as usize].drop_partial_position();
        self.tree.append(out, ix);
    }

    fn element(&mut self, ix: NodeId, out: &mut Chain) {
        let NodeKind::Element { tag, props, data } = &mut self.tree.nodes[ix as usize].kind else {
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
        let mut safe = TAG_NAMES.binary_search(&tag).is_ok();
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
        *props = sanitize_props(tag, std::mem::take(props), self.sanitize);
        if self.harden(ix, tag, self.harden) {
            self.tree.append(out, ix);
        }
    }

    /// `rehype-harden` for one sanitized element whose children are final; `false`
    /// removes it.
    fn harden(&mut self, ix: NodeId, tag: &'static str, harden: &Harden) -> bool {
        let (key, resource) = match tag {
            "a" => ("href", Resource::Link),
            "img" => ("src", Resource::Image),
            _ => return true,
        };
        let props = self.tree.properties_mut(ix).expect("an element");
        let transformed = match get(props, key) {
            Some(Value::Str(url)) => transform_url(url, resource),
            _ => Err(Blocked),
        };
        let changed = match transformed {
            Ok(changed) => changed,
            Err(Blocked) => return self.blocked(ix, resource, harden),
        };
        if let Some(changed) = changed {
            set(props, key, Value::Str(Cow::Owned(changed)));
        }
        if resource == Resource::Link {
            set(props, "target", Value::Str(Cow::Borrowed("_blank")));
            set(
                props,
                "rel",
                Value::Str(Cow::Borrowed("noopener noreferrer")),
            );
        }
        true
    }

    /// harden's `resolveLinkBlockPolicy` / `resolveImageBlockPolicy`: the replacement is a
    /// new, unpositioned span.
    fn blocked(&mut self, ix: NodeId, resource: Resource, harden: &Harden) -> bool {
        let policy = match resource {
            Resource::Link => harden.link_policy,
            Resource::Image => harden.image_policy,
        };
        if policy == BlockPolicy::Remove {
            return false;
        }
        let shown = |value: Option<&Value<'_>>| match value {
            Some(Value::Str(text)) => text.to_string(),
            Some(Value::Num(number)) => number.to_string(),
            Some(Value::Bool(boolean)) => boolean.to_string(),
            _ => "undefined".to_owned(),
        };
        let props = std::mem::take(self.tree.properties_mut(ix).expect("an element"));
        let (props, children): (Props<'s>, Option<NodeId>) = match (resource, policy) {
            (Resource::Link, BlockPolicy::TextOnly) => (Vec::new(), None),
            (Resource::Link, _) => {
                let title = format!("Blocked URL: {}", shown(get(&props, "href")));
                let suffix = self.tree.text(Cow::Borrowed(" [blocked]"));
                (
                    vec![
                        ("title", Value::Str(Cow::Owned(title))),
                        (
                            "class",
                            Value::Str(Cow::Owned(harden.blocked_link_class.clone())),
                        ),
                    ],
                    Some(suffix),
                )
            }
            (Resource::Image, policy) => {
                // `String(node.properties.alt || fallback)`.
                let alt = match get(&props, "alt") {
                    Some(Value::Str(alt)) if !alt.is_empty() => Some(alt.to_string()),
                    Some(Value::Num(number)) if *number != 0 => Some(number.to_string()),
                    Some(Value::Bool(true)) => Some("true".to_owned()),
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
                            Value::Str(Cow::Owned(harden.blocked_image_class.clone())),
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
        node.kind = NodeKind::Element {
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
    tree: &'a Tree<'s>,
    points: &'a Points,
    out: String,
}

impl Json<'_, '_> {
    fn point(&mut self, edge: Edge) {
        let Some(point) = edge.point(self.points) else {
            return;
        };
        self.out.push_str("{\"line\":");
        json_u32(&mut self.out, point.line);
        self.out.push_str(",\"column\":");
        json_u32(&mut self.out, point.column);
        self.out.push_str(",\"offset\":");
        json_u32(&mut self.out, point.offset);
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

    fn children(&mut self, ix: NodeId) {
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

    fn value(&mut self, value: &Value<'_>) {
        match value {
            Value::Str(string) => json_string(&mut self.out, string),
            Value::Bool(boolean) => self.out.push_str(if *boolean { "true" } else { "false" }),
            Value::Num(number) => {
                let _ = write!(self.out, "{number}");
            }
            Value::Class(name) => {
                self.out.push_str("[\"");
                self.out.push_str(name);
                self.out.push_str("\"]");
            }
            Value::List(list) => {
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

    pub(crate) fn node(&mut self, ix: NodeId) {
        let tree = self.tree;
        let node = &tree.nodes[ix as usize];
        match &node.kind {
            NodeKind::Root => {
                self.out.push_str("{\"type\":\"root\",\"children\":");
                self.children(ix);
                if tree.quirks {
                    self.out.push_str(",\"data\":{\"quirksMode\":false}");
                }
            }
            NodeKind::Element { tag, props, data } => {
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
            NodeKind::Text(value) => {
                self.out.push_str("{\"type\":\"text\",\"value\":");
                json_string(&mut self.out, value);
            }
            NodeKind::Raw(value) => {
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

/// rehype-sanitize's schema, plus `className` values `<code>` may keep beyond
/// `language-*`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sanitize {
    /// For example `["math-display", "math-inline"]`, for a rehype plugin after the chain
    /// that reads remark-math's classes.
    pub code_classes: &'static [&'static str],
    pub schema: Schema,
}

/// The schema rehype-sanitize runs with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Schema {
    /// Streamdown 2.7.1's: hast-util-sanitize's default with no `clobberPrefix`, `tel:`
    /// and `streamdown:` links, `streamdown:` sources (remend's sentinels for incomplete
    /// links and images), and `metastring` on `<code>`.
    #[default]
    Streamdown,
    /// hast-util-sanitize 5.0.2's `defaultSchema` as it is (rehype-sanitize with no
    /// options, or Streamdown 1.x's): `user-content-` before `id`, `name` and the ARIA
    /// references, and only the default protocols.
    Default,
}

/// The pipeline's settings. The default is Streamdown's, with no `plugins` prop.
pub struct Options {
    /// Streamdown's `plugins.math` (`@streamdown/math` with `singleDollarTextMath:
    /// false`): `$$` math, and rehype-katex after the chain (see [`Kind::Math`]).
    pub math: bool,
    /// Streamdown's `plugins.cjk` (`@streamdown/cjk`): CJK-friendly emphasis and
    /// strikethrough, and autolink literals that end at CJK punctuation.
    pub cjk: bool,
    /// remark-cjk-friendly 1.2.3 and its strikethrough after remark-gfm, as Streamdown 1.x
    /// attached them (see [`Syntax::legacy_cjk`]), in place of `cjk`.
    pub legacy_cjk: bool,
    /// With `math`: remark-math's `singleDollarTextMath` (`$x$` is math too), its
    /// default, which `@streamdown/math` turns off.
    pub single_dollar: bool,
    /// Streamdown's remarkCodeMeta: a fence's meta string as `metastring` on `<code>`.
    /// On by default.
    pub code_meta: bool,
    /// remark-breaks: every line ending in text is a `<br>`.
    pub breaks: bool,
    /// rehype-slug after the chain: an id on every heading.
    pub slug: Option<crate::extras::Slug>,
    /// rehype-external-links after the chain: `rel` and `target` on links to other
    /// sites.
    pub external_links: Option<crate::extras::ExternalLinks>,
    pub harden: Harden,
    pub sanitize: Sanitize,
    pub plugins: Vec<Box<dyn Plugin>>,
    /// End a simple raw start tag left open (`<sup>1` while its `</sup>` streams) with
    /// its paragraph, cell, list item or heading, as parse5 does, instead of leaving the
    /// document to JavaScript. The element then has no position, so only output without
    /// positions ([`crate::render_markup`]) sets this.
    /// A comment still streaming (no `-->` yet) is removed as parse5 and sanitize remove
    /// it, too.
    pub close_open_raw: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            math: false,
            cjk: false,
            legacy_cjk: false,
            single_dollar: false,
            code_meta: true,
            breaks: false,
            slug: None,
            external_links: None,
            harden: Harden::default(),
            sanitize: Sanitize::default(),
            plugins: Vec::new(),
            close_open_raw: false,
        }
    }
}

impl Options {
    /// The micromark extensions these options turn on.
    #[must_use]
    pub fn syntax(&self) -> Syntax {
        Syntax {
            math: self.math,
            cjk: self.cjk,
            legacy_cjk: self.legacy_cjk,
            single_dollar: self.math && self.single_dollar,
        }
    }
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
        edge.point(self.points).map(|point| point.offset)
    }
}

/// A transform at a fixed point of the pipeline: rill's remark or rehype plugin. A build
/// pairs each with its JavaScript form, which renders whatever rill leaves to JavaScript.
///
/// A document with raw HTML stops before `hast_unsanitized` (parse5 runs in JavaScript,
/// then the JavaScript forms). With math, `hast` runs before KaTeX: math elements are
/// still `<code class="language-math">` (in a `<pre>` for display math), which KaTeX
/// replaces afterwards, so a plugin whose JavaScript form would change KaTeX's output
/// returns [`Error::Unsupported`] from `mdast` for such documents.
pub trait Plugin {
    /// After parsing, before mdast-util-to-hast: where remark plugins run. Returning
    /// [`Error::Unsupported`] leaves the document to JavaScript.
    fn mdast(&self, _tree: &mut mdast::Tree<'_>) -> Result<(), Error> {
        Ok(())
    }

    /// Before rehype-sanitize: a rehype plugin placed ahead of it.
    fn hast_unsanitized(&self, _tree: &mut Tree<'_>, _root: NodeId, _context: &Context<'_>) {}

    /// The final tree (but for KaTeX): a rehype plugin after the chain.
    fn hast(&self, _tree: &mut Tree<'_>, _root: NodeId, _context: &Context<'_>) {}

    /// Whether what the plugin makes of a block depends only on that block's text and
    /// the link definitions before it, not on the rest of the text (positions included).
    /// rill then renders a long block that link definitions made one (Streamdown keeps
    /// such a text in one block) as pieces, of which a streaming update renders the last.
    fn piecewise(&self) -> bool {
        false
    }
}

/// How far [`render`] took the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    /// Final HAST: Streamdown's whole rehype chain ran.
    Complete,
    /// HAST before `rehype-raw`: raw HTML (or a link in a link) needs parse5. Run
    /// rehype-raw, rehype-sanitize and rehype-harden on it in JavaScript, then (with
    /// [`Options::math`]) rehype-katex.
    Raw,
    /// With [`Options::math`]: final HAST except its math elements, left as sanitized
    /// `<code class="language-math">` (in a `<pre>` for display math) for rehype-katex,
    /// which Streamdown runs after the rest of the chain. Rendering each on its own
    /// matches running it on the whole tree; sanitize and harden never see its output.
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

pub(crate) struct Encoder<'a, 's> {
    tree: &'a Tree<'s>,
    points: &'a Points,
    source: &'s str,
    ops: Vec<u32>,
    strings: String,
    units: u32,
    interned: HashMap<&'static str, (u32, u32)>,
}

impl<'a, 's> Encoder<'a, 's> {
    pub(crate) fn new(tree: &'a Tree<'s>, points: &'a Points, source: &'s str) -> Self {
        Encoder {
            tree,
            points,
            source,
            // 7 to 12 opcodes per node, about 10 typically.
            ops: Vec::with_capacity(tree.nodes.len() * 11),
            // Generated strings run to about half the source.
            strings: String::with_capacity(source.len() / 2),
            units: 0,
            interned: HashMap::new(),
        }
    }

    pub(crate) fn push(&mut self, op: u32) {
        self.ops.push(op);
    }

    /// The opcode count so far, for a count patched in later with [`Encoder::patch`].
    pub(crate) fn slot(&mut self) -> usize {
        self.ops.push(0);
        self.ops.len() - 1
    }

    pub(crate) fn patch(&mut self, slot: usize, value: u32) {
        self.ops[slot] = value;
    }

    pub(crate) fn finish(self) -> (Vec<u32>, String) {
        (self.ops, self.strings)
    }

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

    pub(crate) fn string(&mut self, value: &str) {
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

    pub(crate) fn name(&mut self, value: &'static str) {
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
        let Some(point) = edge.point(self.points) else {
            return;
        };
        self.ops.extend([point.line, point.column, point.offset]);
    }

    fn position(&mut self, start: Edge, end: Edge) {
        let flags = u32::from(start != Edge::None) | (u32::from(end != Edge::None) << 1);
        self.ops.push(flags);
        self.point(start);
        self.point(end);
    }

    /// The child count, then the children: the count is patched in after them.
    fn children(&mut self, ix: NodeId) {
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

    fn value(&mut self, value: &Value<'_>) {
        match value {
            Value::Str(string) => {
                self.ops.push(op::STRING);
                self.string(string);
            }
            Value::Bool(true) => self.ops.push(op::TRUE),
            Value::Bool(false) => self.ops.push(op::FALSE),
            Value::Num(number) => self.ops.extend([op::NUMBER, *number as u32]),
            Value::Class(name) => {
                self.ops.extend([op::LIST, 1]);
                self.name(name);
            }
            Value::List(list) => {
                self.ops.extend([op::LIST, list.len() as u32]);
                for item in list {
                    self.string(item);
                }
            }
        }
    }

    pub(crate) fn node(&mut self, ix: NodeId) {
        let tree = self.tree;
        let node = &tree.nodes[ix as usize];
        match &node.kind {
            NodeKind::Root => {
                self.ops.extend([op::ROOT, u32::from(tree.quirks)]);
                self.children(ix);
            }
            NodeKind::Element { tag, props, data } => {
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
            NodeKind::Text(value)
                if value == "\n" && node.start == Edge::None && node.end == Edge::None =>
            {
                self.ops.push(op::NEWLINE);
                return;
            }
            NodeKind::Text(value) => {
                self.ops.push(op::TEXT);
                self.string(value);
            }
            NodeKind::Raw(value) => {
                self.ops.push(op::RAW);
                self.string(value);
            }
        }
        self.position(node.start, node.end);
    }
}

/// A rendered document before serialization, with the MDAST's `points` for its positions.
pub(crate) struct Built<'s> {
    pub(crate) kind: Kind,
    pub(crate) points: Points,
    pub(crate) tree: Tree<'s>,
    pub(crate) root: NodeId,
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

/// Whether elements under `root` nest more than `limit` deep (see
/// [`mdast::MAX_DEPTH`]: the stages after this one recurse per level).
fn nested_deeper(tree: &Tree<'_>, root: NodeId, limit: usize) -> bool {
    // The next sibling to visit at each level.
    let mut pending = vec![tree.nodes[root as usize].first];
    while let Some(&next) = pending.last() {
        if next == NIL {
            pending.pop();
            continue;
        }
        let node = &tree.nodes[next as usize];
        *pending.last_mut().expect("a level") = node.next;
        if node.first != NIL {
            if pending.len() >= limit {
                return true;
            }
            pending.push(node.first);
        }
    }
    false
}

pub(crate) fn build<'s>(source: &'s str, options: &Options) -> Result<Built<'s>, Error> {
    let mut mdast = mdast::parse_arena(source, options.syntax())?;
    if options.cjk {
        mdast::cjk_autolink_boundary(&mut mdast, mdast::Tree::ROOT);
    }
    if options.breaks {
        crate::extras::newline_to_break(&mut mdast);
    }
    for plugin in &options.plugins {
        plugin.mdast(&mut mdast)?;
    }
    let Converted {
        mut tree,
        root,
        raw_boundary,
        math,
    } = convert(&mdast, options);
    // Raw tags paired into elements nest as deep as the text has them.
    if nested_deeper(&tree, root, mdast::MAX_DEPTH) {
        return Err(Error::TooDeep);
    }
    // HAST owns or borrows the source for every string, so only the positions stay.
    let mdast::Tree { points, .. } = mdast;
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
    // With math, everything but the math elements is final here: they stay sanitized
    // code, and the JavaScript tail renders each one (see js/src/math.ts).
    Finisher {
        tree: &mut tree,
        sanitize: &options.sanitize,
        harden: &options.harden,
        table_depth: 0,
    }
    .children(root);
    if let Some(slug) = &options.slug {
        crate::extras::slug_headings(&mut tree, root, slug).map_err(|()| Error::Unsupported)?;
    }
    if let Some(links) = &options.external_links {
        crate::extras::external_links(&mut tree, root, links);
    }
    for plugin in &options.plugins {
        plugin.hast(&mut tree, root, &context);
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
    let mut encoder = Encoder::new(&tree, &points, source);
    encoder.node(root);
    let (ops, strings) = encoder.finish();
    Ok(Encoded { kind, ops, strings })
}

#[cfg(test)]
mod tests {
    use super::{GLOBAL, TAG_NAMES};

    #[test]
    fn lookup_tables_are_sorted() {
        assert!(TAG_NAMES.is_sorted(), "TAG_NAMES is binary searched");
        assert!(GLOBAL.is_sorted(), "GLOBAL is binary searched");
    }
}
