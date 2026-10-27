//! MDAST: rill-cmark's events become the tree remark builds (remark-parse with remark-gfm,
//! plus remark-math and remark-cjk-friendly as [`Syntax`] selects), in an arena. Nodes link by index, text borrows the
//! source where it can, and positions stay byte offsets until serialization, where they
//! become JavaScript's UTF-16 units.

use std::borrow::Cow;

pub use rill_cmark::Syntax;
use rill_cmark::{Alignment, CodeBlockKind, CowStr, Event, LinkType, Parser, Tag, TagEnd};

use crate::autolink::{self, Finder, Piece};
use crate::json::{json_string, json_u32};
use crate::text::{push_cow, utf16_units};

/// mdast `referenceType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    Shortcut,
    Collapsed,
    Full,
}

/// One table column's mdast `align`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignKind {
    Left,
    Right,
    Center,
    None,
}

impl From<Alignment> for AlignKind {
    fn from(alignment: Alignment) -> Self {
        match alignment {
            Alignment::None => AlignKind::None,
            Alignment::Left => AlignKind::Left,
            Alignment::Center => AlignKind::Center,
            Alignment::Right => AlignKind::Right,
        }
    }
}

/// An event stream the builder cannot turn into MDAST. Every UTF-8 string is valid
/// Markdown, so these mean a bug in the vendored parser, never bad input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// An end event with no open node, or a mismatched end.
    Unbalanced,
    /// A child for a node that cannot have children.
    LeafParent,
    /// NUL must be replaced (see [`without_nul`]) before parsing.
    Nul,
    /// Nodes nested deeper than [`MAX_DEPTH`]: the stages after parsing recurse once
    /// per level, and WebAssembly's stack is small (and a trap leaves the instance
    /// unusable), so such a document is left to JavaScript.
    TooDeep,
    /// A [`crate::Plugin`] cannot render this document exactly; JavaScript renders it.
    Unsupported,
}

/// How deep [`parse_arena`] nests nodes. Markdown people write nests a dozen levels at
/// most; 3,000 nested quotes overflow the WebAssembly stack.
pub const MAX_DEPTH: usize = 256;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::Unbalanced => "unbalanced parser events",
            Error::LeafParent => "child event inside a leaf node",
            Error::Nul => "source contains NUL",
            Error::TooDeep => "nodes nested too deep",
            Error::Unsupported => "a plugin left the document to JavaScript",
        })
    }
}

impl std::error::Error for Error {}

/// micromark replaces NUL with U+FFFD; both are one UTF-16 unit, so positions agree.
#[must_use]
pub fn without_nul(source: &str) -> Cow<'_, str> {
    if source.contains('\0') {
        Cow::Owned(source.replace('\0', "\u{fffd}"))
    } else {
        Cow::Borrowed(source)
    }
}

// ---------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------

/// A unist point in JavaScript (UTF-16) units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub line: u32,
    pub column: u32,
    pub offset: u32,
}

/// Byte offset → unist point in UTF-16 units, as remark reports them. Offsets must fall on
/// character boundaries.
pub struct Points {
    line_starts: Vec<u32>,
    /// The UTF-16 offset of every byte that starts a character, and of the end. Empty for
    /// ASCII sources, where the two agree.
    utf16: Vec<u32>,
}

impl Points {
    #[must_use]
    pub fn new(source: &str) -> Self {
        let bytes = source.as_bytes();
        let mut line_starts = vec![0];
        for found in memchr::memchr2_iter(b'\n', b'\r', bytes) {
            // A CRLF ends one line, at its LF.
            if !(bytes[found] == b'\r' && bytes.get(found + 1) == Some(&b'\n')) {
                line_starts.push(found as u32 + 1);
            }
        }
        let mut utf16 = Vec::new();
        if !source.is_ascii() {
            utf16.reserve(bytes.len() + 1);
            let mut units = 0;
            for &byte in bytes {
                utf16.push(units);
                units += utf16_units(byte);
            }
            utf16.push(units);
        }
        Points { line_starts, utf16 }
    }

    #[must_use]
    pub fn units(&self, byte: u32) -> u32 {
        self.utf16.get(byte as usize).copied().unwrap_or(byte)
    }

    /// The point one unit past `byte`, a line ending, without starting a new line: where
    /// micromark starts a task item's paragraph after the line ending that follows its
    /// check, a point no source byte has.
    #[must_use]
    pub fn past_line_ending(&self, byte: u32) -> Point {
        let mut point = self.point(byte);
        point.column += 1;
        point.offset += 1;
        point
    }

    #[must_use]
    pub fn point(&self, byte: u32) -> Point {
        let line = self.line_starts.partition_point(|start| *start <= byte);
        let offset = self.units(byte);
        let column = offset - self.units(self.line_starts[line - 1]) + 1;
        Point {
            line: line as u32,
            column,
            offset,
        }
    }
}

/// Where a node is in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    None,
    /// Byte offsets into the parsed source.
    Bytes(u32, u32),
    /// From one unit past the line ending at byte `line_ending`, still on its line (see
    /// [`Points::past_line_ending`]), to byte `end`.
    PastLineEnding {
        line_ending: u32,
        end: u32,
    },
}

// ---------------------------------------------------------------------------
// Arena MDAST
// ---------------------------------------------------------------------------

pub type NodeId = u32;
pub const NIL: NodeId = u32::MAX;

/// A fenced or indented code block's, or a math block's, payload.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeBlock<'s> {
    pub value: Cow<'s, str>,
    /// Always `None` for math.
    pub lang: Option<Box<str>>,
    pub meta: Option<Box<str>>,
    /// Always `true` for math.
    pub fenced: bool,
    /// Indented code on micromark's lazy line (see `CodeBlockKind::LazyIndented`).
    pub lazy: bool,
}

/// mdast's Resource and Alternative mixins: a link's or image's destination.
#[derive(Clone, Debug, PartialEq)]
pub struct Resource<'s> {
    pub url: Cow<'s, str>,
    pub title: Option<Cow<'s, str>>,
    /// An image's alt; empty for a link.
    pub alt: String,
}

/// mdast's Association mixin: a footnote definition's or footnote reference's label.
#[derive(Clone, Debug, PartialEq)]
pub struct Association {
    pub identifier: Box<str>,
    pub label: Box<str>,
}

/// A link reference or image reference.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    pub kind: ReferenceKind,
    pub identifier: Box<str>,
    pub label: Box<str>,
    /// An image reference's alt; empty for a link reference.
    pub alt: String,
}

/// A link or image definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Definition<'s> {
    pub url: Cow<'s, str>,
    pub title: Option<Cow<'s, str>>,
    pub identifier: Box<str>,
    pub label: Box<str>,
}

