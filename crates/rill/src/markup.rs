//! Streamdown 2.7.1's DOM for a finished HAST tree, as the `<Streamdown>` drop-in
//! renders it: what Streamdown's built-in components (lib/components.tsx) make of each
//! element after hast-util-to-jsx-runtime, as HTML, and the few places React has to
//! take over.
//!
//! The output is a program of segments for one block, read by js/src/dropin.tsx:
//!
//! - a static element: tag, React props, and its children as HTML (React-escaped,
//!   attributes in React's order), for `dangerouslySetInnerHTML`;
//! - an element React creates because a descendant needs React, with its children as
//!   segments;
//! - a code block, a table, a link button around React content, and math (KaTeX, which
//!   JavaScript renders) as their own segments;
//! - a HAST subtree for hast-util-to-jsx-runtime with Streamdown's components (images,
//!   and anything under an element the page gives its own component).
//!
//! Anything this does not reproduce exactly makes [`encode`] fail, and the block
//! renders with Streamdown's own `Block`.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::animate::{self, Animate, Animated};
use crate::hast::{self, Encoder, Kind, NodeKind, Options, Props, Tree, Value};
use crate::mdast::{Error, NodeId};

/// The `<Streamdown>` props the markup depends on.
#[derive(Clone, Debug, Default)]
pub struct MarkupOptions {
    /// `prefix`: Tailwind's prefix, put before every class Streamdown's `cn` returns.
    pub prefix: String,
    /// `linkSafety.enabled`: links render as buttons that open a confirmation.
    pub link_safety: bool,
    /// React's server rendering (`renderToString`, and hydrating it): styles as React
    /// writes them and `<!-- -->` between adjacent text. Otherwise the markup matches
    /// what React builds in the browser (styles as the CSSOM serializes them).
    pub ssr: bool,
    /// Tag names the page gives its own component: elements holding one render
    /// through hast-util-to-jsx-runtime.
    pub overrides: Vec<String>,
    /// JavaScript renders math with KaTeX: without it, a tree with math is
    /// [`Unsupported`].
    pub katex: bool,
    /// The block ends in a code fence still streaming, whose state Streamdown's
    /// components read from a context only its own `Block` provides: a tree with an
    /// element the page gives its own component is [`Unsupported`].
    pub incomplete: bool,
    /// While an answer streams: the last element of the block, its last element, and so
    /// on down are React elements (with their other children static), so an update that
    /// grows the end changes only text nodes there, as React does, instead of replacing
    /// static HTML.
    pub spine: bool,
    /// `animated` while the answer streams: the animate plugin's pass on the tree first.
    pub animate: Option<Animate>,
    /// Every element a React element (as while animated): a block that was animated
    /// settles as React reconciles it, keeping attributes React leaves behind (an empty
    /// `style`).
    pub deep: bool,
}

/// The tree holds something only Streamdown's own `Block` renders exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported;

/// Segment opcodes. Element segments start with the HAST tag name, which keys them as
/// hast-util-to-jsx-runtime does (`tag-n`, counting each tag among its siblings), so React
/// keeps and remounts their state where it would in Streamdown.
pub mod seg {
    pub const TEXT: u32 = 0;
    pub const STATIC: u32 = 1;
    pub const ELEMENT: u32 = 2;
    pub const CODE: u32 = 3;
    pub const TABLE: u32 = 4;
    pub const HAST: u32 = 5;
    pub const MATH: u32 = 6;
    pub const LINK: u32 = 7;
    /// Prop values.
    pub const STRING: u32 = 0;
    pub const TRUE: u32 = 1;
    pub const FALSE: u32 = 2;
    pub const NUMBER: u32 = 3;
    /// `style={{textAlign}}` from a table cell's `align`.
    pub const ALIGN: u32 = 4;
    /// `style={{"--name": value, ...}}` from `--name:value;...`.
    pub const STYLE: u32 = 5;
    /// Static HTML parts.
    pub const HTML: u32 = 0;
    pub const KATEX: u32 = 1;
}

