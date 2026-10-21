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
use crate::mdast::{self, AlignKind, At, Error, Ix, MKind, MTree, NIL, Points, Pt, ReferenceKind};

// ---------------------------------------------------------------------------
// Arena HAST
// ---------------------------------------------------------------------------

/// One side of a unist position. parse5 text merges can keep only one side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    None,
    Byte(u32),
    Pt(Pt),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Val<'s> {
    Str(Cow<'s, str>),
    Bool(bool),
    Num(i64),
    List(Vec<Cow<'s, str>>),
    /// A one-class list, `["name"]`, without allocating.
    Class(&'static str),
    /// A number serialized as a string, like `String(n)`.
    Digits(usize),
}

type Props<'s> = Vec<(&'static str, Val<'s>)>;

#[derive(Clone, Debug)]
pub enum HKind<'s> {
    Root,
    Element {
        tag: &'static str,
        props: Props<'s>,
        /// `data.meta` from mdast-util-to-hast's code handler.
        meta: Option<String>,
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

pub struct HTree<'s> {
    pub nodes: Vec<HNode<'s>>,
    /// `data.quirksMode` from `rehype-raw`.
    pub quirks: bool,
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

fn edges(at: At) -> (Edge, Edge) {
    match at {
        At::None => (Edge::None, Edge::None),
        At::Bytes(start, end) => (Edge::Byte(start), Edge::Byte(end)),
        At::Points(start, end) => (Edge::Pt(start), Edge::Pt(end)),
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
                meta: None,
            },
            at,
        )
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

    fn tag(&self, ix: Ix) -> Option<&'static str> {
        match &self.nodes[ix as usize].kind {
            HKind::Element { tag, .. } => Some(tag),
            _ => None,
        }
    }

    fn props_mut(&mut self, ix: Ix) -> Option<&mut Props<'s>> {
        match &mut self.nodes[ix as usize].kind {
            HKind::Element { props, .. } => Some(props),
            _ => None,
        }
    }

    /// mdast-util-to-hast `wrap`: newline text nodes between (and around, if loose).
    fn wrap(&mut self, nodes: Chain, loose: bool) -> Chain {
        let mut out = EMPTY;
        if loose {
            let newline = self.text("\n");
            self.append(&mut out, newline);
        }
        let mut cursor = nodes.first;
        let mut first = true;
        while cursor != NIL {
            let next = self.nodes[cursor as usize].next;
            if !first {
                let newline = self.text("\n");
                self.append(&mut out, newline);
            }
            self.append(&mut out, cursor);
            first = false;
            cursor = next;
        }
        if loose && nodes.first != NIL {
            let newline = self.text("\n");
            self.append(&mut out, newline);
        }
        out
    }
}

fn set<'s>(props: &mut Props<'s>, key: &'static str, value: Val<'s>) {
    match props.iter_mut().find(|(name, _)| *name == key) {
        Some((_, slot)) => *slot = value,
        None => props.push((key, value)),
    }
}

fn get<'p, 's>(props: &'p Props<'s>, key: &str) -> Option<&'p Val<'s>> {
    props
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value)
}

fn class<'s>(name: &'static str) -> Val<'s> {
    Val::Class(name)
}

impl<'s> Val<'s> {
    /// Normalizes the allocation-free variants for code that inspects values.
    fn expanded(self) -> Val<'s> {
        match self {
            Val::Class(name) => Val::List(vec![Cow::Borrowed(name)]),
            Val::Digits(n) => Val::Str(Cow::Owned(n.to_string())),
            other => other,
        }
    }
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
    if !value.contains(['\n', '\r']) {
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
    if out.len() == value.len() {
        value.clone()
    } else {
        Cow::Owned(out)
    }
}

/// micromark-util-sanitize-uri 2.0.1 `normalizeUri`.
pub fn normalize_uri<'s>(value: &Cow<'s, str>) -> Cow<'s, str> {
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
        return value.clone();
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
        for encoded in character.encode_utf8(&mut buffer).bytes() {
            let _ = write!(out, "%{encoded:02X}");
        }
        index += character.len_utf8();
    }
    Cow::Owned(out)
}

fn js_whitespace(c: char) -> bool {
    rill_cmark::autolink_literal::unicode_whitespace(c)
}

fn revert_suffix(kind: ReferenceKind, label: &str, identifier: &str) -> String {
    match kind {
        ReferenceKind::Collapsed => "][]".into(),
        ReferenceKind::Full => format!("][{}]", if label.is_empty() { identifier } else { label }),
        ReferenceKind::Shortcut => "]".into(),
    }
}

// ---------------------------------------------------------------------------
// mdast-util-to-hast 13.2.1
// ---------------------------------------------------------------------------

struct ToHast<'t, 's> {
    m: &'t MTree<'s>,
    h: HTree<'s>,
    definitions: HashMap<String, (Cow<'s, str>, Option<Cow<'s, str>>)>,
    footnotes: HashMap<String, Ix>,
    footnote_order: Vec<String>,
    footnote_counts: HashMap<String, usize>,
    /// Raw HTML or links inside links: parse5 effects the native raw stage omits.
    raw_boundary: bool,
    link_depth: usize,
    math: bool,
}