/// A node's type and fields. Rare, large payloads are boxed so the common nodes (text,
/// paragraphs, table cells, list items) stay small.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind<'s> {
    Root,
    Paragraph,
    Heading(u8),
    ThematicBreak,
    Blockquote,
    List {
        ordered: bool,
        start: Option<u32>,
        spread: bool,
    },
    ListItem {
        spread: bool,
        checked: Option<bool>,
    },
    Html(Cow<'s, str>),
    Code(Box<CodeBlock<'s>>),
    Math(Box<CodeBlock<'s>>),
    Definition(Box<Definition<'s>>),
    Text(Cow<'s, str>),
    Emphasis,
    Strong,
    Delete,
    InlineCode(Cow<'s, str>),
    InlineMath(Cow<'s, str>),
    Break,
    Link(Box<Resource<'s>>),
    LinkReference(Box<Reference>),
    Image(Box<Resource<'s>>),
    ImageReference(Box<Reference>),
    FootnoteDefinition(Box<Association>),
    FootnoteReference(Box<Association>),
    Table(Box<[AlignKind]>),
    TableRow,
    TableCell,
}

impl NodeKind<'_> {
    /// The mdast `type`.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            NodeKind::Root => "root",
            NodeKind::Paragraph => "paragraph",
            NodeKind::Heading(_) => "heading",
            NodeKind::ThematicBreak => "thematicBreak",
            NodeKind::Blockquote => "blockquote",
            NodeKind::List { .. } => "list",
            NodeKind::ListItem { .. } => "listItem",
            NodeKind::Html(_) => "html",
            NodeKind::Code { .. } => "code",
            NodeKind::Math { .. } => "math",
            NodeKind::Definition { .. } => "definition",
            NodeKind::Text(_) => "text",
            NodeKind::Emphasis => "emphasis",
            NodeKind::Strong => "strong",
            NodeKind::Delete => "delete",
            NodeKind::InlineCode(_) => "inlineCode",
            NodeKind::InlineMath(_) => "inlineMath",
            NodeKind::Break => "break",
            NodeKind::Link { .. } => "link",
            NodeKind::LinkReference { .. } => "linkReference",
            NodeKind::Image { .. } => "image",
            NodeKind::ImageReference { .. } => "imageReference",
            NodeKind::FootnoteDefinition { .. } => "footnoteDefinition",
            NodeKind::FootnoteReference { .. } => "footnoteReference",
            NodeKind::Table(_) => "table",
            NodeKind::TableRow => "tableRow",
            NodeKind::TableCell => "tableCell",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node<'s> {
    pub kind: NodeKind<'s>,
    pub first: NodeId,
    pub last: NodeId,
    pub next: NodeId,
    pub at: At,
}

pub struct Tree<'s> {
    pub source: &'s str,
    pub nodes: Vec<Node<'s>>,
    pub points: Points,
}

impl<'s> Tree<'s> {
    pub const ROOT: NodeId = 0;

    /// A new, detached node.
    pub fn push(&mut self, kind: NodeKind<'s>, at: At) -> NodeId {
        let ix = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            kind,
            first: NIL,
            last: NIL,
            next: NIL,
            at,
        });
        ix
    }

    /// Appends detached `child` to `parent`'s children.
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        let last = self.nodes[parent as usize].last;
        if last == NIL {
            self.nodes[parent as usize].first = child;
        } else {
            self.nodes[last as usize].next = child;
        }
        self.nodes[parent as usize].last = child;
    }

    /// Links the chain `first..=last` into `parent`'s children between `previous` (`NIL`
    /// before the first) and `next` (`NIL` after the last).
    pub(crate) fn splice(
        &mut self,
        parent: NodeId,
        previous: NodeId,
        (first, last): (NodeId, NodeId),
        next: NodeId,
    ) {
        if previous == NIL {
            self.nodes[parent as usize].first = first;
        } else {
            self.nodes[previous as usize].next = first;
        }
        self.nodes[last as usize].next = next;
        if next == NIL {
            self.nodes[parent as usize].last = last;
        }
    }

    #[must_use]
    pub fn node(&self, ix: NodeId) -> &Node<'s> {
        &self.nodes[ix as usize]
    }

    #[must_use]
    pub fn children(&self, parent: NodeId) -> Children<'_, 's> {
        Children {
            tree: self,
            next: self.nodes[parent as usize].first,
        }
    }

    /// `ix`'s byte span; `None` for `NIL` or a node without one.
    fn bytes(&self, ix: NodeId) -> Option<(u32, u32)> {
        match self.nodes.get(ix as usize)?.at {
            At::Bytes(start, end) => Some((start, end)),
            _ => None,
        }
    }

    #[must_use]
    pub fn point_range(&self, at: At) -> Option<(Point, Point)> {
        match at {
            At::None => None,
            At::Bytes(start, end) => Some((self.points.point(start), self.points.point(end))),
            At::PastLineEnding { line_ending, end } => Some((
                self.points.past_line_ending(line_ending),
                self.points.point(end),
            )),
        }
    }

    /// The tree as remark's MDAST JSON, positions in UTF-16 units.
    #[must_use]
    pub fn to_json(&self) -> String {
        // The text, plus up to about 150 bytes of syntax and position per node.
        let mut out = String::with_capacity(self.source.len() + self.nodes.len() * 150);
        self.write_json(&mut out, Tree::ROOT);
        out
    }

    fn write_json(&self, out: &mut String, ix: NodeId) {
        let node = self.node(ix);
        let field = |out: &mut String, key: &str| {
            out.push_str(",\"");
            out.push_str(key);
            out.push_str("\":");
        };
        let string = |out: &mut String, key: &str, value: &str| {
            field(out, key);
            json_string(out, value);
        };
        let nullable = |out: &mut String, key: &str, value: Option<&str>| {
            field(out, key);
            match value {
                Some(value) => json_string(out, value),
                None => out.push_str("null"),
            }
        };
        let reference_type = |out: &mut String, kind: ReferenceKind| {
            field(out, "referenceType");
            out.push_str(match kind {
                ReferenceKind::Shortcut => "\"shortcut\"",
                ReferenceKind::Collapsed => "\"collapsed\"",
                ReferenceKind::Full => "\"full\"",
            });
        };
        let flag = |out: &mut String, key: &str, value: bool| {
            field(out, key);
            out.push_str(if value { "true" } else { "false" });
        };
        out.push_str("{\"type\":\"");
        out.push_str(node.kind.type_name());
        out.push('"');
        let mut parent = true;
        match &node.kind {
            NodeKind::Heading(depth) => {
                field(out, "depth");
                json_u32(out, u32::from(*depth));
            }
            NodeKind::List {
                ordered,
                start,
                spread,
            } => {
                flag(out, "ordered", *ordered);
                field(out, "start");
                match start {
                    Some(start) => json_u32(out, *start),
                    None => out.push_str("null"),
                }
                flag(out, "spread", *spread);
            }
            NodeKind::ListItem { spread, checked } => {
                flag(out, "spread", *spread);
                field(out, "checked");
                out.push_str(match checked {
                    Some(true) => "true",
                    Some(false) => "false",
                    None => "null",
                });
            }
            NodeKind::Html(value)
            | NodeKind::Text(value)
            | NodeKind::InlineCode(value)
            | NodeKind::InlineMath(value) => {
                parent = false;
                string(out, "value", value);
            }
            NodeKind::Code(code) | NodeKind::Math(code) => {
                parent = false;
                if matches!(node.kind, NodeKind::Code(_)) {
                    nullable(out, "lang", code.lang.as_deref());
                }
                nullable(out, "meta", code.meta.as_deref());
                string(out, "value", &code.value);
            }
            NodeKind::Definition(definition) => {
                parent = false;
                string(out, "identifier", &definition.identifier);
                string(out, "label", &definition.label);
                nullable(out, "title", definition.title.as_deref());
                string(out, "url", &definition.url);
            }
            NodeKind::Link(resource) | NodeKind::Image(resource) => {
                if matches!(node.kind, NodeKind::Image(_)) {
                    parent = false;
                    string(out, "alt", &resource.alt);
                }
                nullable(out, "title", resource.title.as_deref());
                string(out, "url", &resource.url);
            }
            NodeKind::LinkReference(reference) | NodeKind::ImageReference(reference) => {
                if matches!(node.kind, NodeKind::ImageReference(_)) {
                    parent = false;
                    string(out, "alt", &reference.alt);
                }
                string(out, "identifier", &reference.identifier);
                string(out, "label", &reference.label);
                reference_type(out, reference.kind);
            }
            NodeKind::FootnoteDefinition(association)
            | NodeKind::FootnoteReference(association) => {
                parent = matches!(node.kind, NodeKind::FootnoteDefinition(_));
                string(out, "identifier", &association.identifier);
                string(out, "label", &association.label);
            }
            NodeKind::Table(align) => {
                field(out, "align");
                out.push('[');
                for (index, align) in align.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(match align {
                        AlignKind::Left => "\"left\"",
                        AlignKind::Right => "\"right\"",
                        AlignKind::Center => "\"center\"",
                        AlignKind::None => "null",
                    });
                }
                out.push(']');
            }
            NodeKind::ThematicBreak | NodeKind::Break => parent = false,
            _ => {}
        }
        if parent {
            out.push_str(",\"children\":[");
            for (index, child) in self.children(ix).enumerate() {
                if index > 0 {
                    out.push(',');
                }
                self.write_json(out, child);
            }
            out.push(']');
        }
        if let Some((start, end)) = self.point_range(node.at) {
            out.push_str(",\"position\":{\"start\":");
            write_point(out, start);
            out.push_str(",\"end\":");
            write_point(out, end);
            out.push('}');
        }
        out.push('}');
    }
}

pub struct Children<'t, 's> {
    tree: &'t Tree<'s>,
    next: NodeId,
}

impl Iterator for Children<'_, '_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        (self.next != NIL).then(|| {
            let ix = self.next;
            self.next = self.tree.nodes[ix as usize].next;
            ix
        })
    }
}

/// micromark `normalizeIdentifier` followed by mdast-util-from-markdown's lower-casing.
fn normalize_identifier(label: &str) -> String {
    let mut collapsed = String::with_capacity(label.len());
    let parts = label.split(['\t', '\n', '\r', ' ']);
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(part);
    }
    if collapsed.is_ascii() {
        // Case folding round trips stay ASCII and come back to the ASCII lowercase.
        collapsed.make_ascii_lowercase();
        return collapsed;
    }
    collapsed.to_lowercase().to_uppercase().to_lowercase()
}

/// A footnote's identifier and label.
fn association(label: CowStr<'_>) -> Box<Association> {
    Box::new(Association {
        identifier: normalize_identifier(&label).into(),
        label: label.into_string().into(),
    })
}

/// Where `slice` sits in `source`, if it is a subslice of it.
fn source_offset(source: &str, slice: &str) -> Option<usize> {
    let start = (slice.as_ptr() as usize).checked_sub(source.as_ptr() as usize)?;
    (start + slice.len() <= source.len()).then_some(start)
}

// ---------------------------------------------------------------------------
// Builder: pulldown events → arena
// ---------------------------------------------------------------------------

struct Frame {
    node: NodeId,
    start: usize,
    end: usize,
    /// Spaces pulldown synthesized from a partly consumed tab at an indented code start.
    synthesized: usize,
}

/// A closed node whose end waits on what opens the line that closed it.
struct Exit {
    node: NodeId,
    settle: Settle,
    /// pulldown's end: the start of the line that closed it.
    closing: usize,
    content_end: usize,
}

enum Settle {
    /// A blockquote or list: micromark's container exit. `eof_prefixed`: the closing line
    /// is the last and only continues the containers still open (`flowStart` at EOF).
    Container { eof_prefixed: bool },
    /// An unclosed code or math fence: where `closeFlow` stopped feeding it.
    Fence {
        in_blockquote: bool,
        /// The open containers when it closed (for its end at EOF).
        prefixes: Vec<Prefix>,
    },
    /// A list item (in `list`) whose last child settles first: it ends no earlier than
    /// that child.
    Item { list: NodeId },
}

/// An open container's line prefix: a quote marker, or indentation to an item's content
/// column.
#[derive(Clone, Copy)]
enum Prefix {
    Quote,
    Item(usize),
}

#[derive(Clone, Copy)]
enum Opens {
    Container,
    Item,
    Other,
}

/// Text collected for the next text node, with its byte span.
struct PendingText<'s> {
    value: Cow<'s, str>,
    start: usize,
    end: usize,
}

/// A task item's check (`[x]`) and the paragraph it opens.
#[derive(Clone, Copy)]
struct TaskCheck {
    paragraph: NodeId,
    start: usize,
    end: usize,
}

struct Builder<'s> {
    tree: Tree<'s>,
    syntax: Syntax,
    stack: Vec<Frame>,
    text: Option<PendingText<'s>>,
    /// Plain text collected for the open image's alt; nested images only add text.
    alt: Option<String>,
    nested_images: usize,
    /// The open task item's paragraph with its check's byte span.
    task: Option<TaskCheck>,
    /// Nodes that start past a line ending (see `task_paragraph`), with that line ending.
    starts: Vec<(NodeId, u32)>,
    exits: Vec<Exit>,
    /// Where plain text (image alts included) holds a code run micromark gave up on.
    failed_runs: Vec<u32>,
    /// Each node's previous sibling when the builder appended it (`NIL` for a first
    /// child), so a closing node finds it without walking its parent's children.
    previous: Vec<NodeId>,
}

impl<'s> Builder<'s> {
    /// Appends `ix` to `parent`, noting its previous sibling.
    fn append(&mut self, parent: NodeId, ix: NodeId) {
        let last = self.tree.nodes[parent as usize].last;
        if self.previous.len() <= ix as usize {
            self.previous.resize(ix as usize + 1, NIL);
        }
        self.previous[ix as usize] = last;
        self.tree.append(parent, ix);
    }

    /// Notes the code runs in plain text: micromark tried each as code and gave up.
    fn note_failed_run(&mut self, start: usize, end: usize) {
        let runs =
            code_runs(&self.tree.source[start..end], self.syntax).map(|at| (start + at) as u32);
        self.failed_runs.extend(runs);
    }

    fn parent(&self) -> Result<NodeId, Error> {
        self.stack
            .last()
            .map(|frame| frame.node)
            .ok_or(Error::Unbalanced)
    }