/// A React prop value, as hast-util-to-jsx-runtime passes it.
#[derive(Clone, Debug)]
enum Prop<'a> {
    Str(Cow<'a, str>),
    Bool(bool),
    Num(i64),
    /// `style: {textAlign}`.
    Align(Cow<'a, str>),
    /// A style of custom properties (`--name:value;...`, as the animate plugin writes
    /// them), which hast-util-to-jsx-runtime hands React as an object.
    Style(Cow<'a, str>),
}

type ReactProps<'a> = Vec<(&'static str, Prop<'a>)>;

/// Elements Streamdown's components render with a class and `data-streamdown`: the tag
/// they render, the class, the `data-streamdown` value.
fn classed(tag: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match tag {
        "ol" => (
            "ol",
            "list-inside list-decimal whitespace-normal [li_&]:pl-6",
            "ordered-list",
        ),
        "ul" => (
            "ul",
            "list-inside list-disc whitespace-normal [li_&]:pl-6",
            "unordered-list",
        ),
        "li" => ("li", "py-1 [&>p]:inline", "list-item"),
        "hr" => ("hr", "my-6 border-border", "horizontal-rule"),
        "strong" => ("span", "font-semibold", "strong"),
        "h1" => ("h1", "mt-6 mb-2 font-semibold text-3xl", "heading-1"),
        "h2" => ("h2", "mt-6 mb-2 font-semibold text-2xl", "heading-2"),
        "h3" => ("h3", "mt-6 mb-2 font-semibold text-xl", "heading-3"),
        "h4" => ("h4", "mt-6 mb-2 font-semibold text-lg", "heading-4"),
        "h5" => ("h5", "mt-6 mb-2 font-semibold text-base", "heading-5"),
        "h6" => ("h6", "mt-6 mb-2 font-semibold text-sm", "heading-6"),
        "thead" => ("thead", "bg-muted/80", "table-header"),
        "tbody" => ("tbody", "divide-y divide-border", "table-body"),
        "tr" => ("tr", "border-border", "table-row"),
        "th" => (
            "th",
            "whitespace-nowrap px-4 py-2 text-left font-semibold text-sm",
            "table-header-cell",
        ),
        "td" => ("td", "px-4 py-2 text-sm", "table-cell"),
        "blockquote" => (
            "blockquote",
            "my-4 border-muted-foreground/30 border-l-4 pl-4 text-muted-foreground italic",
            "blockquote",
        ),
        "sup" => ("sup", "text-sm", "superscript"),
        "sub" => ("sub", "text-sm", "subscript"),
        "code" => (
            "code",
            "rounded bg-muted px-1.5 py-0.5 font-mono text-sm",
            "inline-code",
        ),
        _ => return None,
    })
}

/// A built-in class as it appears in an attribute (React-escaped).
fn escaped_class(base: &'static str) -> &'static str {
    match base {
        "list-inside list-decimal whitespace-normal [li_&]:pl-6" => {
            "list-inside list-decimal whitespace-normal [li_&amp;]:pl-6"
        }
        "list-inside list-disc whitespace-normal [li_&]:pl-6" => {
            "list-inside list-disc whitespace-normal [li_&amp;]:pl-6"
        }
        "py-1 [&>p]:inline" => "py-1 [&amp;&gt;p]:inline",
        _ => base,
    }
}

const LINK_BUTTON_CLASS: &str =
    "wrap-anywhere appearance-none text-left font-medium text-primary underline";
const LINK_CLASS: &str = "wrap-anywhere font-medium text-primary underline";
/// remend's sentinel for a link whose URL has not arrived.
const INCOMPLETE_LINK: &str = "streamdown:incomplete-link";

/// `data-` and lowercase letters, digits and dashes: a name hast-util-to-jsx-runtime
/// passes on as it is.
fn data_attribute(key: &str) -> bool {
    key.strip_prefix("data-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    })
}

/// The elements KaTeX's output is made of (with `trust` off, as rehype-katex runs it: no
/// links or images).
fn katex_tag(tag: &str) -> bool {
    matches!(
        tag,
        "span"
            | "svg"
            | "path"
            | "line"
            | "math"
            | "semantics"
            | "annotation"
            | "mrow"
            | "mi"
            | "mn"
            | "mo"
            | "ms"
            | "mtext"
            | "mspace"
            | "msup"
            | "msub"
            | "msubsup"
            | "mfrac"
            | "msqrt"
            | "mroot"
            | "mover"
            | "munder"
            | "munderover"
            | "mtable"
            | "mtr"
            | "mtd"
            | "mpadded"
            | "mphantom"
            | "mstyle"
            | "menclose"
    )
}

fn void(tag: &str) -> bool {
    matches!(tag, "br" | "hr" | "img" | "input")
}

/// hast-util-to-jsx-runtime drops whitespace-only text in these.
fn table_part(tag: &str) -> bool {
    matches!(tag, "table" | "thead" | "tbody" | "tfoot" | "tr")
}

/// hast-util-whitespace: only HTML whitespace.
fn html_whitespace(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c'))
}

/// Whether JavaScript's `trim()` leaves nothing.
fn js_blank(value: &str) -> bool {
    value.chars().all(|c| {
        matches!(
            c,
            '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
        )
    })
}

/// React's attribute and text escaping (`escapeTextForBrowser`). Outside `ssr`, the HTML
/// is parsed in the browser, which reads a carriage return as a line feed: it is written as
/// a reference, so the DOM keeps it as React's text and attributes do.
fn escape(out: &mut String, value: &str, ssr: bool) {
    let mut run = 0;
    for (index, byte) in value.bytes().enumerate() {
        let entity = match byte {
            b'"' => "&quot;",
            b'&' => "&amp;",
            b'\'' => "&#x27;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'\r' if !ssr => "&#13;",
            _ => continue,
        };
        out.push_str(&value[run..index]);
        out.push_str(entity);
        run = index + 1;
    }
    out.push_str(&value[run..]);
}

/// The declarations of a style made only of custom properties with plain values (what
/// the animate plugin writes): `--name:value`, `;`-separated.
fn custom_properties(style: &str) -> bool {
    style.split(';').all(|declaration| {
        declaration.split_once(':').is_some_and(|(name, value)| {
            name.starts_with("--")
                && name[2..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
                && !value.is_empty()
                && !value.starts_with(char::is_whitespace)
                && !value.ends_with(char::is_whitespace)
                && !value.contains(['\'', '"', '\\', '/', '\r', '\n', '!', ':'])
        })
    })
}

/// A style of custom properties as React writes it: on the server `--a:b;--c:d`, in the
/// browser (through the CSSOM) `--a: b; --c: d;`.
fn css_text(style: &str, ssr: bool) -> String {
    if ssr {
        return style.to_owned();
    }
    let mut out = String::with_capacity(style.len() + 16);
    for declaration in style.split(';') {
        let (name, value) = declaration.split_once(':').expect("checked");
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push(';');
    }
    out
}

/// How an element renders.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Need {
    /// Static HTML.
    Html,
    /// Through React: it, or something in it, is interactive.
    React,
    /// Through hast-util-to-jsx-runtime: an element the page gives its own component (or
    /// math whose output holds one), with what it holds. Its ancestors are React elements.
    Jsx,
}

/// Where an element is on the streaming spine ([`MarkupOptions::spine`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spine {
    Off,
    /// A React element, and so is its last element child.
    Last,
    /// A React element, and so is everything inside it.
    Deep,
}

struct Markup<'e, 't, 's, 'o> {
    tree: &'t Tree<'s>,
    options: &'o MarkupOptions,
    encoder: &'e mut Encoder<'t, 's>,
    needs: Vec<Option<Need>>,
    /// Math elements are left for KaTeX (a [`Kind::Math`] tree).
    math: bool,
    /// Footnote lists without the items that hold only a backreference.
    footnote_items: HashMap<NodeId, Vec<NodeId>>,
}

/// What the static HTML writer accumulates for one static element.
#[derive(Default)]
struct Html {
    out: String,
    /// KaTeX placeholders split the HTML: `(html before, math element)`.
    parts: Vec<(String, NodeId)>,
    /// The `href` of each link button, in document order.
    links: Vec<String>,
}

impl<'t, 's> Markup<'_, 't, 's, '_> {
    fn props(&self, ix: NodeId) -> &Props<'s> {
        match &self.tree.nodes[ix as usize].kind {
            NodeKind::Element { props, .. } => props,
            _ => unreachable!("only elements have props"),
        }
    }

    fn tag(&self, ix: NodeId) -> Option<&'static str> {
        self.tree.tag_name(ix)
    }

    fn text(&self, ix: NodeId) -> Option<&'t str> {
        let tree = self.tree;
        match &tree.nodes[ix as usize].kind {
            NodeKind::Text(value) => Some(value),
            _ => None,
        }
    }

    fn overridden(&self, tag: &str) -> bool {
        self.options.overrides.iter().any(|name| name == tag)
    }

    fn class_list(&self, ix: NodeId) -> Vec<&str> {
        match hast::get(self.props(ix), "className") {
            Some(Value::List(list)) => list.iter().map(|item| item.as_ref()).collect(),
            Some(Value::Class(name)) => vec![name],
            Some(Value::Str(name)) => vec![name.as_ref()],
            _ => Vec::new(),
        }
    }

    /// A math element KaTeX replaces: `<code class="language-math">`, or the `<pre>`
    /// around one.
    fn math(&self, ix: NodeId) -> bool {
        if !self.math {
            return false;
        }
        match self.tag(ix) {
            Some("code") => self.class_list(ix).contains(&"language-math"),
            Some("pre") => self
                .tree
                .children(ix)
                .find(|&child| self.tag(child).is_some())
                .is_some_and(|code| {
                    self.tag(code) == Some("code")
                        && self.class_list(code).contains(&"language-math")
                }),
            _ => false,
        }
    }

    /// The `<code>` of a fenced code block: the `<pre>`'s one child, holding at most one
    /// text node (what Streamdown's `pre` and `code` components read).
    fn code_block(&self, pre: NodeId) -> Option<NodeId> {
        let mut children = self.tree.children(pre);
        let code = children.next()?;
        if children.next().is_some() || self.tag(code) != Some("code") {
            return None;
        }
        let mut texts = self.tree.children(code);
        match (texts.next(), texts.next()) {
            (None, _) => Some(code),
            (Some(text), None) if self.text(text).is_some() => Some(code),
            _ => None,
        }
    }

    /// Whether `ix` or an element under it renders through hast-util-to-jsx-runtime.
    fn holds_jsx(&mut self, ix: NodeId) -> bool {
        if self.need(ix) == Need::Jsx {
            return true;
        }
        let children: Vec<NodeId> = self.tree.children(ix).collect();
        children.into_iter().any(|child| self.holds_jsx(child))
    }

    fn need(&mut self, ix: NodeId) -> Need {
        if let Some(need) = self.needs[ix as usize] {
            return need;
        }
        let need = match self.tag(ix) {
            None => Need::Html,
            Some(tag) if self.overridden(tag) => Need::Jsx,
            // KaTeX's output renders through Streamdown's components too.
            Some(_) if self.math(ix) => {
                if self.options.overrides.iter().any(|tag| katex_tag(tag)) {
                    Need::Jsx
                } else {
                    Need::Html
                }
            }
            Some("pre") => match self.code_block(ix) {
                Some(_) if !self.overridden("code") => Need::React,
                _ => Need::Jsx,
            },
            Some("img") => Need::React,
            Some(tag) => {
                // The children as React renders them (a footnote list without its empty
                // items, a list item's lone paragraph unwrapped).
                let children = self.children(ix);
                // Adjacent text (harden's ` [blocked]` after a link's text, the line
                // endings around a dropped footnote) is one text node in parsed HTML, but
                // React makes one per string.
                let adjacent_text = !self.options.ssr
                    && children
                        .windows(2)
                        .any(|pair| self.text(pair[0]).is_some() && self.text(pair[1]).is_some());
                let inner = children
                    .into_iter()
                    .map(|child| self.need(child).min(Need::React))
                    .max()
                    .unwrap_or(Need::Html)
                    .max(if adjacent_text {
                        Need::React
                    } else {
                        Need::Html
                    });
                if tag == "table" {
                    inner.max(Need::React)
                } else {
                    inner
                }
            }
        };
        self.needs[ix as usize] = Some(need);
        need
    }

    // -----------------------------------------------------------------------------
    // Props
    // -----------------------------------------------------------------------------

    /// hast-util-to-jsx-runtime's props for an element, in order.
    fn jsx_props(&self, ix: NodeId) -> Result<ReactProps<'s>, Unsupported> {
        let tag = self.tag(ix).unwrap_or_default();
        let mut out = ReactProps::new();
        let mut align = None;
        for (key, value) in self.props(ix) {
            let name = match *key {
                "className" | "class" => "className",
                "ariaDescribedBy" => "aria-describedby",
                "ariaLabel" => "aria-label",
                "dataFootnoteRef" => "data-footnote-ref",
                "dataFootnoteBackref" => "data-footnote-backref",
                "dataFootnotes" => "data-footnotes",
                "align" if matches!(tag, "td" | "th") => "align",
                "href" | "title" | "alt" | "src" | "id" | "start" | "type" | "checked"
                | "disabled" | "metastring" | "target" | "rel" | "width" | "height" => key,
                "style" => match value {
                    Value::Str(style) if custom_properties(style) => {
                        out.push(("style", Prop::Style(style.clone())));
                        continue;
                    }
                    _ => return Err(Unsupported),
                },
                // A plugin's data attributes, named as in HTML: React's prop name too.
                _ if data_attribute(key) => key,
                _ => return Err(Unsupported),
            };
            let value = match value {
                Value::List(_) if name.starts_with("data-") => return Err(Unsupported),
                Value::Str(value) => Prop::Str(value.clone()),
                Value::Bool(value) => Prop::Bool(*value),
                Value::Num(value) => Prop::Num(*value),
                Value::Class(name) => Prop::Str(Cow::Borrowed(name)),
                Value::List(list) => Prop::Str(Cow::Owned(list.join(" "))),
            };
            if name == "align"
                && let Prop::Str(value) = value
            {
                align = Some(value);
                continue;
            }
            out.push((name, value));
        }
        if let Some(value) = align {
            out.push(("style", Prop::Align(value)));
        }
        Ok(out)
    }

    /// Streamdown's `cn` for a built-in class and the element's own.
    fn cn(&self, base: &str, own: Option<&Prop<'_>>) -> String {
        let mut joined = base.to_owned();
        if let Some(Prop::Str(own)) = own
            && !own.is_empty()
        {
            joined.push(' ');
            joined.push_str(own);
        }
        self.prefixed(&joined)
    }

    /// Streamdown's `prefixClasses`.
    fn prefixed(&self, classes: &str) -> String {
        let prefix = &self.options.prefix;
        if prefix.is_empty() {
            return classes.to_owned();
        }
        let mut out = String::with_capacity(classes.len() * 2);
        for class in classes.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            let own = class
                .strip_prefix(prefix.as_str())
                .is_some_and(|rest| rest.starts_with(':'));
            if !own {
                out.push_str(prefix);
                out.push(':');
            }
            out.push_str(class);
        }
        out
    }

    /// The element Streamdown's component renders for `ix`: its tag and props.
    fn component(&self, ix: NodeId) -> Result<(&'static str, ReactProps<'s>), Unsupported> {
        let tag = self.tag(ix).unwrap_or_default();
        let jsx = self.jsx_props(ix)?;
        let take = |name: &str, props: &ReactProps<'s>| {
            props
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        };
        let rest = |props: ReactProps<'s>, names: &[&str]| -> ReactProps<'s> {
            props
                .into_iter()
                .filter(|(key, _)| !names.contains(key))
                .collect()
        };
        if let Some((element, base, name)) = classed(tag) {
            let class = self.cn(base, take("className", &jsx).as_ref());
            let mut out = vec![
                ("className", Prop::Str(Cow::Owned(class))),
                ("data-streamdown", Prop::Str(Cow::Borrowed(name))),
            ];
            out.extend(rest(jsx, &["className"]));
            return Ok((element, out));
        }
        match tag {
            "section" => {
                let mut out = ReactProps::new();
                if let Some(class) = take("className", &jsx) {
                    out.push(("className", class));
                }
                out.extend(rest(jsx, &["className"]));
                Ok(("section", out))
            }
            "a" => {
                let own = take("className", &jsx);
                let href = match take("href", &jsx) {
                    Some(Prop::Str(href)) => Some(href),
                    None => None,
                    Some(_) => return Err(Unsupported),
                };
                let incomplete = href.as_deref() == Some(INCOMPLETE_LINK);
                let incomplete = Prop::Bool(incomplete);
                match href {
                    Some(href) if self.options.link_safety && !href.is_empty() => Ok((
                        "button",
                        vec![
                            (
                                "className",
                                Prop::Str(Cow::Owned(self.cn(LINK_BUTTON_CLASS, own.as_ref()))),
                            ),
                            ("data-incomplete", incomplete),
                            ("data-streamdown", Prop::Str(Cow::Borrowed("link"))),
                            ("type", Prop::Str(Cow::Borrowed("button"))),
                        ],
                    )),
                    href => {
                        let mut out = vec![
                            (
                                "className",
                                Prop::Str(Cow::Owned(self.cn(LINK_CLASS, own.as_ref()))),
                            ),
                            ("data-incomplete", incomplete),
                            ("data-streamdown", Prop::Str(Cow::Borrowed("link"))),
                        ];
                        if let Some(href) = href {
                            out.push(("href", Prop::Str(href)));
                        }
                        out.push(("rel", Prop::Str(Cow::Borrowed("noreferrer"))));
                        out.push(("target", Prop::Str(Cow::Borrowed("_blank"))));
                        for (key, value) in rest(jsx, &["className", "href"]) {
                            match out.iter_mut().find(|(name, _)| *name == key) {
                                Some((_, slot)) => *slot = value,
                                None => out.push((key, value)),
                            }
                        }
                        Ok(("a", out))
                    }
                }
            }
            _ => Ok((tag, jsx)),
        }
    }

    /// The `href` of a link Streamdown renders as a button.
    fn link_button(&self, ix: NodeId) -> Option<String> {
        if !self.options.link_safety || self.tag(ix) != Some("a") {
            return None;
        }
        match hast::get(self.props(ix), "href") {
            Some(Value::Str(href)) if !href.is_empty() => Some(href.to_string()),
            _ => None,
        }
    }

    // -----------------------------------------------------------------------------
    // Children, after Streamdown's components
    // -----------------------------------------------------------------------------

    /// The nodes an element renders as its children.
    fn children(&self, ix: NodeId) -> Vec<NodeId> {
        if let Some(items) = self.footnote_items.get(&ix) {
            return items.clone();
        }
        let tag = self.tag(ix).unwrap_or_default();
        let children: Vec<NodeId> = self.tree.children(ix).collect();
        if table_part(tag) {
            return children
                .into_iter()
                .filter(|&child| !self.text(child).is_some_and(html_whitespace))
                .collect();
        }
        if tag == "li" {
            // MemoLi: a lone paragraph (ignoring "\n" and "") renders without its <p>.
            let mut kept = children
                .iter()
                .copied()
                .filter(|&child| !matches!(self.text(child), Some("\n" | "")));
            if let (Some(only), None) = (kept.next(), kept.next())
                && self.tag(only) == Some("p")
            {
                return self.tree.children(only).collect();
            }
        }
        children
    }

    /// MemoParagraph renders a paragraph holding only an image as just the image.
    fn unwrapped_paragraph(&self, ix: NodeId) -> bool {
        if self.tag(ix) != Some("p") {
            return false;
        }
        let mut kept = self
            .tree
            .children(ix)
            .filter(|&child| self.text(child) != Some(""));
        matches!((kept.next(), kept.next()), (Some(only), None) if self.tag(only) == Some("img"))
    }

    fn backref(&self, ix: NodeId) -> bool {
        self.tag(ix).is_some() && hast::get(self.props(ix), "dataFootnoteBackref").is_some()
    }

    /// MemoSection's `isEmptyFootnote`: an item holding only a backreference.
    fn empty_footnote(&self, item: NodeId) -> bool {
        if self.tag(item).is_none() {
            return false;
        }
        let mut content = false;
        let mut backref = false;
        for child in self.tree.children(item) {
            if let Some(text) = self.text(child) {
                content |= !js_blank(text);
            } else if self.backref(child) {
                backref = true;
            } else {
                for grandchild in self.tree.children(child) {
                    let filled = match self.text(grandchild) {
                        Some(text) => !js_blank(text),
                        None => !self.backref(grandchild),
                    };
                    if filled {
                        content = true;
                        break;
                    }
                }
            }
        }
        backref && !content
    }

    /// Records the footnote lists of a footnotes section without their empty items;
    /// returns the section's children (a list with none left is dropped).
    fn footnotes(&mut self, section: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        for child in self.tree.children(section) {
            if self.tag(child) == Some("ol") {
                let items: Vec<NodeId> = self
                    .tree
                    .children(child)
                    .filter(|&item| !self.empty_footnote(item))
                    .collect();
                if items.is_empty() {
                    continue;
                }
                self.footnote_items.insert(child, items);
            }
            out.push(child);
        }
        out
    }

    fn prepare(&mut self, ix: NodeId) {
        if self.tag(ix) == Some("section") && hast::get(self.props(ix), "dataFootnotes").is_some() {
            let children = self.footnotes(ix);
            self.footnote_items.insert(ix, children);
        }
        let children: Vec<NodeId> = self.tree.children(ix).collect();
        for child in children {
            self.prepare(child);
        }
    }

    // -----------------------------------------------------------------------------
    // HTML
    // -----------------------------------------------------------------------------

    fn attributes(&self, out: &mut String, tag: &str, props: &ReactProps<'_>) {
        let ssr = self.options.ssr;
        let mut write = |name: &str, value: &Prop<'_>| {
            let name = if name == "className" { "class" } else { name };
            let text: Cow<'_, str> = match value {
                Prop::Str(value) => Cow::Borrowed(value.as_ref()),
                Prop::Num(value) => Cow::Owned(value.to_string()),
                Prop::Bool(value) if matches!(name, "checked" | "disabled") => {
                    if !value {
                        return;
                    }
                    Cow::Borrowed("")
                }
                Prop::Bool(value) if name.starts_with("data-") || name.starts_with("aria-") => {
                    Cow::Borrowed(if *value { "true" } else { "false" })
                }
                Prop::Bool(_) => return,
                Prop::Align(value) if ssr => Cow::Owned(format!("text-align:{value}")),
                Prop::Align(value) => Cow::Owned(format!("text-align: {value};")),
                Prop::Style(style) => Cow::Owned(css_text(style, ssr)),
            };
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            escape(out, &text, ssr);
            out.push('"');
        };
        if tag == "input" {
            // React sets an input's `type` and `checked` after its other attributes in
            // the browser, and only `checked` last on the server.
            let late: &[&str] = if ssr {
                &["checked"]
            } else {
                &["type", "checked"]
            };
            for (name, value) in props {
                if !late.contains(name) {
                    write(name, value);
                }
            }
            for late in late {
                if let Some((name, value)) = props.iter().find(|(name, _)| name == late) {
                    write(name, value);
                }
            }
            return;
        }
        for (name, value) in props {
            write(name, value);
        }
    }

    fn html_children(
        &self,
        html: &mut Html,
        children: impl IntoIterator<Item = NodeId>,
    ) -> Result<(), Unsupported> {
        let mut text_before = false;
        for child in children {
            let text = self.text(child);
            if self.options.ssr && text_before && text.is_some_and(|text| !text.is_empty()) {
                html.out.push_str("<!-- -->");
            }
            match text {
                Some(text) => escape(&mut html.out, text, self.options.ssr),
                None => self.html_node(html, child)?,
            }
            if text.is_none_or(|text| !text.is_empty()) {
                text_before = text.is_some();
            }
        }
        Ok(())
    }

    fn html_node(&self, html: &mut Html, ix: NodeId) -> Result<(), Unsupported> {
        if self.math(ix) {
            html.parts.push((std::mem::take(&mut html.out), ix));
            return Ok(());
        }
        if self.unwrapped_paragraph(ix) {
            return Err(Unsupported);
        }
        let tag = self.write_start(html, ix)?;
        if void(tag) {
            html.out.push_str("/>");
            return Ok(());
        }
        html.out.push('>');
        let source = self.tag(ix).unwrap_or_default();
        if table_part(source) || source == "li" || self.footnote_items.contains_key(&ix) {
            self.html_children(html, self.children(ix))?;
        } else {
            self.html_children(html, self.tree.children(ix))?;
        }
        html.out.push_str("</");
        html.out.push_str(tag);
        html.out.push('>');
        Ok(())
    }

    /// Writes `<tag attributes` for the element Streamdown renders for `ix`; returns
    /// the tag.
    fn write_start(&self, html: &mut Html, ix: NodeId) -> Result<&'static str, Unsupported> {
        let source = self.tag(ix).unwrap_or_default();
        // Most elements have no props of their own: their markup is a constant.
        if self.props(ix).is_empty() && self.options.prefix.is_empty() {
            if let Some((tag, base, name)) = classed(source) {
                html.out.push('<');
                html.out.push_str(tag);
                html.out.push_str(" class=\"");
                html.out.push_str(escaped_class(base));
                html.out.push_str("\" data-streamdown=\"");
                html.out.push_str(name);
                html.out.push('"');
                return Ok(tag);
            }
            if !matches!(source, "a" | "section") {
                html.out.push('<');
                html.out.push_str(source);
                return Ok(source);
            }
        }
        let (tag, props) = self.component(ix)?;
        if let Some(href) = self.link_button(ix) {
            html.links.push(href);
        }
        html.out.push('<');
        html.out.push_str(tag);
        self.attributes(&mut html.out, tag, &props);
        Ok(tag)
    }

    // -----------------------------------------------------------------------------
    // Segments
    // -----------------------------------------------------------------------------

    fn write_props(&mut self, props: &ReactProps<'_>) {
        self.encoder.push(props.len() as u32);
        for (name, value) in props {
            self.encoder.name(name);
            match value {
                Prop::Str(value) => {
                    self.encoder.push(seg::STRING);
                    self.encoder.string(value);
                }
                Prop::Bool(true) => self.encoder.push(seg::TRUE),
                Prop::Bool(false) => self.encoder.push(seg::FALSE),
                Prop::Num(value) => {
                    self.encoder.push(seg::NUMBER);
                    self.encoder.push(*value as u32);
                }
                Prop::Align(value) => {
                    self.encoder.push(seg::ALIGN);
                    self.encoder.string(value);
                }
                Prop::Style(style) => {
                    self.encoder.push(seg::STYLE);
                    self.encoder.string(style);
                }
            }
        }
    }

    /// Writes a count, then the segments for `children` of an element on `spine`.
    fn segments(&mut self, children: &[NodeId], spine: Spine) -> Result<(), Unsupported> {
        let slot = self.encoder.slot();
        let last = match spine {
            Spine::Last => children
                .iter()
                .rposition(|&child| self.tag(child).is_some()),
            _ => None,
        };
        let mut count = 0;
        for (index, &child) in children.iter().enumerate() {
            let on = match spine {
                Spine::Deep => Spine::Deep,
                Spine::Last if last == Some(index) => {
                    // What holds text grows anywhere inside: all of it is React.
                    match self.tag(child) {
                        Some(
                            "p" | "li" | "tr" | "td" | "th" | "h1" | "h2" | "h3" | "h4" | "h5"
                            | "h6",
                        ) => Spine::Deep,
                        _ => Spine::Last,
                    }
                }
                _ => Spine::Off,
            };
            count += self.segment(child, on)?;
        }
        self.encoder.patch(slot, count);
        Ok(())
    }

    /// Writes the segments for one node on `spine`; returns how many.
    fn segment(&mut self, ix: NodeId, spine: Spine) -> Result<u32, Unsupported> {
        if let Some(text) = self.text(ix) {
            self.encoder.push(seg::TEXT);
            self.encoder.string(text);
            return Ok(1);
        }
        let Some(tag) = self.tag(ix) else {
            return Err(Unsupported);
        };
        let need = self.need(ix);
        if self.math(ix) && need != Need::Jsx {
            self.encoder.push(seg::MATH);
            self.encoder.node(ix);
            return Ok(1);
        }
        if need == Need::Jsx || tag == "img" {
            self.encoder.push(seg::HAST);
            self.encoder.node(ix);
            return Ok(1);
        }
        if tag == "pre" {
            return self.code(ix);
        }
        if tag == "table" {
            self.encoder.push(seg::TABLE);
            self.encoder.name(tag);
            let props = self.jsx_props(ix)?;
            self.write_props(&props);
            let children = self.children(ix);
            self.segments(&children, spine)?;
            return Ok(1);
        }
        if self.unwrapped_paragraph(ix) {
            let children: Vec<NodeId> = self.tree.children(ix).collect();
            let mut count = 0;
            for child in children {
                count += self.segment(child, spine)?;
            }
            return Ok(count);
        }
        let (element, props) = self.component(ix)?;
        let children = self.children(ix);
        // While streaming, a table's sections are React elements with a static element
        // per row, so a growing table re-renders only its last row.
        let split = self.options.spine && matches!(tag, "thead" | "tbody" | "tfoot");
        if need == Need::React || void(element) || split || spine != Spine::Off {
            if let Some(href) = self.link_button(ix) {
                self.encoder.push(seg::LINK);
                self.encoder.name(tag);
                self.write_props(&props);
                self.encoder.string(&href);
            } else {
                self.encoder.push(seg::ELEMENT);
                self.encoder.name(tag);
                self.encoder.name(element);
                self.write_props(&props);
            }
            self.segments(&children, spine)?;
            return Ok(1);
        }
        let mut html = Html::default();
        if let Some(href) = self.link_button(ix) {
            html.links.push(href);
        }
        self.html_children(&mut html, children.iter().copied())?;
        self.encoder.push(seg::STATIC);
        self.encoder.name(tag);
        self.encoder.name(element);
        self.write_props(&props);
        self.encoder.push(html.parts.len() as u32 * 2 + 1);
        for (before, math) in html.parts {
            self.encoder.push(seg::HTML);
            self.encoder.string(&before);
            self.encoder.push(seg::KATEX);
            self.encoder.node(math);
        }
        self.encoder.push(seg::HTML);
        self.encoder.string(&html.out);
        self.encoder.push(html.links.len() as u32);
        for href in &html.links {
            self.encoder.string(href);
        }
        Ok(1)
    }

    /// A fenced code block: the `<code>`'s class, its text, and its other props, which
    /// Streamdown's code block forwards to its body.
    fn code(&mut self, pre: NodeId) -> Result<u32, Unsupported> {
        let code = self.code_block(pre).ok_or(Unsupported)?;
        let props = self.jsx_props(code)?;
        self.encoder.push(seg::CODE);
        self.encoder.name("pre");
        let text = self
            .tree
            .children(code)
            .next()
            .and_then(|text| self.text(text))
            .unwrap_or_default();
        self.encoder.string(text);
        self.write_props(&props);
        Ok(1)
    }
}