impl<'s> ToHast<'_, 's> {
    fn collect(&mut self, ix: Ix) {
        match &self.m.node(ix).kind {
            MKind::Definition {
                url,
                title,
                identifier,
                ..
            } => {
                self.definitions
                    .entry(identifier.to_uppercase())
                    .or_insert_with(|| (url.clone(), title.clone()));
            }
            MKind::FootnoteDefinition { identifier, .. } => {
                self.footnotes
                    .entry(identifier.to_uppercase())
                    .or_insert(ix);
            }
            _ => {}
        }
        for child in self.m.children(ix) {
            self.collect(child);
        }
    }

    fn defined(&self, identifier: &str) -> bool {
        self.definitions.contains_key(&identifier.to_uppercase())
    }

    /// Handlers that return an array skip the after-break trim.
    fn returns_array(&self, ix: Ix) -> bool {
        match &self.m.node(ix).kind {
            MKind::Break => true,
            MKind::LinkReference { identifier, .. } | MKind::ImageReference { identifier, .. } => {
                !self.defined(identifier)
            }
            _ => false,
        }
    }

    fn all(&mut self, parent: Ix) -> Chain {
        let mut values = EMPTY;
        let mut previous_break = false;
        for child in self.m.children(parent) {
            let result = self.one(child, parent);
            if result.first != NIL && previous_break && !self.returns_array(child) {
                let head = result.first;
                let target = match &self.h.nodes[head as usize].kind {
                    HKind::Text(_) => Some(head),
                    HKind::Element { .. } => {
                        let first = self.h.nodes[head as usize].first;
                        (first != NIL
                            && matches!(self.h.nodes[first as usize].kind, HKind::Text(_)))
                        .then_some(first)
                    }
                    _ => None,
                };
                if let Some(target) = target
                    && let HKind::Text(value) = &mut self.h.nodes[target as usize].kind
                {
                    *value = trim_markdown_space_start(value);
                }
            }
            previous_break = matches!(self.m.node(child).kind, MKind::Break);
            self.h.extend(&mut values, result);
        }
        values
    }

    fn element(&mut self, tag: &'static str, ix: Ix, props: Props<'s>) -> Chain {
        let children = self.all(ix);
        let element = self.h.element(tag, props, self.m.node(ix).at);
        self.h.set_children(element, children);
        self.h.one_chain(element)
    }

    fn wrapped(&mut self, tag: &'static str, ix: Ix, props: Props<'s>, loose: bool) -> Chain {
        let children = self.all(ix);
        let children = self.h.wrap(children, loose);
        let element = self.h.element(tag, props, self.m.node(ix).at);
        self.h.set_children(element, children);
        self.h.one_chain(element)
    }

    fn leaf_in(&mut self, tag: &'static str, props: Props<'s>, at: At, child: Ix) -> Ix {
        let element = self.h.element(tag, props, at);
        self.h.set_children(
            element,
            Chain {
                first: child,
                last: child,
            },
        );
        element
    }

    fn one(&mut self, ix: Ix, parent: Ix) -> Chain {
        let at = self.m.node(ix).at;
        match &self.m.node(ix).kind {
            MKind::Root => {
                let children = self.all(ix);
                let children = self.h.wrap(children, false);
                let root = self.h.push(HKind::Root, at);
                self.h.set_children(root, children);
                self.h.one_chain(root)
            }
            MKind::Blockquote => self.wrapped("blockquote", ix, Vec::new(), true),
            MKind::Break => {
                let br = self.h.element("br", Vec::new(), at);
                let mut chain = self.h.one_chain(br);
                let newline = self.h.text("\n");
                self.h.append(&mut chain, newline);
                chain
            }
            MKind::Code {
                value, lang, meta, ..
            } => {
                let value = if value.is_empty() {
                    Cow::Borrowed("")
                } else {
                    Cow::Owned(format!("{value}\n"))
                };
                let mut props = Vec::new();
                if let Some(lang) = lang.as_deref().filter(|lang| !lang.is_empty()) {
                    let first = lang.split(js_whitespace).next().unwrap_or("");
                    props.push((
                        "className",
                        Val::List(vec![Cow::Owned(format!("language-{first}"))]),
                    ));
                }
                let meta = meta.clone().filter(|meta| !meta.is_empty());
                let text = self.h.text(value);
                let code = self.h.push(
                    HKind::Element {
                        tag: "code",
                        props,
                        meta,
                    },
                    at,
                );
                self.h.set_children(
                    code,
                    Chain {
                        first: text,
                        last: text,
                    },
                );
                let pre = self.leaf_in("pre", Vec::new(), at, code);
                self.h.one_chain(pre)
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
            MKind::ThematicBreak => {
                let hr = self.h.element("hr", Vec::new(), at);
                self.h.one_chain(hr)
            }
            MKind::FootnoteReference { identifier, .. } => {
                let id = identifier.to_uppercase();
                let safe_id = normalize_uri(&Cow::Owned(id.to_lowercase())).into_owned();
                let counter = match self.footnote_order.iter().position(|known| *known == id) {
                    Some(index) if self.footnote_counts.contains_key(&id) => index + 1,
                    _ => {
                        self.footnote_order.push(id.clone());
                        self.footnote_order.len()
                    }
                };
                let reuse = self.footnote_counts.get(&id).copied().unwrap_or(0) + 1;
                self.footnote_counts.insert(id, reuse);
                let suffix = if reuse > 1 {
                    format!("-{reuse}")
                } else {
                    String::new()
                };
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
                    ("ariaDescribedBy", class("footnote-label")),
                ];
                if self.link_depth > 0 {
                    self.raw_boundary = true;
                }
                let label = self.h.text(Cow::Owned(counter.to_string()));
                let link = self.leaf_in("a", props, at, label);
                let sup = self.leaf_in("sup", Vec::new(), at, link);
                self.h.one_chain(sup)
            }
            MKind::Html(value) => {
                self.raw_boundary = true;
                let raw = self.h.push(HKind::Raw(value.clone()), at);
                self.h.one_chain(raw)
            }
            MKind::Image { url, title, alt } => {
                let mut props = vec![
                    ("src", Val::Str(normalize_uri(url))),
                    ("alt", Val::Str(Cow::Owned(alt.clone()))),
                ];
                if let Some(title) = title {
                    props.push(("title", Val::Str(title.clone())));
                }
                let img = self.h.element("img", props, at);
                self.h.one_chain(img)
            }
            MKind::ImageReference {
                kind,
                identifier,
                label,
                alt,
            } => match self.definitions.get(&identifier.to_uppercase()).cloned() {
                None => {
                    let suffix = revert_suffix(*kind, label, identifier);
                    let text = self.h.text(Cow::Owned(format!("![{alt}{suffix}")));
                    self.h.one_chain(text)
                }
                Some((url, title)) => {
                    let mut props = vec![
                        ("src", Val::Str(normalize_uri(&url))),
                        ("alt", Val::Str(Cow::Owned(alt.clone()))),
                    ];
                    if let Some(title) = title {
                        props.push(("title", Val::Str(title)));
                    }
                    let img = self.h.element("img", props, at);
                    self.h.one_chain(img)
                }
            },
            MKind::InlineCode(value) => {
                let value = if value.contains(['\n', '\r']) {
                    Cow::Owned(value.replace("\r\n", " ").replace(['\r', '\n'], " "))
                } else {
                    value.clone()
                };
                let text = self.h.push(HKind::Text(value), at);
                let code = self.leaf_in("code", Vec::new(), at, text);
                self.h.one_chain(code)
            }
            MKind::Link { url, title } => {
                let mut props = vec![("href", Val::Str(normalize_uri(url)))];
                if let Some(title) = title {
                    props.push(("title", Val::Str(title.clone())));
                }
                self.link(ix, props)
            }
            MKind::LinkReference {
                kind,
                identifier,
                label,
            } => match self.definitions.get(&identifier.to_uppercase()).cloned() {
                None => {
                    let suffix = revert_suffix(*kind, label, identifier);
                    let mut contents = self.all(ix);
                    let head = (contents.first != NIL)
                        .then(|| &mut self.h.nodes[contents.first as usize].kind);
                    match head {
                        Some(HKind::Text(value)) => *value = Cow::Owned(format!("[{value}")),
                        _ => {
                            let open = self.h.text("[");
                            self.h.prepend(&mut contents, open);
                        }
                    }
                    match &mut self.h.nodes[contents.last as usize].kind {
                        HKind::Text(value) => value.to_mut().push_str(&suffix),
                        _ => {
                            let close = self.h.text(Cow::Owned(suffix));
                            self.h.append(&mut contents, close);
                        }
                    }
                    contents
                }
                Some((url, title)) => {
                    let mut props = vec![("href", Val::Str(normalize_uri(&url)))];
                    if let Some(title) = title {
                        props.push(("title", Val::Str(title)));
                    }
                    self.link(ix, props)
                }
            },
            MKind::List { ordered, start, .. } => {
                let tag = if *ordered { "ol" } else { "ul" };
                let mut props = Vec::new();
                if let Some(start) = start.filter(|start| *start != 1) {
                    props.push(("start", Val::Num(i64::from(start))));
                }
                let results = self.all(ix);
                let mut cursor = results.first;
                while cursor != NIL {
                    let task = matches!(
                        &self.h.nodes[cursor as usize].kind,
                        HKind::Element { tag: "li", props, .. }
                            if matches!(get(props, "className"), Some(Val::Class("task-list-item")))
                    );
                    if task {
                        props.push(("className", class("contains-task-list")));
                        break;
                    }
                    cursor = self.h.nodes[cursor as usize].next;
                }
                let children = self.h.wrap(results, true);
                let element = self.h.element(tag, props, at);
                self.h.set_children(element, children);
                self.h.one_chain(element)
            }
            MKind::ListItem { spread, checked } => self.list_item(ix, parent, *spread, *checked),
            MKind::Table(align) => self.table(ix, &align.clone(), at),
            MKind::Text(value) => {
                let text = self.h.push(HKind::Text(trim_lines(value)), at);
                self.h.one_chain(text)
            }
            MKind::Math { value, .. } => {
                self.math = true;
                let text = self.h.text(value.clone());
                let class = Val::List(vec![
                    Cow::Borrowed("language-math"),
                    Cow::Borrowed("math-display"),
                ]);
                let code = self.leaf_in("code", vec![("className", class)], At::None, text);
                let pre = self.leaf_in("pre", Vec::new(), at, code);
                self.h.one_chain(pre)
            }
            MKind::InlineMath(value) => {
                self.math = true;
                let text = self.h.text(value.clone());
                let class = Val::List(vec![
                    Cow::Borrowed("language-math"),
                    Cow::Borrowed("math-inline"),
                ]);
                let code = self.leaf_in("code", vec![("className", class)], at, text);
                self.h.one_chain(code)
            }
            MKind::Definition { .. } | MKind::FootnoteDefinition { .. } => EMPTY,
            // Rows and cells are handled by their table; the unknown handler's `div`.
            MKind::TableRow | MKind::TableCell => self.element("div", ix, Vec::new()),
        }
    }

    fn link(&mut self, ix: Ix, props: Props<'s>) -> Chain {
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
        let loose = match &self.m.node(parent).kind {
            MKind::List {
                spread: list_spread,
                ..
            } => {
                *list_spread
                    || self.m.children(parent).any(|item| {
                        matches!(self.m.node(item).kind, MKind::ListItem { spread: true, .. })
                    })
            }
            _ => spread,
        };
        let mut props = Vec::new();
        if let Some(checked) = checked {
            if results.first == NIL || self.h.tag(results.first) != Some("p") {
                let paragraph = self.h.element("p", Vec::new(), At::None);
                self.h.prepend(&mut results, paragraph);
            }
            let paragraph = results.first;
            let mut children = self.h.children_of(paragraph);
            if children.first != NIL {
                let space = self.h.text(" ");
                self.h.prepend(&mut children, space);
            }
            let input = self.h.element(
                "input",
                vec![
                    ("type", Val::Str(Cow::Borrowed("checkbox"))),
                    ("checked", Val::Bool(checked)),
                    ("disabled", Val::Bool(true)),
                ],
                At::None,
            );
            self.h.prepend(&mut children, input);
            self.h.set_children(paragraph, children);
            props.push(("className", class("task-list-item")));
        }
        let tail_is_paragraph = results.last != NIL && self.h.tag(results.last) == Some("p");
        let nonempty = results.first != NIL;
        let mut children = EMPTY;
        let mut cursor = results.first;
        let mut index = 0;
        while cursor != NIL {
            let next = self.h.nodes[cursor as usize].next;
            let is_paragraph = self.h.tag(cursor) == Some("p");
            if loose || index != 0 || !is_paragraph {
                let newline = self.h.text("\n");
                self.h.append(&mut children, newline);
            }
            if is_paragraph && !loose {
                let inner = self.h.children_of(cursor);
                self.h.extend(&mut children, inner);
            } else {
                self.h.append(&mut children, cursor);
            }
            cursor = next;
            index += 1;
        }
        if nonempty && (loose || !tail_is_paragraph) {
            let newline = self.h.text("\n");
            self.h.append(&mut children, newline);
        }
        let li = self.h.element("li", props, self.m.node(ix).at);
        self.h.set_children(li, children);
        self.h.one_chain(li)
    }

    fn table(&mut self, ix: Ix, align: &[AlignKind], at: At) -> Chain {
        let mut rows = EMPTY;
        for (index, row) in self.m.children(ix).enumerate() {
            let row = self.table_row(row, index, align);
            self.h.append(&mut rows, row);
        }
        let mut content = EMPTY;
        let first_row = self.m.node(ix).first;
        if rows.first != NIL {
            let first = rows.first;
            let rest = self.h.nodes[first as usize].next;
            let head_rows = self.h.one_chain(first);
            let children = self.h.wrap(head_rows, true);
            let head = self
                .h
                .element("thead", Vec::new(), self.m.node(first_row).at);
            self.h.set_children(head, children);
            self.h.append(&mut content, head);
            if rest != NIL {
                let body_rows = Chain {
                    first: rest,
                    last: rows.last,
                };
                let children = self.h.wrap(body_rows, true);
                let second = self.m.node(first_row).next;
                let start = self.m.pt_range(self.m.node(second).at);
                let end = self.m.pt_range(self.m.node(self.m.node(ix).last).at);
                let body_at = match (start, end) {
                    (Some((start, _)), Some((_, end))) => At::Points(start, end),
                    _ => At::None,
                };
                let body = self.h.element("tbody", Vec::new(), body_at);
                self.h.set_children(body, children);
                self.h.append(&mut content, body);
            }
        }
        let children = self.h.wrap(content, true);
        let table = self.h.element("table", Vec::new(), at);
        self.h.set_children(table, children);
        self.h.one_chain(table)
    }

    fn table_row(&mut self, row: Ix, index: usize, align: &[AlignKind]) -> Ix {
        let tag = if index == 0 { "th" } else { "td" };
        let mut cells = EMPTY;
        let mut source = self.m.node(row).first;
        for alignment in align {
            let mut props = Vec::new();
            let value = match alignment {
                AlignKind::Left => Some("left"),
                AlignKind::Right => Some("right"),
                AlignKind::Center => Some("center"),
                AlignKind::None => None,
            };
            if let Some(value) = value {
                props.push(("align", Val::Str(Cow::Borrowed(value))));
            }
            let cell = if source == NIL {
                self.h.element(tag, props, At::None)
            } else {
                let children = self.all(source);
                let cell = self.h.element(tag, props, self.m.node(source).at);
                self.h.set_children(cell, children);
                source = self.m.node(source).next;
                cell
            };
            self.h.append(&mut cells, cell);
        }
        let children = self.h.wrap(cells, true);
        let tr = self.h.element("tr", Vec::new(), self.m.node(row).at);
        self.h.set_children(tr, children);
        tr
    }

    fn footer(&mut self) -> Option<Ix> {
        let mut items = EMPTY;
        for (reference_index, id) in self.footnote_order.clone().into_iter().enumerate() {
            let Some(&definition) = self.footnotes.get(&id) else {
                continue;
            };
            let MKind::FootnoteDefinition { identifier, .. } = &self.m.node(definition).kind else {
                continue;
            };
            let safe_id =
                normalize_uri(&Cow::Owned(identifier.to_uppercase().to_lowercase())).into_owned();
            let mut content = self.all(definition);
            let counts = self.footnote_counts.get(&id).copied().unwrap_or(0);
            let mut back = EMPTY;
            for rereference in 1..=counts {
                if back.first != NIL {
                    let space = self.h.text(" ");
                    self.h.append(&mut back, space);
                }
                let suffix = if rereference > 1 {
                    format!("-{rereference}")
                } else {
                    String::new()
                };
                let arrow = self.h.text("↩");
                let mut children = self.h.one_chain(arrow);
                if rereference > 1 {
                    let number = self.h.text(Cow::Owned(rereference.to_string()));
                    let sup = self.leaf_in("sup", Vec::new(), At::None, number);
                    self.h.append(&mut children, sup);
                }
                let link = self.h.element(
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
                        ("className", class("data-footnote-backref")),
                    ],
                    At::None,
                );
                self.h.set_children(link, children);
                self.h.append(&mut back, link);
            }
            if content.last != NIL && self.h.tag(content.last) == Some("p") {
                let tail = content.last;
                let mut inner = self.h.children_of(tail);
                let tail_text =
                    (inner.last != NIL).then(|| &mut self.h.nodes[inner.last as usize].kind);
                match tail_text {
                    Some(HKind::Text(value)) => value.to_mut().push(' '),
                    _ => {
                        let space = self.h.text(" ");
                        self.h.append(&mut inner, space);
                    }
                }
                self.h.extend(&mut inner, back);
                self.h.set_children(tail, inner);
            } else {
                self.h.extend(&mut content, back);
            }
            let children = self.h.wrap(content, true);
            let li = self.h.element(
                "li",
                vec![(
                    "id",
                    Val::Str(Cow::Owned(format!("user-content-fn-{safe_id}"))),
                )],
                self.m.node(definition).at,
            );
            self.h.set_children(li, children);
            self.h.append(&mut items, li);
        }
        if items.first == NIL {
            return None;
        }
        let label_text = self.h.text("Footnotes");
        let label = self.leaf_in(
            "h2",
            vec![
                ("className", class("sr-only")),
                ("id", Val::Str(Cow::Borrowed("footnote-label"))),
            ],
            At::None,
            label_text,
        );
        let list_children = self.h.wrap(items, true);
        let list = self.h.element("ol", Vec::new(), At::None);
        self.h.set_children(list, list_children);
        let mut children = self.h.one_chain(label);
        let newline = self.h.text("\n");
        self.h.append(&mut children, newline);
        self.h.append(&mut children, list);
        let newline = self.h.text("\n");
        self.h.append(&mut children, newline);
        let section = self.h.element(
            "section",
            vec![
                ("dataFootnotes", Val::Bool(true)),
                ("className", class("footnotes")),
            ],
            At::None,
        );
        self.h.set_children(section, children);
        Some(section)
    }
}