    fn kind_mut(&mut self) -> Option<&mut NodeKind<'s>> {
        let ix = self.stack.last()?.node;
        Some(&mut self.tree.nodes[ix as usize].kind)
    }

    fn leaf(&mut self, kind: NodeKind<'s>, at: At) -> Result<(), Error> {
        self.flush_text()?;
        let parent = self.parent()?;
        let ix = self.tree.push(kind, at);
        self.append(parent, ix);
        Ok(())
    }

    fn flush_text(&mut self) -> Result<(), Error> {
        if let Some(PendingText { value, start, end }) = self.text.take() {
            let parent = self.parent()?;
            let ix = self
                .tree
                .push(NodeKind::Text(value), At::Bytes(start as u32, end as u32));
            self.append(parent, ix);
        }
        Ok(())
    }

    fn push_text(&mut self, value: Cow<'s, str>, start: usize, end: usize) {
        let source = self.tree.source;
        match &mut self.text {
            Some(PendingText {
                value: text,
                end: text_end,
                ..
            }) => {
                // Contiguous source slices stay one borrow instead of a copy.
                match (&*text, &value) {
                    (Cow::Borrowed(left), Cow::Borrowed(right)) => {
                        match (source_offset(source, left), source_offset(source, right)) {
                            (Some(at), Some(right_at)) if at + left.len() == right_at => {
                                *text = Cow::Borrowed(&source[at..right_at + right.len()]);
                            }
                            _ => push_cow(text, &value),
                        }
                    }
                    _ => push_cow(text, &value),
                }
                *text_end = end;
            }
            None => self.text = Some(PendingText { value, start, end }),
        }
    }

    fn open(&mut self, kind: NodeKind<'s>, start: usize, end: usize) -> Result<(), Error> {
        self.flush_text()?;
        let parent = self.parent()?;
        if self.stack.len() > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        let ix = self.tree.push(kind, At::None);
        self.append(parent, ix);
        self.stack.push(Frame {
            node: ix,
            start,
            end,
            synthesized: 0,
        });
        Ok(())
    }

    /// Blockquotes enclosing the node being closed (already popped off the stack).
    fn quote_depth(&self) -> usize {
        self.stack
            .iter()
            .filter(|frame| {
                matches!(
                    self.tree.nodes[frame.node as usize].kind,
                    NodeKind::Blockquote
                )
            })
            .count()
    }

    /// Whether a container encloses the node being closed (already popped off the stack).
    fn in_container(&self) -> bool {
        self.stack.iter().any(|frame| {
            matches!(
                self.tree.nodes[frame.node as usize].kind,
                NodeKind::Blockquote
                    | NodeKind::ListItem { .. }
                    | NodeKind::FootnoteDefinition { .. }
            )
        })
    }

    fn close(&mut self) -> Result<(), Error> {
        self.flush_text()?;
        if self.stack.len() < 2 {
            return Err(Error::Unbalanced);
        }
        let Frame {
            node: ix,
            mut start,
            mut end,
            synthesized,
        } = self.stack.pop().ok_or(Error::Unbalanced)?;
        // Only concrete blocks look at their containers, only lists and items at spread.
        let (concrete, listing) = match self.tree.nodes[ix as usize].kind {
            NodeKind::Code { .. } | NodeKind::Math { .. } | NodeKind::Html(_) => (true, false),
            NodeKind::List { .. } | NodeKind::ListItem { .. } => (false, true),
            _ => (false, false),
        };
        let in_container = concrete && self.in_container();
        let quote_depth = if concrete { self.quote_depth() } else { 0 };
        let in_blockquote = quote_depth > 0;
        let source = self.tree.source;
        let bytes = source.as_bytes();
        let first_span = self.tree.bytes(self.tree.nodes[ix as usize].first);
        let last_span = self.tree.bytes(self.tree.nodes[ix as usize].last);
        let spread = listing && self.spread(ix);
        let pulldown_end = end;
        let pending = !self.exits.is_empty();
        let fence_prefixes = match &self.tree.nodes[ix as usize].kind {
            NodeKind::Code(code) | NodeKind::Math(code) if code.fenced && in_container => {
                self.prefixes()
            }
            _ => Vec::new(),
        };
        let eof_prefixed = matches!(
            self.tree.nodes[ix as usize].kind,
            NodeKind::List { .. } | NodeKind::Blockquote | NodeKind::FootnoteDefinition { .. }
        ) && {
            let (line_start, line_end) = closing_line(source, end);
            line_end == source.len() && self.continued_prefix_end(line_start).0 == source.len()
        };
        let parent = self.stack.last().map_or(NIL, |frame| frame.node);
        let mut exit = None;
        // A container's content end; its own end settles later (see `settle_exits`).
        let mut container_end = None;
        let content_end = |start: usize| {
            last_span.map_or_else(
                || trim_blank_lines(source, start, end),
                |(_, last)| last as usize,
            )
        };
        match &mut self.tree.nodes[ix as usize].kind {
            NodeKind::Code(code) | NodeKind::Math(code) if code.fenced => {
                let defer;
                (end, defer) = fence_end(source, &mut code.value, start, end, in_container);
                if defer {
                    exit = Some(Exit {
                        node: ix,
                        settle: Settle::Fence {
                            in_blockquote,
                            prefixes: fence_prefixes,
                        },
                        closing: pulldown_end,
                        content_end: 0,
                    });
                }
            }
            NodeKind::Code(code) => {
                start = indented_code_start(source, start, synthesized);
                end = trim_blank_lines(source, start, end);
                // ponytail: top level only; container prefixes would need the spine.
                if !in_container && !code.lazy {
                    let (tail, suffix) = indented_tail(source, end);
                    end = tail;
                    if !suffix.is_empty() {
                        push_cow(&mut code.value, &suffix);
                    }
                }
            }
            NodeKind::Html(value) => {
                if !in_container && unterminated_raw_html(source, start, end) {
                    *value = Cow::Borrowed(&source[start..]);
                    end = source.len();
                } else {
                    end = trim_blank_lines(source, start, end);
                    if in_blockquote {
                        // Lines of only quote markers are blank past their prefixes.
                        end = trim_quoted_blank_lines(source, start, end, quote_depth);
                    }
                }
            }
            NodeKind::List {
                spread: list_spread,
                ..
            } => {
                if let Some((first, _)) = first_span {
                    start = first as usize;
                }
                *list_spread = spread;
                container_end = Some(content_end(start));
            }
            NodeKind::Blockquote => container_end = Some(trim_blank_lines(source, start, end)),
            // gfm-footnote definitions are micromark containers too.
            NodeKind::FootnoteDefinition { .. } => container_end = Some(content_end(start)),
            NodeKind::ListItem {
                spread: item_spread,
                ..
            } => {
                end = trim_line_ending(source, start, end);
                // pulldown starts an item behind a partly consumed tab, even at the line
                // ending or quote marker before it; micromark starts at the marker.
                while start < end && matches!(bytes[start], b' ' | b'\t' | b'\n' | b'\r' | b'>') {
                    start += 1;
                }
                if let Some((_, last)) = last_span {
                    end = last as usize;
                } else {
                    // An empty item ends with its marker line.
                    end = source[start..end]
                        .find(['\n', '\r'])
                        .map_or(end, |at| start + at);
                }
                *item_spread = spread;
                if pending {
                    exit = Some(Exit {
                        node: ix,
                        settle: Settle::Item { list: parent },
                        closing: end,
                        content_end: end,
                    });
                }
            }
            NodeKind::TableCell => {
                if start > 0 && bytes[start - 1] == b'|' {
                    start -= 1;
                }
            }
            NodeKind::TableRow => {
                end = trim_line_ending(source, start, end);
                // The last cell runs through the closing pipe to the row end.
                let last = self.tree.nodes[ix as usize].last;
                if last != NIL
                    && let At::Bytes(_, cell_end) = &mut self.tree.nodes[last as usize].at
                {
                    *cell_end = (*cell_end).max(end as u32);
                }
            }
            NodeKind::Heading(_) => {
                end = trim_line_ending(source, start, end);
                // micromark: a setext heading's content includes the definitions that
                // opened its paragraph, so the heading starts at the first of them.
                if parent != NIL && !atx_heading(&bytes[start..]) {
                    let mut earliest = start;
                    let mut candidate = self.previous_sibling(parent, ix);
                    // Definitions directly before, each ending on the line before.
                    while candidate != NIL {
                        let NodeKind::Definition { .. } = self.tree.nodes[candidate as usize].kind
                        else {
                            break;
                        };
                        let Some((def_start, def_end)) = self.tree.bytes(candidate) else {
                            break;
                        };
                        if line_endings(&bytes[def_end as usize..earliest]) != 1
                            || !source[def_end as usize..earliest].trim().is_empty()
                        {
                            break;
                        }
                        earliest = def_start as usize;
                        candidate = self.previous_sibling(parent, candidate);
                    }
                    start = earliest;
                }
            }
            NodeKind::Paragraph | NodeKind::Table(_) | NodeKind::ThematicBreak => {
                end = trim_line_ending(source, start, end);
            }
            _ => {}
        }
        if let Some(content_end) = container_end {
            exit = Some(Exit {
                node: ix,
                settle: Settle::Container { eof_prefixed },
                closing: end,
                content_end,
            });
            end = content_end;
        }
        self.tree.nodes[ix as usize].at = At::Bytes(start as u32, end as u32);
        if let Some(check) = self.task
            && check.paragraph == ix
        {
            self.task = None;
            self.task_paragraph(ix, check.start, check.end, end);
        }
        if matches!(self.tree.nodes[ix as usize].kind, NodeKind::Paragraph)
            && self.tree.nodes[ix as usize].first == NIL
        {
            // micromark never makes an empty paragraph (pulldown does after definitions).
            self.unlink_last_child(ix);
        }
        self.exits.extend(exit);
        Ok(())
    }

    /// Settles deferred ends once the next block (`next`: its start and what it opens) or
    /// EOF (`None`) is known.
    ///
    /// micromark exits a container at the tokenizer's position: after the prefixes the
    /// closing line continued when that line opens a new container, at EOF when the document
    /// ends on it, and else at the closing line's start. subtokenize then moves each exit
    /// back over line endings and indentation, never past a blockquote prefix. An unclosed
    /// fence ends where `closeFlow` stopped feeding it: at the closing line's start when that
    /// line opens a container or item, else where it already ends.
    fn settle_exits(&mut self, next: Option<(usize, Opens)>) {
        let mut exits = std::mem::take(&mut self.exits);
        for Exit {
            node: ix,
            settle,
            closing,
            content_end,
        } in exits.drain(..)
        {
            match settle {
                Settle::Item { list } => self.settle_item(ix, list, next),
                Settle::Fence {
                    in_blockquote,
                    prefixes,
                } => self.settle_fence(ix, in_blockquote, &prefixes, closing, next),
                Settle::Container { eof_prefixed } => {
                    self.settle_container(ix, eof_prefixed, closing, content_end, next);
                }
            }
        }
        // Settling never defers more, so the emptied buffer goes back for the next exits.
        self.exits = exits;
    }

    /// A list item ends no earlier than its last child.
    fn settle_item(&mut self, ix: NodeId, list: NodeId, next: Option<(usize, Opens)>) {
        let source = self.tree.source;
        // Only a sibling's prefix: an outer list's closes this list first.
        let sibling = self.stack.last().is_some_and(|frame| frame.node == list);
        if let Some((_, last_end)) = self.tree.bytes(self.tree.nodes[ix as usize].last)
            && let At::Bytes(_, item_end) = &mut self.tree.nodes[ix as usize].at
        {
            // prepareList: with no line ending before the next item's prefix, the
            // item ends where that prefix does.
            *item_end =
                match next {
                    Some((next_start, Opens::Item))
                        if sibling
                            && source.get(last_end as usize..next_start).is_some_and(
                                |between| between.bytes().all(|b| matches!(b, b' ' | b'\t' | b'>')),
                            ) =>
                    {
                        list_item_prefix_end(source, next_start) as u32
                    }
                    _ => (*item_end).max(last_end),
                };
        }
    }

    /// An unclosed fence in containers, closed by the line at `closing`.
    fn settle_fence(
        &mut self,
        ix: NodeId,
        in_blockquote: bool,
        prefixes: &[Prefix],
        closing: usize,
        next: Option<(usize, Opens)>,
    ) {
        let source = self.tree.source;
        if next.is_none()
            && let At::Bytes(fence_start, _) = self.tree.nodes[ix as usize].at
        {
            let (end, eof_value) = fence_at_eof(source, prefixes, fence_start as usize);
            let node = &mut self.tree.nodes[ix as usize];
            node.at = At::Bytes(fence_start, end as u32);
            if let NodeKind::Code(code) | NodeKind::Math(code) = &mut node.kind {
                code.value = Cow::Owned(eof_value);
            }
            return;
        }
        let (closing_line, line_end) = closing_line(source, closing);
        // A line ending joins the code only before a non-lazy line, so a lazy closing line
        // leaves the one before it. A blockquote does not continue over a blank line
        // without `>`, so such lines close it earlier.
        let mut line_start = closing_line;
        while in_blockquote && line_start > 0 {
            let previous = start_of_line(source, line_start - 1);
            let line = source[previous..line_start].trim_end_matches(['\n', '\r']);
            if previous == 0 || !line.trim_matches([' ', '\t']).is_empty() {
                break;
            }
            line_start = previous;
        }
        let opens_container = line_start == closing_line
            && matches!(next, Some((next_start, Opens::Container | Opens::Item))
                if (closing_line..=line_end).contains(&next_start));
        let end = if opens_container || line_start == 0 {
            line_start
        } else if source[..line_start].ends_with("\r\n") {
            line_start - 2
        } else {
            line_start - 1
        };
        if let At::Bytes(_, node_end) = &mut self.tree.nodes[ix as usize].at {
            *node_end = (*node_end).max(end as u32);
        }
        // `onexitcodefenced` strips one final line ending from data that, closed lazily,
        // already stops before the last one: a blank last line goes.
        if !opens_container
            && let NodeKind::Code(code) | NodeKind::Math(code) =
                &mut self.tree.nodes[ix as usize].kind
        {
            pop_line_ending(&mut code.value);
        }
    }

    /// A blockquote, list or footnote definition closed by the line at `closing`.
    fn settle_container(
        &mut self,
        ix: NodeId,
        eof_prefixed: bool,
        closing: usize,
        content_end: usize,
        next: Option<(usize, Opens)>,
    ) {
        let source = self.tree.source;
        let (line_start, line_end) = closing_line(source, closing);
        let point = match next {
            _ if closing >= source.len() => source.len(),
            Some((next_start, Opens::Container))
                if (line_start..=line_end).contains(&next_start) =>
            {
                self.continued_prefix_end(line_start).0
            }
            // `flowStart` at EOF right after the prefixes; anything more is a lazy line.
            None if eof_prefixed => source.len(),
            _ => line_start,
        };
        // The walk back stops at content, which a child's settled end may extend.
        let content_end = self
            .tree
            .bytes(self.tree.nodes[ix as usize].last)
            .map_or(content_end, |(_, last_end)| {
                content_end.max(last_end as usize)
            });
        let end = container_exit(source, point, content_end).max(content_end);
        if let At::Bytes(_, node_end) = &mut self.tree.nodes[ix as usize].at {
            *node_end = end as u32;
        }
        if let NodeKind::List { spread, .. } = &mut self.tree.nodes[ix as usize].kind {
            // prepareList: line endings between the content and the exit count toward
            // spread, and the last item reaches the exit unless one follows its content.
            if line_endings(&source.as_bytes()[content_end..end]) >= 2 {
                *spread = true;
            }
            self.extend_last_item(ix, end);
        }
    }

    /// The open containers' line prefixes, outermost first.
    fn prefixes(&self) -> Vec<Prefix> {
        let source = self.tree.source;
        self.stack
            .iter()
            .filter_map(|frame| match self.tree.nodes[frame.node as usize].kind {
                NodeKind::Blockquote => Some(Prefix::Quote),
                // ponytail: four columns from the line start, exact at the top level.
                NodeKind::FootnoteDefinition { .. } => Some(Prefix::Item(4)),
                NodeKind::ListItem { .. } => {
                    Some(Prefix::Item(item_content_column(source, frame.start)))
                }
                _ => None,
            })
            .collect()
    }

    /// A label's source (`start..end`, inside its brackets) as micromark slices it: each
    /// line past the first starts after the prefixes of the containers it continues (a
    /// lazy line's failed item indentation stays). `None` unless that drops anything.
    fn label_source(&self, start: usize, end: usize) -> Option<String> {
        let source = self.tree.source;
        let label = &source[start..end];
        if !label.contains(['\n', '\r']) {
            return None;
        }
        let prefixes = self.prefixes();
        let mut out = String::with_capacity(label.len());
        let mut at = start;
        while let Some(eol) = source[at..end].find(['\n', '\r']).map(|found| at + found) {
            let line_start = eol + line_ending_len(source, eol);
            out.push_str(&source[at..line_start]);
            at = (0..=prefixes.len())
                .rev()
                .map(|depth| prefix_end(source, &prefixes[..depth], line_start))
                .find_map(|(prefix_end, continued)| continued.then_some(prefix_end))
                .unwrap_or(line_start)
                .min(end);
        }
        out.push_str(&source[at..end]);
        (out.len() != label.len()).then_some(out)
    }

    /// A reference's identifier and label from its label source (see `label_source`), or
    /// from pulldown's `id` (whitespace already collapsed) when the source is not found.
    fn reference_label(&self, id: &str, range: Option<(usize, usize)>) -> (Box<str>, Box<str>) {
        let source = self.tree.source;
        let raw = range.map(|(start, end)| {
            self.label_source(start, end)
                .map_or(Cow::Borrowed(&source[start..end]), Cow::Owned)
        });
        let raw = raw.as_deref().unwrap_or(id);
        (
            normalize_identifier(raw).into(),
            rill_cmark::decode_string(raw).into(),
        )
    }

    fn continued_prefix_end(&self, line_start: usize) -> (usize, bool) {
        prefix_end(self.tree.source, &self.prefixes(), line_start)
    }

    fn extend_last_item(&mut self, list: NodeId, list_end: usize) {
        let item = self.tree.nodes[list as usize].last;
        if item == NIL {
            return;
        }
        let content_end = self
            .tree
            .bytes(self.tree.nodes[item as usize].last)
            .map(|(_, child_end)| child_end as usize);
        if let (Some(content_end), At::Bytes(_, item_end)) =
            (content_end, &mut self.tree.nodes[item as usize].at)
            && self
                .tree
                .source
                .get(content_end..list_end)
                .is_some_and(|between| !between.contains(['\n', '\r']))
        {
            *item_end = list_end as u32;
        }
    }

    /// mdast-util-gfm-task-list-item `exitParagraphWithTaskListItem`: micromark's paragraph
    /// starts at the check; a text head drops the one whitespace after it and the paragraph
    /// starts there, with `column++` even when that whitespace was a line ending.
    fn task_paragraph(&mut self, ix: NodeId, check_start: usize, check_end: usize, end: usize) {
        let head = self.tree.nodes[ix as usize].first;
        if head == NIL || !matches!(self.tree.nodes[head as usize].kind, NodeKind::Text(_)) {
            self.tree.nodes[ix as usize].at = At::Bytes(check_start as u32, end as u32);
            return;
        }
        let bytes = self.tree.source.as_bytes();
        let eol = skip_blanks(bytes, check_end);
        if !matches!(bytes.get(eol), Some(b'\n' | b'\r')) {
            return;
        }
        if bytes[eol..].starts_with(b"\r\n")
            && let NodeKind::Text(value) = &mut self.tree.nodes[head as usize].kind
        {
            // `slice(1)` drops only the CR.
            value.to_mut().insert(0, '\n');
        }
        self.starts.push((head, eol as u32));
        self.starts.push((ix, eol as u32));
    }

    /// Removes `ix`, the newest child of the current parent frame.
    fn unlink_last_child(&mut self, ix: NodeId) {
        let Some(parent) = self.stack.last().map(|frame| frame.node) else {
            return;
        };
        let previous = self.previous_sibling(parent, ix);
        if previous == NIL {
            self.tree.nodes[parent as usize].first = NIL;
            self.tree.nodes[parent as usize].last = NIL;
        } else {
            self.tree.nodes[previous as usize].next = NIL;
            self.tree.nodes[parent as usize].last = previous;
        }
    }

    fn previous_sibling(&self, parent: NodeId, ix: NodeId) -> NodeId {
        // Noted when appended, while the sibling links are still those it was appended
        // with (no node is inserted before another while building).
        if let Some(&previous) = self.previous.get(ix as usize) {
            let linked = if previous == NIL {
                self.tree.nodes[parent as usize].first == ix
            } else {
                self.tree.nodes[previous as usize].next == ix
            };
            if linked {
                return previous;
            }
        }
        let mut previous = NIL;
        for child in self.tree.children(parent) {
            if child == ix {
                return previous;
            }
            previous = child;
        }
        NIL
    }

    /// Whether a blank line separates any two consecutive children of `ix`.
    fn spread(&self, ix: NodeId) -> bool {
        let mut previous: Option<u32> = None;
        for child in self.tree.children(ix) {
            let span = self.tree.bytes(child);
            if let (Some(end), Some((start, _))) = (previous, span)
                && blank_between(self.tree.source, end as usize, start as usize)
            {
                return true;
            }
            previous = span.map(|(_, end)| end);
        }
        false
    }

    fn event(&mut self, event: Event<'s>, start: usize, end: usize) -> Result<(), Error> {
        let source = self.tree.source;
        if !self.exits.is_empty() && !matches!(event, Event::End(_)) {
            let opens = match event {
                Event::Start(Tag::BlockQuote | Tag::List(_) | Tag::FootnoteDefinition(_)) => {
                    Opens::Container
                }
                Event::Start(Tag::Item) => Opens::Item,
                _ => Opens::Other,
            };
            self.settle_exits(Some((start, opens)));
        }
        // Inside an image only plain text matters: it becomes the alt, and a nested
        // image contributes its own alt text, like mdast-util-to-string.
        if let Some(alt) = self.alt.as_mut() {
            match &event {
                Event::Start(Tag::Image { .. }) => {
                    self.nested_images += 1;
                    return Ok(());
                }
                Event::End(TagEnd::Image) if self.nested_images > 0 => {
                    self.nested_images -= 1;
                    return Ok(());
                }
                Event::End(TagEnd::Image) => {}
                // mdast-util-to-string keeps inline HTML's source in the alt.
                Event::Text(text)
                | Event::Code(text)
                | Event::InlineMath(text)
                | Event::InlineHtml(text) => {
                    alt.push_str(text);
                    if matches!(event, Event::Text(_)) {
                        self.note_failed_run(start, end);
                    }
                    return Ok(());
                }
                Event::SoftBreak | Event::HardBreak => {
                    alt.push('\n');
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
        match event {
            Event::Start(tag) => self.start(tag, start, end),
            Event::End(TagEnd::Image) => {
                let alt = self.alt.take().unwrap_or_default();
                let Frame {
                    node: ix,
                    start,
                    mut end,
                    ..
                } = self.stack.pop().ok_or(Error::Unbalanced)?;
                match &mut self.tree.nodes[ix as usize].kind {
                    NodeKind::Image(image) => image.alt = alt,
                    NodeKind::ImageReference(reference) => {
                        reference.alt = alt;
                        if reference.kind == ReferenceKind::Collapsed
                            && source.get(end..end + 2) == Some("[]")
                        {
                            end += 2;
                        }
                    }
                    _ => return Err(Error::Unbalanced),
                }
                self.tree.nodes[ix as usize].at = At::Bytes(start as u32, end as u32);
                Ok(())
            }
            Event::End(TagEnd::CodeBlock | TagEnd::HtmlBlock | TagEnd::MathBlock) => {
                match self.kind_mut() {
                    Some(NodeKind::Code(code) | NodeKind::Math(code)) => {
                        pop_line_ending(&mut code.value)
                    }
                    Some(NodeKind::Html(value)) => pop_line_ending(value),
                    _ => return Err(Error::Unbalanced),
                }
                self.close()
            }
            Event::End(TagEnd::Link) => {
                let collapsed = matches!(
                    self.kind_mut(),
                    Some(NodeKind::LinkReference(reference)) if reference.kind == ReferenceKind::Collapsed
                );
                if let Some(frame) = self.stack.last_mut()
                    && collapsed
                    && source.get(frame.end..frame.end + 2) == Some("[]")
                {
                    frame.end += 2;
                }
                self.close()
            }
            Event::End(_) => self.close(),
            Event::Text(text) => {
                match self.kind_mut() {
                    Some(NodeKind::Code(code) | NodeKind::Math(code)) => {
                        let synthesized = code.value.is_empty() && start == end;
                        push_cow(&mut code.value, &text);
                        if synthesized && let Some(frame) = self.stack.last_mut() {
                            frame.synthesized = text.len();
                        }
                        return Ok(());
                    }
                    Some(NodeKind::Html(value)) => {
                        // pulldown synthesizes the block's own indentation; MDAST starts there,
                        // but after a tab a container prefix partly consumed.
                        let synthesized = value.is_empty() && start == end;
                        push_cow(value, &text);
                        if synthesized && let Some(frame) = self.stack.last_mut() {
                            let bytes = source.as_bytes();
                            for _ in 0..text.len() {
                                if frame.start > 0 && bytes[frame.start - 1] == b' ' {
                                    frame.start -= 1;
                                }
                            }
                        }
                        return Ok(());
                    }
                    _ => {}
                }
                let bytes = source.as_bytes();
                // The backslash may already belong to the previous node (a literal).
                let previous_end = self
                    .stack
                    .last()
                    .and_then(|frame| self.tree.bytes(self.tree.nodes[frame.node as usize].last))
                    .map_or(0, |(_, end)| end as usize);
                // An odd run of backslashes: the last one escapes this text's first byte.
                let escaped = self.text.is_none()
                    && odd_backslashes(&bytes[previous_end.min(start)..start])
                    && bytes.get(start).is_some_and(u8::is_ascii_punctuation);
                self.note_failed_run(start - usize::from(escaped), end);
                self.push_text(Cow::from(text), start - usize::from(escaped), end);
                Ok(())
            }
            Event::Html(html) => match self.kind_mut() {
                Some(NodeKind::Html(value)) => {
                    push_cow(value, &html);
                    Ok(())
                }
                _ => Err(Error::Unbalanced),
            },
            Event::InlineHtml(html) => {
                self.leaf(NodeKind::Html(Cow::from(html)), bytes(start, end))
            }
            Event::Code(code) => {
                self.leaf(NodeKind::InlineCode(Cow::from(code)), bytes(start, end))
            }
            Event::InlineMath(value) => {
                self.leaf(NodeKind::InlineMath(Cow::from(value)), bytes(start, end))
            }
            Event::FootnoteReference(label) => self.leaf(
                NodeKind::FootnoteReference(association(label)),
                bytes(start, end),
            ),
            Event::SoftBreak => {
                let ending = source[start..end].trim_start_matches([' ', '\t']);
                self.push_text(Cow::Borrowed(ending), start, end);
                Ok(())
            }
            Event::HardBreak => self.leaf(NodeKind::Break, bytes(start, end)),
            Event::Rule => self.leaf(
                NodeKind::ThematicBreak,
                bytes(start, trim_line_ending(source, start, end)),
            ),
            Event::Definition { label, dest, title } => {
                // pulldown's label is the source inside the brackets.
                let range = (source.as_bytes().get(start) == Some(&b'[')
                    && source[start + 1..].starts_with(&*label))
                .then(|| (start + 1, start + 1 + label.len()));
                let (identifier, label) = self.reference_label(&label, range);
                let end = skip_blanks(source.as_bytes(), end);
                self.leaf(
                    NodeKind::Definition(Box::new(Definition {
                        url: Cow::from(dest),
                        title: title.map(Cow::from),
                        identifier,
                        label,
                    })),
                    bytes(start, end),
                )
            }
            Event::TaskListMarker(checked) => {
                // The check belongs to an item whose first child is the open paragraph.
                let task = match self.stack.as_slice() {
                    [.., item, paragraph]
                        if matches!(
                            self.tree.nodes[paragraph.node as usize].kind,
                            NodeKind::Paragraph
                        ) && matches!(
                            self.tree.nodes[item.node as usize].kind,
                            NodeKind::ListItem { .. }
                        ) && self.tree.nodes[item.node as usize].first == paragraph.node =>
                    {
                        Some((item.node, paragraph.node))
                    }
                    _ => None,
                };
                match task {
                    Some((item, paragraph)) => {
                        if let NodeKind::ListItem { checked: slot, .. } =
                            &mut self.tree.nodes[item as usize].kind
                        {
                            *slot = Some(checked);
                        }
                        self.task = Some(TaskCheck {
                            paragraph,
                            start,
                            end,
                        });
                    }
                    None => {
                        // A setext heading swallowed the paragraph: the marker is plain text.
                        let end = skip_blanks(source.as_bytes(), end);
                        self.push_text(Cow::Borrowed(&source[start..end]), start, end);
                        if let Some(frame) = self.stack.last_mut() {
                            frame.start = frame.start.min(start);
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn start(&mut self, tag: Tag<'s>, start: usize, end: usize) -> Result<(), Error> {
        let source = self.tree.source;
        let kind = match tag {
            Tag::Paragraph => NodeKind::Paragraph,
            Tag::Heading(level) => NodeKind::Heading(level as u8),
            Tag::BlockQuote => NodeKind::Blockquote,
            Tag::CodeBlock(kind) => {
                let fenced = kind.is_fenced();
                let (lang, meta) = if fenced {
                    fence_info(source, start)
                } else {
                    (None, None)
                };
                NodeKind::Code(Box::new(CodeBlock {
                    value: Cow::Borrowed(""),
                    lang,
                    meta,
                    fenced,
                    lazy: kind == CodeBlockKind::LazyIndented,
                }))
            }
            Tag::MathBlock(_) => {
                let info = fence_info_raw(source, start);
                NodeKind::Math(Box::new(CodeBlock {
                    value: Cow::Borrowed(""),
                    lang: None,
                    meta: (!info.is_empty()).then(|| rill_cmark::decode_string(info).into()),
                    fenced: true,
                    lazy: false,
                }))
            }
            Tag::HtmlBlock => NodeKind::Html(Cow::Borrowed("")),
            Tag::List(first) => NodeKind::List {
                ordered: first.is_some(),
                start: first.map(|value| value as u32),
                spread: false,
            },
            Tag::Item => NodeKind::ListItem {
                spread: false,
                checked: None,
            },
            Tag::FootnoteDefinition(label) => NodeKind::FootnoteDefinition(association(label)),
            Tag::Table(align) => {
                NodeKind::Table(align.iter().copied().map(AlignKind::from).collect())
            }
            Tag::TableHead | Tag::TableRow => NodeKind::TableRow,
            Tag::TableCell => NodeKind::TableCell,
            Tag::Emphasis => NodeKind::Emphasis,
            Tag::Strong => NodeKind::Strong,
            Tag::Strikethrough => NodeKind::Delete,
            Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            } => match reference_kind(link_type) {
                Some(kind) => {
                    let range = reference_label_range(source, start, end, kind, 1);
                    let (identifier, label) = self.reference_label(&id, range);
                    NodeKind::LinkReference(Box::new(Reference {
                        kind,
                        identifier,
                        label,
                        alt: String::new(),
                    }))
                }
                None => NodeKind::Link(Box::new(Resource {
                    url: if link_type == LinkType::Email {
                        Cow::Owned(format!("mailto:{dest_url}"))
                    } else {
                        Cow::from(dest_url)
                    },
                    title: (!title.is_empty()).then(|| Cow::from(title)),
                    alt: String::new(),
                })),
            },
            Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            } => {
                self.alt = Some(String::new());
                match reference_kind(link_type) {
                    Some(kind) => {
                        let range = reference_label_range(source, start, end, kind, 2);
                        let (identifier, label) = self.reference_label(&id, range);
                        NodeKind::ImageReference(Box::new(Reference {
                            kind,
                            identifier,
                            label,
                            alt: String::new(),
                        }))
                    }
                    None => NodeKind::Image(Box::new(Resource {
                        url: Cow::from(dest_url),
                        title: (!title.is_empty()).then(|| Cow::from(title)),
                        alt: String::new(),
                    })),
                }
            }
        };
        self.open(kind, start, end)
    }
}

fn bytes(start: usize, end: usize) -> At {
    At::Bytes(start as u32, end as u32)
}

/// Where a reference's label lies in its source (`start..end`, without a collapsed `[]`):
/// a full reference's second brackets, otherwise its text's, which open `open` bytes in
/// (`[` or `![`). A label holds no unescaped bracket.
fn reference_label_range(
    source: &str,
    start: usize,
    end: usize,
    kind: ReferenceKind,
    open: usize,
) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let close = end.checked_sub(1).filter(|close| bytes[*close] == b']')?;
    let label_start = match kind {
        ReferenceKind::Full => {
            (start..close)
                .rev()
                .find(|&at| bytes[at] == b'[' && !odd_backslashes(&bytes[..at]))?
                + 1
        }
        _ => start + open,
    };
    (label_start <= close).then_some((label_start, close))
}

fn reference_kind(link_type: LinkType) -> Option<ReferenceKind> {
    match link_type {
        LinkType::Reference => Some(ReferenceKind::Full),
        LinkType::Collapsed => Some(ReferenceKind::Collapsed),
        LinkType::Shortcut => Some(ReferenceKind::Shortcut),
        _ => None,
    }
}

/// Parses `source` (NUL-free; see [`without_nul`]) with `syntax` into an arena MDAST,
/// including mdast-util-gfm-autolink-literal's text transform.
pub fn parse_arena(source: &str, syntax: Syntax) -> Result<Tree<'_>, Error> {
    parse_arena_with(source, syntax, &[])
}

/// [`parse_arena`], with the footnotes `footnotes` labels defined as well (see
/// [`crate::hast::Footnotes::defined`]).
pub fn parse_arena_with<'s>(
    source: &'s str,
    syntax: Syntax,
    footnotes: &[String],
) -> Result<Tree<'s>, Error> {
    if source.contains('\0') {
        return Err(Error::Nul);
    }
    let mut tree = Tree {
        source,
        // Chat Markdown runs 4 to 19 bytes per node, about 8 typically: room for 6.
        nodes: Vec::with_capacity(source.len() / 6 + 16),
        points: Points::new(source),
    };
    tree.push(NodeKind::Root, bytes(0, source.len()));
    let mut builder = Builder {
        tree,
        syntax,
        stack: vec![Frame {
            node: Tree::ROOT,
            start: 0,
            end: source.len(),
            synthesized: 0,
        }],
        text: None,
        alt: None,
        nested_images: 0,
        task: None,
        starts: Vec::new(),
        exits: Vec::new(),
        failed_runs: Vec::new(),
        previous: Vec::new(),
    };
    let events = Parser::with_footnotes(source, syntax, footnotes).into_offset_iter();
    for (event, range) in events {
        builder.event(event, range.start, range.end)?;
    }
    builder.flush_text()?;
    builder.settle_exits(None);
    if builder.stack.len() != 1 {
        return Err(Error::Unbalanced);
    }
    let mut failed_runs = builder.failed_runs;
    let mut tree = builder.tree;
    for (ix, line_ending) in builder.starts {
        if let At::Bytes(_, end) = tree.nodes[ix as usize].at {
            tree.nodes[ix as usize].at = At::PastLineEnding { line_ending, end };
        }
    }
    autolink_transform(&mut tree);
    if !failed_runs.is_empty() {
        // An autolink's text, literal ones included, was never tried as code.
        let autolinks: Vec<(u32, u32)> = (0..tree.nodes.len() as NodeId)
            .filter(|ix| matches!(tree.nodes[*ix as usize].kind, NodeKind::Link(_)))
            .filter_map(|ix| tree.bytes(ix))
            .filter(|(start, _)| source.as_bytes()[*start as usize] != b'[')
            .collect();
        failed_runs.retain(|at| {
            !autolinks
                .iter()
                .any(|(start, end)| (*start..*end).contains(at))
        });
        crossed_line_endings(&mut tree, Tree::ROOT, &mut Vec::new(), &failed_runs);
    }
    Ok(tree)
}

/// micromark: a failed code text attempt (an unclosed `` ` `` or `$$` run) scans on to the
/// paragraph's end, and inside containers every line ending it crossed then ends past the
/// next line's container prefixes. Only a text or break node that ends with one, before
/// another node, shows it. `failed_runs` (ascending) are those runs: only plain text
/// holds one, as a run inside code, HTML, a link destination, or an autolink was never
/// tried as code.
fn crossed_line_endings(
    tree: &mut Tree<'_>,
    ix: NodeId,
    prefixes: &mut Vec<Prefix>,
    failed_runs: &[u32],
) {
    let source = tree.source;
    let prefix = match tree.nodes[ix as usize].kind {
        NodeKind::Paragraph => {
            if !prefixes.is_empty() {
                crossed_in_paragraph(tree, ix, prefixes, failed_runs);
            }
            return;
        }
        NodeKind::Blockquote => Some(Prefix::Quote),
        NodeKind::ListItem { .. } => tree
            .bytes(ix)
            .map(|(start, _)| Prefix::Item(item_content_column(source, start as usize))),
        NodeKind::FootnoteDefinition { .. } => Some(Prefix::Item(4)),
        _ => None,
    };
    prefixes.extend(prefix);
    let mut cursor = tree.nodes[ix as usize].first;
    while cursor != NIL {
        crossed_line_endings(tree, cursor, prefixes, failed_runs);
        cursor = tree.nodes[cursor as usize].next;
    }
    if prefix.is_some() {
        prefixes.pop();
    }
}

fn crossed_in_paragraph(
    tree: &mut Tree<'_>,
    paragraph: NodeId,
    prefixes: &[Prefix],
    failed_runs: &[u32],
) {
    let source = tree.source;
    let Some((paragraph_start, _)) = tree.bytes(paragraph) else {
        return;
    };
    let first_run = failed_runs[failed_runs.partition_point(|at| *at < paragraph_start)..]
        .first()
        .copied();
    let mut cursor = tree.nodes[paragraph as usize].first;
    while cursor != NIL {
        let next = tree.nodes[cursor as usize].next;
        if let Some((_, end)) = tree.bytes(cursor) {
            let text = &source[..end as usize];
            let crossed = first_run.is_some_and(|at| at < end);
            if matches!(
                tree.nodes[cursor as usize].kind,
                NodeKind::Text(_) | NodeKind::Break
            ) && crossed
                && next != NIL
                && text.ends_with(['\n', '\r'])
                && let At::Bytes(_, node_end) = &mut tree.nodes[cursor as usize].at
            {
                *node_end = prefix_end(source, prefixes, end as usize).0 as u32;
            }
        }
        cursor = next;
    }
}

/// Where `text` (source) holds a `` ` `` (or, with `math`, `$$`, or `$` with
/// `single_dollar`) that no backslash escapes.
fn code_runs(text: &str, syntax: Syntax) -> impl Iterator<Item = usize> + '_ {
    let bytes = text.as_bytes();
    (0..bytes.len()).filter(move |&at| {
        let run = bytes[at] == b'`'
            || syntax.math
                && bytes[at] == b'$'
                && (syntax.single_dollar || bytes.get(at + 1) == Some(&b'$'));
        run && !odd_backslashes(&bytes[..at])
    })
}

/// Whether `before` ends with an odd run of backslashes: the last one escapes what
/// follows.
fn odd_backslashes(before: &[u8]) -> bool {
    before.iter().rev().take_while(|b| **b == b'\\').count() % 2 == 1
}

/// Parses `source` with `syntax` into remark's MDAST JSON.
pub fn parse_json(source: &str, syntax: Syntax) -> Result<String, Error> {
    let source = without_nul(source);
    Ok(parse_arena(&source, syntax)?.to_json())
}

pub(crate) fn write_point(out: &mut String, point: Point) {
    out.push_str("{\"line\":");
    json_u32(out, point.line);
    out.push_str(",\"column\":");
    json_u32(out, point.column);
    out.push_str(",\"offset\":");
    json_u32(out, point.offset);
    out.push('}');
}

// ---------------------------------------------------------------------------
// mdast-util-gfm-autolink-literal 2.0.1 `transformGfmAutolinkLiterals` on the arena
// ---------------------------------------------------------------------------

fn autolink_transform(tree: &mut Tree<'_>) {
    let finders: [Finder; 2] = [autolink::find_url, autolink::find_email];
    for finder in finders {
        transform_children(tree, Tree::ROOT, finder);
    }
}

/// mdast-util-find-and-replace: changed text nodes become new, unpositioned nodes.
fn transform_children(tree: &mut Tree<'_>, parent: NodeId, finder: Finder) {
    let mut previous = NIL;
    let mut cursor = tree.nodes[parent as usize].first;
    while cursor != NIL {
        let next = tree.nodes[cursor as usize].next;
        match &tree.nodes[cursor as usize].kind {
            NodeKind::Text(value) => {
                if let Some(pieces) = autolink::replace(value, finder) {
                    let mut chain_first = NIL;
                    let mut chain_last = NIL;
                    for piece in pieces {
                        let ix = match piece {
                            Piece::Text(text) => {
                                tree.push(NodeKind::Text(Cow::Owned(text)), At::None)
                            }
                            Piece::Link { url, text } => {
                                let link = tree.push(
                                    NodeKind::Link(Box::new(Resource {
                                        url: Cow::Owned(url),
                                        title: None,
                                        alt: String::new(),
                                    })),
                                    At::None,
                                );
                                let child = tree.push(NodeKind::Text(Cow::Owned(text)), At::None);
                                tree.append(link, child);
                                link
                            }
                        };
                        if chain_first == NIL {
                            chain_first = ix;
                        } else {
                            tree.nodes[chain_last as usize].next = ix;
                        }
                        chain_last = ix;
                    }
                    tree.splice(parent, previous, (chain_first, chain_last), next);
                    previous = chain_last;
                    cursor = next;
                    continue;
                }
            }
            NodeKind::Link { .. } | NodeKind::LinkReference { .. } => {}
            _ => transform_children(tree, cursor, finder),
        }
        previous = cursor;
        cursor = next;
    }
}

// ---------------------------------------------------------------------------
// @streamdown/cjk 1.0.4 `remarkCjkAutolinkBoundary`
// ---------------------------------------------------------------------------

/// CJK punctuation that ends an autolink literal.
const CJK_AUTOLINK_BOUNDARY: [char; 20] = [
    '。', '．', '，', '、', '？', '！', '：', '；', '（', '）', '【', '】', '「', '」', '『', '』',
    '〈', '〉', '《', '》',
];

fn autolink_prefix(value: &str) -> Option<usize> {
    let lower = |prefix: &str| {
        value
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
            .then_some(prefix.len())
    };
    lower("https://")
        .or_else(|| lower("http://"))
        .or_else(|| lower("mailto:"))
        .or_else(|| lower("www."))
}

/// Streamdown's `plugins.cjk` transform: a link whose only child is text equal to its
/// URL (an autolink literal, or anything that looks like one) ends before the first CJK
/// boundary punctuation; the rest becomes text, relinked by GFM's literal transform. The
/// shortened link keeps its position; every new node has none.
pub fn cjk_autolink_boundary(tree: &mut Tree<'_>, parent: NodeId) {
    let mut cursor = tree.nodes[parent as usize].first;
    while cursor != NIL {
        let next = tree.nodes[cursor as usize].next;
        let Some(boundary) = autolink_boundary(tree, cursor) else {
            cjk_autolink_boundary(tree, cursor);
            cursor = next;
            continue;
        };
        let NodeKind::Link(resource) = &mut tree.nodes[cursor as usize].kind else {
            unreachable!("autolink_boundary only accepts links");
        };
        let rest = resource.url[boundary..].to_owned();
        let head = resource.url[..boundary].to_owned();
        resource.url = Cow::Owned(head.clone());
        let text = tree.push(NodeKind::Text(Cow::Owned(head)), At::None);
        tree.nodes[cursor as usize].first = text;
        tree.nodes[cursor as usize].last = text;
        // The trailing pieces, relinked in a scratch paragraph, follow the link.
        let scratch = tree.push(NodeKind::Paragraph, At::None);
        let mut segment_start = 0;
        for (at, c) in rest.char_indices() {
            if CJK_AUTOLINK_BOUNDARY.contains(&c) {
                relinkified(tree, scratch, &rest[segment_start..at]);
                relinkified(tree, scratch, &rest[at..at + c.len_utf8()]);
                segment_start = at + c.len_utf8();
            }
        }
        relinkified(tree, scratch, &rest[segment_start..]);
        let (first, last) = (
            tree.nodes[scratch as usize].first,
            tree.nodes[scratch as usize].last,
        );
        let mut tail = cursor;
        if first != NIL {
            tree.nodes[cursor as usize].next = first;
            tree.nodes[last as usize].next = next;
            tail = last;
        }
        if next == NIL {
            tree.nodes[parent as usize].last = tail;
        }
        cursor = next;
    }
}

/// Where `link`'s URL meets CJK boundary punctuation, if it is an autolink literal that
/// `remarkCjkAutolinkBoundary` splits there (never at its first character).
fn autolink_boundary(tree: &Tree<'_>, link: NodeId) -> Option<usize> {
    let node = &tree.nodes[link as usize];
    let NodeKind::Link(resource) = &node.kind else {
        return None;
    };
    let child = node.first;
    if child == NIL || child != node.last {
        return None;
    }
    let NodeKind::Text(text) = &tree.nodes[child as usize].kind else {
        return None;
    };
    if text.as_ref() != resource.url.as_ref() {
        return None;
    }
    autolink_prefix(&resource.url)?;
    resource
        .url
        .char_indices()
        .find(|(_, c)| CJK_AUTOLINK_BOUNDARY.contains(c))
        .map(|(at, _)| at)
        .filter(|at| *at > 0)
}

/// `relinkifyTrailingText`'s `append`: `text` as a text node in `scratch`, through GFM's
/// literal transform unless it is a bare prefix such as `www.`.
fn relinkified(tree: &mut Tree<'_>, scratch: NodeId, text: &str) {
    if text.is_empty() {
        return;
    }
    let paragraph = tree.push(NodeKind::Paragraph, At::None);
    let node = tree.push(NodeKind::Text(Cow::Owned(text.to_owned())), At::None);
    tree.append(paragraph, node);
    if autolink_prefix(text) != Some(text.len()) {
        for finder in [autolink::find_url as Finder, autolink::find_email] {
            transform_children(tree, paragraph, finder);
        }
    }
    let mut cursor = tree.nodes[paragraph as usize].first;
    while cursor != NIL {
        let next = tree.nodes[cursor as usize].next;
        tree.nodes[cursor as usize].next = NIL;
        tree.append(scratch, cursor);
        cursor = next;
    }
}

// ---------------------------------------------------------------------------
// micromark position helpers
// ---------------------------------------------------------------------------

/// Whether a blank line separates two sibling blocks: only container prefixes and
/// whitespace occur between them, so two line endings mean a blank line.
fn blank_between(source: &str, end: usize, start: usize) -> bool {
    line_endings(source.as_bytes().get(end..start).unwrap_or_default()) >= 2
}

/// micromark's indented code past pulldown's end: whitespace-only lines continue it (blank
/// ones only when more follows), a line with four columns of prefix extends it, and what
/// follows that prefix is code. Returns the new end and the value's suffix, whose final
/// line ending `onexitcodeindented` drops.
fn indented_tail(source: &str, end: usize) -> (usize, String) {
    let bytes = source.as_bytes();
    let (mut tail, mut committed) = (end, 0);
    let mut suffix = String::new();
    let mut at = end;
    while let Some(found) = source[at..].find(['\n', '\r']) {
        let eol = at + found;
        let line_start = eol + line_ending_len(source, eol);
        let line_end = end_of_line(source, line_start);
        let mut column = 0;
        let mut cursor = line_start;
        while cursor < line_end && column < 4 && matches!(bytes[cursor], b' ' | b'\t') {
            column = next_column(column, bytes[cursor]);
            cursor += 1;
        }
        let rest = &source[cursor..line_end];
        if !rest.trim_matches([' ', '\t']).is_empty() || column < 4 && line_end == source.len() {
            break;
        }
        suffix.push_str(&source[eol..line_start]);
        if column >= 4 {
            suffix.push_str(rest);
            committed = suffix.len();
            tail = if rest.is_empty() { cursor } else { line_end };
        }
        at = line_end;
    }
    suffix.truncate(committed);
    suffix.truncate(without_line_ending(&suffix));
    (tail, suffix)
}

/// Whether a heading's source opens with an ATX sequence: one to six `#`, then a space,
/// tab, line ending or EOF (`#*` heads a setext heading).
fn atx_heading(bytes: &[u8]) -> bool {
    let hashes = bytes.iter().take_while(|b| **b == b'#').count();
    (1..=6).contains(&hashes)
        && matches!(bytes.get(hashes), None | Some(b' ' | b'\t' | b'\n' | b'\r'))
}

/// An item's content column on its first line (pulldown's item start may sit before the
/// marker); a blank first line puts it one past the marker.
fn item_content_column(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut marker = start;
    while matches!(bytes.get(marker), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        marker += 1;
    }
    let line_start = start_of_line(source, marker);
    let end = list_item_prefix_end(source, marker);
    let rest = source[end..].trim_start_matches([' ', '\t']);
    let blank = rest.is_empty() || rest.starts_with(['\n', '\r']);
    end - line_start + usize::from(blank)
}

/// The end of micromark's `listItemPrefix` at `at`: the marker, then the whitespace before
/// the content (one column of it when the content is indented five or more, none on a
/// blank item).
fn list_item_prefix_end(source: &str, at: usize) -> usize {
    let bytes = source.as_bytes();
    let at = at + bytes[at..].iter().take_while(|b| **b == b' ').count();
    let digits = bytes[at..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    let marker = at + if digits > 0 { digits + 1 } else { 1 };
    let spaces = bytes[marker.min(bytes.len())..]
        .iter()
        .take_while(|b| matches!(b, b' ' | b'\t'))
        .count();
    match bytes.get(marker + spaces) {
        None | Some(b'\n' | b'\r') => marker,
        Some(_) if spaces > 4 => marker + 1,
        Some(_) => marker + spaces,
    }
}

/// The start of the line holding byte `at`: just past the line ending before it.
fn start_of_line(source: &str, at: usize) -> usize {
    source[..at].rfind(['\n', '\r']).map_or(0, |eol| eol + 1)
}

/// The end of the line holding byte `at`: its line ending, or the end of `source`.
fn end_of_line(source: &str, at: usize) -> usize {
    source[at..]
        .find(['\n', '\r'])
        .map_or(source.len(), |found| at + found)
}

/// The column after `byte` at `column`: tabs stop every four columns.
fn next_column(column: usize, byte: u8) -> usize {
    if byte == b'\t' {
        column + 4 - column % 4
    } else {
        column + 1
    }
}

/// The first byte at or after `at` that is not a space or tab.
fn skip_blanks(bytes: &[u8], at: usize) -> usize {
    at + bytes.get(at..).map_or(0, |rest| {
        rest.iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count()
    })
}

/// Line endings in `text`, a CRLF counting once (at its LF).
fn line_endings(text: &[u8]) -> usize {
    text.iter()
        .enumerate()
        .filter(|&(at, &byte)| byte == b'\n' || byte == b'\r' && text.get(at + 1) != Some(&b'\n'))
        .count()
}

fn line_ending_len(source: &str, at: usize) -> usize {
    if source[at..].starts_with("\r\n") {
        2
    } else {
        1
    }
}

/// `line` without up to `depth` blockquote markers (each after up to three spaces, with
/// one optional space or tab).
fn strip_quotes(mut line: &str, depth: usize) -> &str {
    for _ in 0..depth {
        let indent = line.len() - line.trim_start_matches(' ').len();
        match line[indent..].strip_prefix('>') {
            Some(rest) if indent <= 3 => line = rest.strip_prefix([' ', '\t']).unwrap_or(rest),
            _ => break,
        }
    }
    line
}

/// The end of a line's prefixes for `prefixes`, and whether the line continues them all:
/// a blockquote needs its marker, an item its indentation unless the line is blank.
fn prefix_end(source: &str, prefixes: &[Prefix], line_start: usize) -> (usize, bool) {
    let bytes = source.as_bytes();
    let blank = |at: usize| {
        bytes[at..]
            .iter()
            .take_while(|b| !matches!(b, b'\n' | b'\r'))
            .all(|b| matches!(b, b' ' | b'\t'))
    };
    let mut at = line_start;
    for prefix in prefixes {
        match *prefix {
            Prefix::Quote => {
                let indent = bytes[at..]
                    .iter()
                    .take(3)
                    .take_while(|b| **b == b' ')
                    .count();
                if bytes.get(at + indent) != Some(&b'>') {
                    return (at, false);
                }
                at += indent + 1;
                if matches!(bytes.get(at), Some(b' ' | b'\t')) {
                    at += 1;
                }
            }
            Prefix::Item(column) => {
                let column = line_start + column;
                while at < column && bytes.get(at) == Some(&b' ') {
                    at += 1;
                }
                if at < column && !blank(at) {
                    return (at, false);
                }
            }
        }
    }
    (at, true)
}

/// An unclosed fence in containers (`prefixes`) that nothing follows: where micromark
/// ends it and its value. Each line continues the containers or closes them (the code
/// then ends before the line ending ahead of it); a last line of only container prefixes
/// ends it at that line's start; past the prefixes, the fence's own indentation is
/// stripped and the rest is code.
fn fence_at_eof(source: &str, prefixes: &[Prefix], start: usize) -> (usize, String) {
    // On the fence's line an item may open with its marker: its prefix reaches the item's
    // content column whatever stands there.
    let fence_line = start_of_line(source, start);
    let mut own = fence_line;
    for prefix in prefixes {
        match *prefix {
            Prefix::Quote => own = prefix_end(source, &[Prefix::Quote], own).0,
            Prefix::Item(column) => own = own.max(fence_line + column),
        }
    }
    let indent = start.saturating_sub(own);
    let mut value = String::new();
    let mut at = end_of_line(source, start);
    let mut last_empty = false;
    let mut lines = 0;
    while at < source.len() {
        let eol_end = at + line_ending_len(source, at);
        let line_end = end_of_line(source, eol_end);
        let (prefix, continued) = prefix_end(source, prefixes, eol_end);
        if !continued {
            break;
        }
        if line_end == source.len() && prefix >= line_end {
            // Only prefixes: the code keeps the line ending before them.
            return (eol_end, value);
        }
        let rest = &source[prefix..line_end];
        let content = &rest[rest.bytes().take(indent).take_while(|b| *b == b' ').count()..];
        if lines > 0 {
            value.push_str(&source[at..eol_end]);
        }
        value.push_str(content);
        lines += 1;
        last_empty = content.is_empty();
        at = line_end;
    }
    if last_empty {
        // micromark's data ends with the line ending before an empty last line.
        value.truncate(without_line_ending(&value));
    }
    (at, value)
}

/// The line that closed a node pulldown ends at `closing`: at that line's start, past its
/// prefixes, or at the line ending before it. Returns the line's start and end.
fn closing_line(source: &str, closing: usize) -> (usize, usize) {
    let line_start = if matches!(source.as_bytes().get(closing), Some(b'\n' | b'\r')) {
        closing + line_ending_len(source, closing)
    } else {
        start_of_line(source, closing.min(source.len()))
    };
    let line_end = end_of_line(source, line_start);
    (line_start, line_end)
}

/// subtokenize: a container exit at `point` moves back over indentation and line endings
/// to the first line ending it passes, stopping at content (which ends at `content_end`,
/// possibly past a line ending it took) or a blockquote prefix.
fn container_exit(source: &str, point: usize, content_end: usize) -> usize {
    let bytes = source.as_bytes();
    let mut end = point;
    let mut at = point;
    loop {
        let line_start = start_of_line(source, at);
        if line_start == 0
            || !bytes[line_start..at]
                .iter()
                .all(|b| matches!(b, b' ' | b'\t'))
        {
            return end;
        }
        let mut eol = line_start - 1;
        if bytes[eol] == b'\n' && eol > 0 && bytes[eol - 1] == b'\r' {
            eol -= 1;
        }
        if eol < content_end {
            return end;
        }
        end = eol;
        at = eol;
    }
}

/// micromark block positions end before the terminal line ending.
fn trim_line_ending(source: &str, start: usize, mut end: usize) -> usize {
    let bytes = source.as_bytes();
    if end > start && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    if end > start && bytes[end - 1] == b'\r' {
        end -= 1;
    }
    end
}

/// Indented code starts after its container prefix, at its four columns of indentation:
/// the first byte at or past four columns before its content, where `synthesized` spaces
/// are the unconsumed part of a tab.
fn indented_code_start(source: &str, start: usize, synthesized: usize) -> usize {
    let bytes = source.as_bytes();
    let mut region = start;
    while region > 0 && matches!(bytes[region - 1], b' ' | b'\t') {
        region -= 1;
    }
    let line_start = start_of_line(source, start);
    let column_at = |end: usize| {
        bytes[line_start..end]
            .iter()
            .fold(0, |column, &byte| next_column(column, byte))
    };
    let target = column_at(start).saturating_sub(synthesized + 4);
    let mut at = region;
    while at < start && column_at(at) < target {
        at += 1;
    }
    at
}

/// The raw info string after a fence: leading spaces and tabs skipped, the rest kept.
fn fence_info_raw(source: &str, start: usize) -> &str {
    let bytes = source.as_bytes();
    let fence = bytes[start];
    let mut at = start;
    while bytes.get(at) == Some(&fence) {
        at += 1;
    }
    let at = skip_blanks(bytes, at);
    let end = end_of_line(source, at);
    &source[at..end]
}

/// micromark splits fenced-code info at the first space or tab; meta keeps its tail.
fn fence_info(source: &str, start: usize) -> (Option<Box<str>>, Option<Box<str>>) {
    let info = fence_info_raw(source, start);
    if info.is_empty() {
        return (None, None);
    }
    match info.find([' ', '\t']) {
        Some(split) => {
            let lang = &info[..split];
            let meta = info[split..].trim_start_matches([' ', '\t']);
            (
                Some(rill_cmark::decode_string(lang).into()),
                (!meta.is_empty()).then(|| rill_cmark::decode_string(meta).into()),
            )
        }
        None => (Some(rill_cmark::decode_string(info).into()), None),
    }
}

/// micromark's end for a fenced code or math block that pulldown ends at `end`, fixing
/// its value; `true` defers the end to what opens the closing line (see `settle_exits`).
fn fence_end<'s>(
    source: &'s str,
    value: &mut Cow<'s, str>,
    start: usize,
    end: usize,
    in_container: bool,
) -> (usize, bool) {
    let unclosed = !has_closing_fence(source, start, end, in_container);
    if !unclosed {
        return (trim_blank_lines(source, start, end), false);
    }
    if !in_container {
        // An unclosed fence runs to the end of the document, keeping trailing blank lines
        // in its value.
        let body = unclosed_fence_body(source, start);
        if value.len() < body.len() && body.starts_with(value.as_ref()) {
            *value = body;
        }
        return (source.len(), false);
    }
    (trim_blank_lines(source, start, end), true)
}

/// An unclosed top-level fence's value: every line after the fence to EOF, less one final
/// line ending, each without up to the fence's own indentation.
fn unclosed_fence_body(source: &str, start: usize) -> Cow<'_, str> {
    let indent = start - start_of_line(source, start);
    let body_start = end_of_line(source, start);
    let body = &source[body_start..];
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix(['\n', '\r']))
        .unwrap_or(body);
    let body = &body[..without_line_ending(body)];
    if indent == 0 {
        return Cow::Borrowed(body);
    }
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    loop {
        let spaces = rest.bytes().take(indent).take_while(|b| *b == b' ').count();
        rest = &rest[spaces..];
        let Some(eol) = rest.find(['\n', '\r']) else {
            if rest.is_empty() && spaces > 0 {
                // The indent emptied the last line: micromark's data ends with the line
                // ending before it, which the value drops.
                out.truncate(without_line_ending(&out));
                return Cow::Owned(out);
            }
            out.push_str(rest);
            return Cow::Owned(out);
        };
        let next = eol + line_ending_len(rest, eol);
        out.push_str(&rest[..next]);
        rest = &rest[next..];
    }
}