/// One block's segments.
pub struct EncodedMarkup {
    pub ops: Vec<u32>,
    pub strings: String,
    /// The footnote calls after this text (see [`crate::hast::Footnotes`]).
    pub footnote_calls: Vec<(String, usize)>,
    /// With [`MarkupOptions::animate`]: what the pass leaves for the next one.
    pub animated: Option<Animated>,
}

/// Why a block renders with Streamdown's own `Block` instead of rill's markup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fallback {
    /// Raw HTML (or a link in a link) that needs parse5, the HTML5 parser.
    Raw,
    /// Nodes nested deeper than [`crate::mdast::MAX_DEPTH`].
    Deep,
    /// A [`crate::Plugin`] left the block to its JavaScript form.
    Plugin,
    /// Math, with no KaTeX in JavaScript to render it.
    Math,
    /// An element or property the markup does not reproduce exactly.
    Shape,
    /// Math while `animated` streams (the plugin would animate KaTeX's output).
    AnimatedMath,
    /// A code fence still streaming in a block holding an element the page gives its
    /// own component (see [`MarkupOptions::incomplete`]).
    Incomplete,
}

impl Fallback {
    /// The name the JavaScript package reports (`rill::wasm::RenderedHast`'s kind).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Fallback::Raw => "raw-html",
            Fallback::Deep => "too-deep",
            Fallback::Plugin => "plugin-declined",
            Fallback::Math => "math-without-katex",
            Fallback::Shape => "unsupported-shape",
            Fallback::AnimatedMath => "animated-math",
            Fallback::Incomplete => "incomplete-fence-components",
        }
    }
}