/// HAST plus what the native raw stage must leave to JavaScript.
pub struct Converted<'s> {
    pub tree: HTree<'s>,
    pub root: Ix,
    pub raw_boundary: bool,
    pub math: bool,
}

pub fn to_hast<'s>(m: &MTree<'s>) -> Converted<'s> {
    let mut state = ToHast {
        m,
        h: HTree {
            nodes: Vec::with_capacity(m.nodes.len() * 2),
            quirks: false,
        },
        definitions: HashMap::new(),
        footnotes: HashMap::new(),
        footnote_order: Vec::new(),
        footnote_counts: HashMap::new(),
        raw_boundary: false,
        link_depth: 0,
        math: false,
    };
    state.collect(MTree::ROOT);
    let root = state.one(MTree::ROOT, MTree::ROOT).first;
    if let Some(section) = state.footer() {
        let mut children = state.h.children_of(root);
        let newline = state.h.text("\n");
        state.h.append(&mut children, newline);
        state.h.append(&mut children, section);
        state.h.set_children(root, children);
    }
    Converted {
        root,
        raw_boundary: state.raw_boundary,
        math: state.math,
        tree: state.h,
    }
}

// ---------------------------------------------------------------------------
// rehype-raw 7.0.0 (parse5 7.3.0 via hast-util-raw 9.1.0) property round trip
// ---------------------------------------------------------------------------