/// `value`'s length without one terminal line ending (`\n`, `\r\n` or `\r`).
fn without_line_ending(value: &str) -> usize {
    let mut length = value.len();
    if value.ends_with('\n') {
        length -= 1;
    }
    if value[..length].ends_with('\r') {
        length -= 1;
    }
    length
}

/// Drops one terminal line ending (`\n`, `\r\n` or `\r`).
fn pop_line_ending(value: &mut Cow<'_, str>) {
    let length = without_line_ending(value);
    match value {
        Cow::Borrowed(borrowed) => *value = Cow::Borrowed(&borrowed[..length]),
        Cow::Owned(owned) => owned.truncate(length),
    }
}

fn has_closing_fence(source: &str, start: usize, end: usize, in_container: bool) -> bool {
    let bytes = source.as_bytes();
    let fence = bytes[start];
    let open = bytes[start..]
        .iter()
        .take_while(|byte| **byte == fence)
        .count();
    let block = &source[start..trim_line_ending(source, start, end)];
    let Some((_, last)) = block.rsplit_once(['\n', '\r']) else {
        return false;
    };
    // A closing fence is indented less than four columns past container prefixes; at the
    // top level a `>` is content.
    let unindented = if in_container {
        last.trim_start_matches([' ', '\t', '>'])
    } else {
        last.trim_start_matches([' ', '\t'])
    };
    if !in_container && last.len() - unindented.len() >= 4 {
        return false;
    }
    let last = unindented;
    let run = last.bytes().take_while(|byte| *byte == fence).count();
    run >= open && last[run..].trim_matches([' ', '\t']).is_empty()
}