/// Markdown → the segments of Streamdown's DOM for it, or `None` where only
/// Streamdown's `Block` renders it exactly (see [`markup`] for why). `source` must be
/// NUL-free (see [`crate::mdast::without_nul`]); offsets index it in UTF-16 units.
pub fn render_markup(
    source: &str,
    options: &Options,
    markup_options: &MarkupOptions,
) -> Result<Option<EncodedMarkup>, Error> {
    match markup(source, options, markup_options) {
        Ok(encoded) => Ok(Some(encoded)),
        Err(Ok(_)) => Ok(None),
        Err(Err(error)) => Err(error),
    }
}

/// [`render_markup`], with the reason a block is left to Streamdown.
pub fn markup(
    source: &str,
    options: &Options,
    markup: &MarkupOptions,
) -> Result<EncodedMarkup, Result<Fallback, Error>> {
    let mut built = match hast::build(source, options) {
        Err(Error::Unsupported) => return Err(Ok(Fallback::Plugin)),
        Err(Error::TooDeep) => return Err(Ok(Fallback::Deep)),
        built => built.map_err(Err)?,
    };
    if built.kind == Kind::Raw {
        return Err(Ok(Fallback::Raw));
    }
    if built.kind == Kind::Math && !markup.katex {
        return Err(Ok(Fallback::Math));
    }
    let animated = match &markup.animate {
        Some(_) if built.kind == Kind::Math => return Err(Ok(Fallback::AnimatedMath)),
        Some(options) => Some(animate::animate(&mut built.tree, built.root, options)),
        None => None,
    };
    let math = built.kind == Kind::Math;
    let mut encoder = Encoder::new(&built.tree, &built.points, source);
    encode(&mut encoder, &built.tree, built.root, markup, math).map_err(Ok)?;
    let (ops, strings) = encoder.finish();
    Ok(EncodedMarkup {
        ops,
        strings,
        footnote_calls: std::mem::take(&mut built.footnote_calls),
        animated,
    })
}

/// Writes the segments for `root`'s children into `encoder`.
fn encode<'t, 's>(
    encoder: &mut Encoder<'t, 's>,
    tree: &'t Tree<'s>,
    root: NodeId,
    options: &MarkupOptions,
    math: bool,
) -> Result<(), Fallback> {
    let mut markup = Markup {
        tree,
        options,
        encoder,
        needs: vec![None; tree.nodes.len()],
        math,
        footnote_items: HashMap::new(),
    };
    markup.prepare(root);
    if options.incomplete
        && tree
            .children(root)
            .any(|ix| markup.need(ix) >= Need::React && markup.holds_jsx(ix))
    {
        return Err(Fallback::Incomplete);
    }
    let children: Vec<NodeId> = tree.children(root).collect();
    // Animated, every element is a React element: Streamdown's words come and go as
    // React reconciles them (an element keeps its place, and the attributes it had), so
    // only React elements keyed as hast-util-to-jsx-runtime keys them end the same.
    markup
        .segments(
            &children,
            if options.animate.is_some() || options.deep {
                Spine::Deep
            } else if options.spine {
                Spine::Last
            } else {
                Spine::Off
            },
        )
        .map_err(|Unsupported| Fallback::Shape)
}