/// hast-util-to-parse5 attribute serialization then hastscript property parsing.
fn round_trip<'s>(key: &str, value: Val<'s>) -> Option<Val<'s>> {
    if let Val::Class(name) = value {
        return Some(match key {
            "className" | "ariaDescribedBy" => Val::Class(name),
            _ => Val::Str(Cow::Borrowed(name)),
        });
    }
    let attribute: Cow<'s, str> = match value.expanded() {
        Val::Bool(false) => return None,
        Val::Bool(true) => Cow::Borrowed(""),
        Val::Num(number) => Cow::Owned(number.to_string()),
        Val::Str(string) => string,
        Val::List(mut list) if list.len() == 1 => list.pop().unwrap_or_default(),
        Val::List(list) => Cow::Owned(list.join(" ")),
        Val::Class(_) | Val::Digits(_) => return None,
    };
    Some(match key {
        "checked" | "disabled" => Val::Bool(true),
        "start" => match attribute.trim().parse::<i64>() {
            Ok(number) => Val::Num(number),
            Err(_) => Val::Str(attribute),
        },
        "className" | "ariaDescribedBy" => {
            if !attribute.is_empty() && attribute.bytes().all(|b| !b.is_ascii_whitespace()) {
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
        Val::Str(s) => Cow::Borrowed(s.as_ref()),
        Val::Num(n) => Cow::Owned(n.to_string()),
        Val::Bool(b) => Cow::Borrowed(if *b { "true" } else { "false" }),
        Val::List(_) | Val::Class(_) | Val::Digits(_) => return false,
    };
    if !safe_protocol(key, &string) {
        return false;
    }
    match allowed {
        Allowed::Any => true,
        Allowed::True => matches!(value, Val::Bool(true)),
        Allowed::Values(values) => matches!(value, Val::Str(s) if values.contains(&s.as_ref())),
        Allowed::Language(values) => {
            let language = string
                .strip_prefix("language-")
                .and_then(|rest| rest.chars().next())
                .is_some_and(|c| !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'));
            language || matches!(value, Val::Str(s) if values.contains(&s.as_ref()))
        }
    }
}

fn clobbered(key: &str) -> bool {
    matches!(key, "ariaDescribedBy" | "ariaLabelledBy" | "id" | "name")
}

fn clobber(value: Val<'_>) -> Val<'_> {
    match value.expanded() {
        Val::Str(s) => Val::Str(Cow::Owned(format!("user-content-{s}"))),
        Val::Num(n) => Val::Str(Cow::Owned(format!("user-content-{n}"))),
        Val::Bool(b) => Val::Str(Cow::Owned(format!("user-content-{b}"))),
        Val::List(list) => Val::List(
            list.into_iter()
                .map(|s| Cow::Owned(format!("user-content-{s}")))
                .collect(),
        ),
        other => other,
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
    let value = value.expanded();
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

fn sanitize_props<'s>(tag: &str, props: Props<'s>) -> Props<'s> {
    let mut result: Props<'s> = Vec::with_capacity(props.len());
    for (key, value) in props {
        let value = match specific(tag, key) {
            Some(allowed) => sanitize_value(&allowed, key, value),
            None => Err(value),
        };
        let safe = match value {
            Ok(safe) => Some(safe),
            Err(value) if GLOBAL.contains(&key) => sanitize_value(&Allowed::Any, key, value).ok(),
            Err(_) => None,
        };
        if let Some(safe) = safe {
            result.push((key, if clobbered(key) { clobber(safe) } else { safe }));
        }
    }
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

fn parse_url(input: &str) -> Option<url::Url> {
    match url::Url::parse(input) {
        Ok(parsed) => Some(parsed),
        Err(_) if path_relative(input) => {
            url::Url::parse("http://example.com").ok()?.join(input).ok()
        }
        Err(_) => None,
    }
}

fn js_search(parsed: &url::Url) -> String {
    parsed
        .query()
        .filter(|q| !q.is_empty())
        .map(|q| format!("?{q}"))
        .unwrap_or_default()
}

fn js_hash(parsed: &url::Url) -> String {
    parsed
        .fragment()
        .filter(|f| !f.is_empty())
        .map(|f| format!("#{f}"))
        .unwrap_or_default()
}

/// Why `transformUrl` returned null.
struct Blocked;

/// `transformUrl`; `Ok(None)` when the URL comes back unchanged.
fn transform_url(url: &str, image: bool) -> Result<Option<String>, Blocked> {
    if url.is_empty() {
        return Err(Blocked);
    }
    if url.starts_with('#') && !image {
        let base = url::Url::parse("http://example.com").map_err(|_| Blocked)?;
        if base.join(url).is_ok_and(|test| js_hash(&test) == url) {
            return Ok(None);
        }
    }
    if url.starts_with("data:") {
        return if image && url.starts_with("data:image/") {
            Ok(None)
        } else {
            Err(Blocked)
        };
    }
    if let Some(after) = url.strip_prefix("blob:") {
        let parsed = url::Url::parse(url).map_err(|_| Blocked)?;
        return if parsed.scheme() == "blob" && !after.is_empty() && after != "invalid" {
            Ok(None)
        } else {
            Err(Blocked)
        };
    }
    let parsed = parse_url(url).ok_or(Blocked)?;
    if matches!(parsed.scheme(), "javascript" | "data" | "file" | "vbscript") {
        return Err(Blocked);
    }
    let result = if matches!(parsed.scheme(), "http" | "https") && path_relative(url) {
        format!(
            "{}{}{}",
            parsed.path(),
            js_search(&parsed),
            js_hash(&parsed)
        )
    } else {
        parsed.as_str().to_owned()
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
    /// `None` with math: KaTeX runs before sanitize and harden, in JavaScript.
    harden: Option<&'a Harden>,
    ancestors: Vec<&'static str>,
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
        let value = match &mut self.tree.nodes[ix as usize].kind {
            HKind::Text(value) => std::mem::take(value),
            _ => Cow::Borrowed(""),
        };
        let into = &mut self.tree.nodes[level.last as usize];
        if let HKind::Text(existing) = &mut into.kind {
            existing.to_mut().push_str(&value);
        }
        into.end = end;
    }

    fn children(&mut self, parent: Ix) {
        // parse5: fostered table text and adjacent text merge at this level.
        let mut level = EMPTY;
        let mut cursor = self.tree.nodes[parent as usize].first;
        let mut fostered = Vec::new();
        while cursor != NIL {
            let next = self.tree.nodes[cursor as usize].next;
            if self.tree.tag(cursor) == Some("table") {
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
        // hast-util-sanitize keeps only complete positions.
        let node = &mut self.tree.nodes[ix as usize];
        if self.harden.is_some() && (node.start == Edge::None || node.end == Edge::None) {
            node.start = Edge::None;
            node.end = Edge::None;
        }
        self.tree.append(out, ix);
    }

    fn element(&mut self, ix: Ix, out: &mut Chain) {
        let tag = self.tree.tag(ix).unwrap_or("");
        if let HKind::Element { props, meta, .. } = &mut self.tree.nodes[ix as usize].kind {
            *meta = None;
            *props = std::mem::take(props)
                .into_iter()
                .filter_map(|(key, value)| round_trip(key, value).map(|value| (key, value)))
                .collect();
        }
        self.ancestors.push(tag);
        self.children(ix);
        self.ancestors.pop();
        let Some(harden) = self.harden else {
            // Math: sanitize and harden run in JavaScript after KaTeX.
            self.tree.append(out, ix);
            return;
        };
        let mut safe = TAG_NAMES.contains(&tag);
        if safe && matches!(tag, "tbody" | "td" | "th" | "thead" | "tfoot" | "tr") {
            safe = self.ancestors.contains(&"table");
        }
        if !safe {
            if tag != "script" {
                let children = self.tree.children_of(ix);
                self.tree.extend(out, children);
            }
            return;
        }
        let node = &mut self.tree.nodes[ix as usize];
        if node.start == Edge::None || node.end == Edge::None {
            node.start = Edge::None;
            node.end = Edge::None;
        }
        if let Some(props) = self.tree.props_mut(ix) {
            *props = sanitize_props(tag, std::mem::take(props));
        }
        if self.harden(ix, tag, harden) {
            self.tree.append(out, ix);
        }
    }

    /// `rehype-harden` for one sanitized element whose children are final; `false`
    /// removes it.
    fn harden(&mut self, ix: Ix, tag: &'static str, harden: &Harden) -> bool {
        let (key, image) = match tag {
            "a" => ("href", false),
            "img" => ("src", true),
            _ => return true,
        };
        let Some(props) = self.tree.props_mut(ix) else {
            return true;
        };
        let transformed = match get(props, key) {
            Some(Val::Str(url)) => transform_url(url, image),
            _ => Err(Blocked),
        };
        let changed = match transformed {
            Ok(changed) => changed,
            Err(Blocked) => return self.blocked(ix, image, harden),
        };
        if let Some(changed) = changed {
            set(props, key, Val::Str(Cow::Owned(changed)));
        }
        if !image {
            set(props, "target", Val::Str(Cow::Borrowed("_blank")));
            set(props, "rel", Val::Str(Cow::Borrowed("noopener noreferrer")));
        }
        true
    }

    /// harden's `resolveLinkBlockPolicy` / `resolveImageBlockPolicy`: the replacement is a
    /// new, unpositioned span.
    fn blocked(&mut self, ix: Ix, image: bool, harden: &Harden) -> bool {
        let policy = if image {
            harden.image_policy
        } else {
            harden.link_policy
        };
        if policy == BlockPolicy::Remove {
            return false;
        }
        let shown = |value: Option<&Val<'_>>| match value {
            Some(Val::Str(text)) => text.to_string(),
            Some(Val::Num(n)) => n.to_string(),
            Some(Val::Bool(b)) => b.to_string(),
            _ => "undefined".to_owned(),
        };
        let props = self
            .tree
            .props_mut(ix)
            .map(std::mem::take)
            .unwrap_or_default();
        let (props, children): (Props<'s>, Option<Ix>) = match (image, policy) {
            (false, BlockPolicy::TextOnly) => (Vec::new(), None),
            (false, _) => {
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
            (true, policy) => {
                // `String(node.properties.alt || fallback)`.
                let alt = match get(&props, "alt") {
                    Some(Val::Str(alt)) if !alt.is_empty() => Some(alt.to_string()),
                    Some(Val::Num(n)) if *n != 0 => Some(n.to_string()),
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
                let node = &mut self.tree.nodes[ix as usize];
                node.first = label;
                node.last = label;
                (props, None)
            }
        };
        if let Some(suffix) = children {
            let node = &self.tree.nodes[ix as usize];
            let mut chain = Chain {
                first: node.first,
                last: node.last,
            };
            self.tree.append(&mut chain, suffix);
            let node = &mut self.tree.nodes[ix as usize];
            node.first = chain.first;
            node.last = chain.last;
        }
        let node = &mut self.tree.nodes[ix as usize];
        node.kind = HKind::Element {
            tag: "span",
            props,
            meta: None,
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
        let pt = match edge {
            Edge::None => return,
            Edge::Byte(byte) => self.points.pt(byte),
            Edge::Pt(pt) => pt,
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
        let mut cursor = self.tree.nodes[ix as usize].first;
        let mut first = true;
        while cursor != NIL {
            if !first {
                self.out.push(',');
            }
            first = false;
            self.node(cursor);
            cursor = self.tree.nodes[cursor as usize].next;
        }
        self.out.push(']');
    }

    fn value(&mut self, value: &Val<'_>) {
        match value {
            Val::Str(s) => json_string(&mut self.out, s),
            Val::Bool(b) => self.out.push_str(if *b { "true" } else { "false" }),
            Val::Num(n) => {
                let _ = write!(self.out, "{n}");
            }
            Val::Class(name) => {
                self.out.push_str("[\"");
                self.out.push_str(name);
                self.out.push_str("\"]");
            }
            Val::Digits(n) => {
                self.out.push('"');
                json_u32(&mut self.out, *n as u32);
                self.out.push('"');
            }
            Val::List(list) => {
                self.out.push('[');
                for (i, item) in list.iter().enumerate() {
                    if i > 0 {
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
            HKind::Element { tag, props, meta } => {
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
                if let Some(meta) = meta {
                    self.out.push_str(",\"data\":{\"meta\":");
                    json_string(&mut self.out, meta);
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

/// How far [`render`] took the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    pub fn name(self) -> &'static str {
        match self {
            Kind::Complete => "complete",
            Kind::Raw => "raw",
            Kind::Math => "math",
        }
    }
}

/// Markdown → Streamdown's HAST, as JSON.
pub fn render(source: &str, harden: &Harden) -> Result<(Kind, String), Error> {
    let source = mdast::without_nul(source);
    let (kind, mdast, tree, root) = build(&source, harden)?;
    let mut json = Json {
        tree: &tree,
        points: &mdast.points,
        out: String::with_capacity(source.len() * 6 + 64),
    };
    json.node(root);
    Ok((kind, json.out))
}

/// `{"kind":"complete"|"raw"|"math","tree":HAST}`.
pub fn render_json(source: &str, harden: &Harden) -> Result<String, Error> {
    let (kind, tree) = render(source, harden)?;
    Ok(format!("{{\"kind\":\"{}\",\"tree\":{tree}}}", kind.name()))
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
        self.units += if value.is_ascii() {
            value.len() as u32
        } else {
            value.encode_utf16().count() as u32
        };
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
        let pt = match edge {
            Edge::None => return,
            Edge::Byte(byte) => self.points.pt(byte),
            Edge::Pt(pt) => pt,
        };
        self.ops.extend([pt.line, pt.column, pt.offset]);
    }

    fn position(&mut self, start: Edge, end: Edge) {
        let flags = u32::from(start != Edge::None) | (u32::from(end != Edge::None) << 1);
        self.ops.push(flags);
        self.point(start);
        self.point(end);
    }

    fn children(&mut self, ix: Ix) {
        let mut count = 0;
        let mut cursor = self.tree.nodes[ix as usize].first;
        while cursor != NIL {
            count += 1;
            cursor = self.tree.nodes[cursor as usize].next;
        }
        self.ops.push(count);
        let mut cursor = self.tree.nodes[ix as usize].first;
        while cursor != NIL {
            self.node(cursor);
            cursor = self.tree.nodes[cursor as usize].next;
        }
    }

    fn value(&mut self, value: &Val<'_>) {
        match value {
            Val::Str(s) => {
                self.ops.push(op::STRING);
                self.string(s);
            }
            Val::Bool(true) => self.ops.push(op::TRUE),
            Val::Bool(false) => self.ops.push(op::FALSE),
            Val::Num(n) => self.ops.extend([op::NUMBER, *n as u32]),
            Val::Digits(n) => {
                self.ops.push(op::STRING);
                let digits = n.to_string();
                self.string(&digits);
            }
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
            HKind::Element { tag, props, meta } => {
                self.ops.push(op::ELEMENT);
                self.name(tag);
                self.ops.push(props.len() as u32);
                for (key, value) in props {
                    self.name(key);
                    self.value(value);
                }
                match meta {
                    Some(meta) => {
                        self.ops.push(1);
                        self.string(meta);
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

fn build<'s>(
    source: &'s str,
    harden: &Harden,
) -> Result<(Kind, mdast::MTree<'s>, HTree<'s>, Ix), Error> {
    let mdast = mdast::parse_arena(source)?;
    let Converted {
        mut tree,
        root,
        raw_boundary,
        math,
    } = to_hast(&mdast);
    if raw_boundary {
        return Ok((Kind::Raw, mdast, tree, root));
    }
    if tree.nodes[root as usize].first == NIL {
        // parse5 locates a fragment root only through its child tokens.
        tree.nodes[root as usize].start = Edge::None;
        tree.nodes[root as usize].end = Edge::None;
    }
    tree.quirks = true;
    Finisher {
        tree: &mut tree,
        harden: (!math).then_some(harden),
        ancestors: Vec::new(),
    }
    .children(root);
    Ok((
        if math { Kind::Math } else { Kind::Complete },
        mdast,
        tree,
        root,
    ))
}

/// The HAST as opcodes over `source` with NUL already replaced (see
/// [`mdast::without_nul`]); offsets index that string in UTF-16 units.
pub fn render_ops(source: &str, harden: &Harden) -> Result<Encoded, Error> {
    let (kind, mdast, tree, root) = build(source, harden)?;
    let mut encoder = Encoder {
        tree: &tree,
        points: &mdast.points,
        source,
        ops: Vec::with_capacity(tree.nodes.len() * 8),
        strings: String::new(),
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