/// HTML block types 1-5 end only on their terminator; unterminated ones reach the end.
fn unterminated_raw_html(source: &str, start: usize, end: usize) -> bool {
    let block = &source.as_bytes()[start..end];
    let starts_with = |at: usize, prefix: &[u8]| {
        block
            .get(at..at + prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    };
    let raw = [b"<script".as_slice(), b"<pre", b"<style", b"<textarea"]
        .iter()
        .any(|open| {
            starts_with(0, open)
                && block
                    .get(open.len())
                    .is_none_or(|byte| matches!(byte, b' ' | b'\t' | b'>' | b'\n' | b'\r'))
        });
    if raw {
        // Type 1 ends on any of its closing tags, in any case.
        let closes = [
            b"</script>".as_slice(),
            b"</pre>",
            b"</style>",
            b"</textarea>",
        ];
        return !memchr::memmem::find_iter(block, b"</")
            .any(|at| closes.iter().any(|close| starts_with(at, close)));
    }
    let terminator: &[u8] = if starts_with(0, b"<!--") {
        b"-->"
    } else if starts_with(0, b"<?") {
        b"?>"
    } else if starts_with(0, b"<![cdata[") {
        b"]]>"
    } else if starts_with(0, b"<!") && block.get(2).is_some_and(u8::is_ascii_alphabetic) {
        b">"
    } else {
        return false;
    };
    memchr::memmem::find(&block[2..], terminator).is_none()
}

/// Code and HTML blocks end on their last non-blank line, keeping its trailing spaces.
/// `trim_blank_lines` past up to `depth` quote markers: a line that only continues the
/// enclosing quotes is blank (concrete blocks open no containers, so more `>` is content).
fn trim_quoted_blank_lines(source: &str, start: usize, mut end: usize, depth: usize) -> usize {
    loop {
        let line_start = start_of_line(source, end);
        if line_start <= start
            || !strip_quotes(&source[line_start..end], depth)
                .trim_matches([' ', '\t'])
                .is_empty()
        {
            return end;
        }
        end = trim_line_ending(source, start, line_start);
    }
}

fn trim_blank_lines(source: &str, start: usize, mut end: usize) -> usize {
    let bytes = source.as_bytes();
    loop {
        let mut content = end;
        while content > start && matches!(bytes[content - 1], b' ' | b'\t') {
            content -= 1;
        }
        if content > start && matches!(bytes[content - 1], b'\n' | b'\r') {
            end = trim_line_ending(source, start, content);
        } else {
            return end;
        }
    }
}

/// Where `source`'s link definitions are, as UTF-16 `[start, end)` ranges, when each
/// is a child of the root; `None` when one sits in a container (a footnote definition
/// is one).
#[must_use]
pub fn root_definitions(source: &str, syntax: Syntax) -> Option<Vec<(u32, u32)>> {
    let found = root_only(source, syntax, |kind| {
        matches!(kind, NodeKind::Definition(_)).then_some(())
    })?;
    Some(
        found
            .into_iter()
            .map(|(start, end, ())| (start, end))
            .collect(),
    )
}

/// `source`'s footnote definitions as UTF-16 `[start, end)` ranges with their labels,
/// when each is a child of the root; `None` when one sits in a container.
#[must_use]
pub fn root_footnotes(source: &str, syntax: Syntax) -> Option<Vec<(u32, u32, Box<str>)>> {
    root_only(source, syntax, |kind| match kind {
        NodeKind::FootnoteDefinition(association) => Some(association.label.clone()),
        _ => None,
    })
}

/// What `pick` takes from each node of `source` it takes anything from, with the node's
/// UTF-16 range, when each such node is a child of the root.
fn root_only<T>(
    source: &str,
    syntax: Syntax,
    pick: impl Fn(&NodeKind<'_>) -> Option<T>,
) -> Option<Vec<(u32, u32, T)>> {
    let tree = parse_arena(source, syntax).ok()?;
    let mut found = Vec::new();
    for child in tree.children(Tree::ROOT) {
        let node = tree.node(child);
        if let (Some(value), At::Bytes(start, end)) = (pick(&node.kind), node.at) {
            found.push((tree.points.units(start), tree.points.units(end), value));
        }
    }
    let all = tree
        .nodes
        .iter()
        .filter(|node| pick(&node.kind).is_some())
        .count();
    (all == found.len()).then_some(found)
}

/// Whether `source` parses into root children none of which spans UTF-16 offset `at`,
/// and the text from `at` parses on its own into the same blocks as it does after the
/// text before it: the text before it and the text after it then render on their own as
/// they do together. (Without the second test, what the text before `at` leaves open can
/// still change the text after it: after a definition's content, or a list's item, an
/// empty list item cannot interrupt, so it is a paragraph there and a list alone.)
#[must_use]
pub fn splits_at(source: &str, syntax: Syntax, at: u32) -> bool {
    let Ok(tree) = parse_arena(source, syntax) else {
        return false;
    };
    let mut first_after = None;
    for child in tree.children(Tree::ROOT) {
        let At::Bytes(start, end) = tree.node(child).at else {
            return false;
        };
        if tree.points.units(start) >= at {
            first_after.get_or_insert(child);
        } else if tree.points.units(end) > at {
            return false;
        }
    }
    // The byte offset of `at`.
    let mut units = 0;
    let split = source
        .char_indices()
        .find_map(|(byte, ch)| {
            let here = units;
            units += ch.len_utf16() as u32;
            (here >= at).then_some(byte)
        })
        .unwrap_or(source.len());
    let Ok(alone) = parse_arena(&source[split..], syntax) else {
        return false;
    };
    same_nodes(
        &tree,
        first_after,
        &alone,
        alone.node(Tree::ROOT).first,
        split as u32,
    )
}

/// Whether two nodes are of the same kind, down to what their source alone does not
/// settle (a list's looseness, indented code on a lazy line, which ends it).
fn same_kind(x: &NodeKind<'_>, y: &NodeKind<'_>) -> bool {
    match (x, y) {
        (NodeKind::Code(a), NodeKind::Code(b)) | (NodeKind::Math(a), NodeKind::Math(b)) => {
            a.fenced == b.fenced && a.lazy == b.lazy
        }
        (
            NodeKind::List {
                ordered,
                start,
                spread,
            },
            NodeKind::List {
                ordered: other_ordered,
                start: other_start,
                spread: other_spread,
            },
        ) => ordered == other_ordered && start == other_start && spread == other_spread,
        (
            NodeKind::ListItem { spread, checked },
            NodeKind::ListItem {
                spread: other_spread,
                checked: other_checked,
            },
        ) => spread == other_spread && checked == other_checked,
        (NodeKind::Heading(a), NodeKind::Heading(b)) => a == b,
        _ => std::mem::discriminant(x) == std::mem::discriminant(y),
    }
}

/// Whether the siblings from `left` in `a` and from `right` in `b` are the same blocks,
/// at the same places (`b`'s shifted by `shift` bytes).
fn same_nodes(a: &Tree<'_>, left: Option<NodeId>, b: &Tree<'_>, right: NodeId, shift: u32) -> bool {
    let mut left = left.unwrap_or(NIL);
    let mut right = right;
    while left != NIL && right != NIL {
        let (x, y) = (a.node(left), b.node(right));
        let place = |at: At, by: u32| match at {
            At::None => None,
            At::Bytes(start, end) => Some((0, start + by, end + by)),
            At::PastLineEnding { line_ending, end } => Some((1, line_ending + by, end + by)),
        };
        // Only blocks: inline content depends on its own text and the definitions, which
        // each piece renders with.
        let container = matches!(
            x.kind,
            NodeKind::Blockquote | NodeKind::List { .. } | NodeKind::ListItem { .. }
        );
        if !same_kind(&x.kind, &y.kind)
            || place(x.at, 0) != place(y.at, shift)
            || container && !same_nodes(a, Some(x.first), b, y.first, shift)
        {
            return false;
        }
        left = x.next;
        right = y.next;
    }
    left == NIL && right == NIL
}
