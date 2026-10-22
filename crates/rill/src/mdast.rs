//! MDAST: rill-cmark's events become the tree remark builds (remark-parse with remark-gfm,
//! remark-math and remark-cjk-friendly), in an arena. Nodes link by index, text borrows the
//! source where it can, and positions stay byte offsets until serialization, where they
//! become JavaScript's UTF-16 units.

use std::borrow::Cow;

use rill_cmark::{Alignment, Event, LinkType, Parser, Tag, TagEnd};

use crate::autolink::{self, Finder, Piece};
use crate::json::{json_string, json_u32};

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

/// An event stream the builder cannot turn into MDAST. Every UTF-8 string is valid
/// Markdown, so these mean a bug in the vendored parser, never bad input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// An end event with no open node, or a mismatched end.
    Unbalanced,
    /// A child for a node that cannot have children.
    LeafParent,
    /// An event for an extension the native options disable.
    DisabledExtension,
    /// NUL must be replaced (see [`without_nul`]) before parsing.
    Nul,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::Unbalanced => "unbalanced parser events",
            Error::LeafParent => "child event inside a leaf node",
            Error::DisabledExtension => "event for a disabled extension",
            Error::Nul => "source contains NUL",
        })
    }
}

impl std::error::Error for Error {}

/// micromark replaces NUL with U+FFFD; both are one UTF-16 unit, so positions agree.
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
pub struct Pt {
    pub line: u32,
    pub column: u32,
    pub offset: u32,
}

/// Index of the first non-ASCII byte, checking 16 bytes at a time.
fn first_non_ascii(bytes: &[u8]) -> Option<usize> {
    let ascii_run = bytes
        .chunks_exact(16)
        .take_while(|chunk| chunk.is_ascii())
        .count()
        * 16;
    let rest = bytes[ascii_run..]
        .iter()
        .position(|byte| !byte.is_ascii())?;
    Some(ascii_run + rest)
}

/// Byte offset → unist point in UTF-16 units, as remark reports them. Offsets must fall on
/// character boundaries.
pub struct Points {
    line_starts: Vec<u32>,
    /// For each non-ASCII character, the byte just past it and how many more UTF-8 bytes
    /// than UTF-16 units the source holds up to there. Empty for ASCII sources.
    narrowing: Vec<(u32, u32)>,
}

impl Points {
    pub fn new(source: &str) -> Self {
        let bytes = source.as_bytes();
        let mut line_starts = vec![0];
        for found in memchr::memchr2_iter(b'\n', b'\r', bytes) {
            // A CRLF ends one line, at its LF.
            if !(bytes[found] == b'\r' && bytes.get(found + 1) == Some(&b'\n')) {
                line_starts.push(found as u32 + 1);
            }
        }
        let mut narrowing = Vec::new();
        let mut narrowed = 0;
        let mut at = 0;
        while let Some(skip) = first_non_ascii(&bytes[at..]) {
            at += skip;
            let character = source[at..]
                .chars()
                .next()
                .expect("whole characters are skipped, so `at` starts one");
            at += character.len_utf8();
            narrowed += (character.len_utf8() - character.len_utf16()) as u32;
            narrowing.push((at as u32, narrowed));
        }
        Points {
            line_starts,
            narrowing,
        }
    }

    pub fn units(&self, byte: u32) -> u32 {
        match self.narrowing.partition_point(|&(end, _)| end <= byte) {
            0 => byte,
            index => byte - self.narrowing[index - 1].1,
        }
    }

    pub fn pt(&self, byte: u32) -> Pt {
        let line = self.line_starts.partition_point(|start| *start <= byte);
        let offset = self.units(byte);
        let column = offset - self.units(self.line_starts[line - 1]) + 1;
        Pt {
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
    /// Explicit points, where micromark reports a position no source byte has.
    Points(Pt, Pt),
}

// ---------------------------------------------------------------------------
// Arena MDAST
// ---------------------------------------------------------------------------

pub type Ix = u32;
pub const NIL: Ix = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub enum MKind<'s> {
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
    Code {
        value: Cow<'s, str>,
        lang: Option<String>,
        meta: Option<String>,
        fenced: bool,
    },
    Math {
        value: Cow<'s, str>,
        meta: Option<String>,
    },
    Definition {
        url: Cow<'s, str>,
        title: Option<Cow<'s, str>>,
        identifier: String,
        label: String,
    },
    Text(Cow<'s, str>),
    Emphasis,
    Strong,
    Delete,
    InlineCode(Cow<'s, str>),
    InlineMath(Cow<'s, str>),
    Break,
    Link {
        url: Cow<'s, str>,
        title: Option<Cow<'s, str>>,
    },
    LinkReference {
        kind: ReferenceKind,
        identifier: String,
        label: String,
    },
    Image {
        url: Cow<'s, str>,
        title: Option<Cow<'s, str>>,
        alt: String,
    },
    ImageReference {
        kind: ReferenceKind,
        identifier: String,
        label: String,
        alt: String,
    },
    FootnoteDefinition {
        identifier: String,
        label: String,
    },
    FootnoteReference {
        identifier: String,
        label: String,
    },
    Table(Vec<AlignKind>),
    TableRow,
    TableCell,
}

impl MKind<'_> {
    /// The mdast `type`.
    pub fn type_name(&self) -> &'static str {
        match self {
            MKind::Root => "root",
            MKind::Paragraph => "paragraph",
            MKind::Heading(_) => "heading",
            MKind::ThematicBreak => "thematicBreak",
            MKind::Blockquote => "blockquote",
            MKind::List { .. } => "list",
            MKind::ListItem { .. } => "listItem",
            MKind::Html(_) => "html",
            MKind::Code { .. } => "code",
            MKind::Math { .. } => "math",
            MKind::Definition { .. } => "definition",
            MKind::Text(_) => "text",
            MKind::Emphasis => "emphasis",
            MKind::Strong => "strong",
            MKind::Delete => "delete",
            MKind::InlineCode(_) => "inlineCode",
            MKind::InlineMath(_) => "inlineMath",
            MKind::Break => "break",
            MKind::Link { .. } => "link",
            MKind::LinkReference { .. } => "linkReference",
            MKind::Image { .. } => "image",
            MKind::ImageReference { .. } => "imageReference",
            MKind::FootnoteDefinition { .. } => "footnoteDefinition",
            MKind::FootnoteReference { .. } => "footnoteReference",
            MKind::Table(_) => "table",
            MKind::TableRow => "tableRow",
            MKind::TableCell => "tableCell",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MNode<'s> {
    pub kind: MKind<'s>,
    pub first: Ix,
    pub last: Ix,
    pub next: Ix,
    pub at: At,
}

pub struct MTree<'s> {
    pub source: &'s str,
    pub nodes: Vec<MNode<'s>>,
    pub points: Points,
}

impl<'s> MTree<'s> {
    pub const ROOT: Ix = 0;

    /// A new, detached node.
    pub fn push(&mut self, kind: MKind<'s>, at: At) -> Ix {
        let ix = self.nodes.len() as Ix;
        self.nodes.push(MNode {
            kind,
            first: NIL,
            last: NIL,
            next: NIL,
            at,
        });
        ix
    }

    /// Appends detached `child` to `parent`'s children.
    pub fn append(&mut self, parent: Ix, child: Ix) {
        let last = self.nodes[parent as usize].last;
        if last == NIL {
            self.nodes[parent as usize].first = child;
        } else {
            self.nodes[last as usize].next = child;
        }
        self.nodes[parent as usize].last = child;
    }

    pub fn node(&self, ix: Ix) -> &MNode<'s> {
        &self.nodes[ix as usize]
    }

    pub fn children(&self, parent: Ix) -> Children<'_, 's> {
        Children {
            tree: self,
            next: self.nodes[parent as usize].first,
        }
    }

    fn bytes(&self, ix: Ix) -> Option<(u32, u32)> {
        match self.nodes[ix as usize].at {
            At::Bytes(start, end) => Some((start, end)),
            _ => None,
        }
    }

    pub fn pt_range(&self, at: At) -> Option<(Pt, Pt)> {
        match at {
            At::None => None,
            At::Bytes(start, end) => Some((self.points.pt(start), self.points.pt(end))),
            At::Points(start, end) => Some((start, end)),
        }
    }
}

pub struct Children<'t, 's> {
    tree: &'t MTree<'s>,
    next: Ix,
}

impl Iterator for Children<'_, '_> {
    type Item = Ix;
    fn next(&mut self) -> Option<Ix> {
        (self.next != NIL).then(|| {
            let ix = self.next;
            self.next = self.tree.nodes[ix as usize].next;
            ix
        })
    }
}

/// micromark `normalizeIdentifier` followed by mdast-util-from-markdown's lower-casing.
pub fn normalize_identifier(label: &str) -> String {
    let collapsed = label
        .split(['\t', '\n', '\r', ' '])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.to_lowercase().to_uppercase().to_lowercase()
}

fn cow<'s>(value: rill_cmark::CowStr<'s>) -> Cow<'s, str> {
    match value {
        rill_cmark::CowStr::Borrowed(borrowed) => Cow::Borrowed(borrowed),
        other => Cow::Owned(other.to_string()),
    }
}

/// Where `slice` sits in `source`, if it is a subslice of it.
fn source_offset(source: &str, slice: &str) -> Option<usize> {
    let start = (slice.as_ptr() as usize).checked_sub(source.as_ptr() as usize)?;
    (start + slice.len() <= source.len()).then_some(start)
}

fn push_cow<'s>(target: &mut Cow<'s, str>, value: &str) {
    if target.is_empty() {
        *target = Cow::Owned(value.to_owned());
    } else {
        target.to_mut().push_str(value);
    }
}

// ---------------------------------------------------------------------------
// Builder: pulldown events → arena
// ---------------------------------------------------------------------------

struct Frame {
    ix: Ix,
    start: usize,
    end: usize,
    /// Spaces pulldown synthesized from a partly consumed tab at an indented code start.
    synthesized: usize,
}

/// A closed node whose end waits on what opens the line that closed it.
struct Exit {
    ix: Ix,
    settle: Settle,
    /// pulldown's end: the start of the line that closed it.
    closing: usize,
    content_end: usize,
}

#[derive(Clone, PartialEq)]
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
    Item { list: Ix },
}

/// An open container's line prefix: a quote marker, or indentation to an item's content
/// column.
#[derive(Clone, Copy, PartialEq)]
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

struct Builder<'s> {
    tree: MTree<'s>,
    stack: Vec<Frame>,
    text: Option<(Cow<'s, str>, usize, usize)>,
    /// Plain text collected for the open image's alt; nested images only add text.
    alt: Option<String>,
    nested_images: usize,
    /// The open task item's paragraph with its check's byte span.
    task: Option<(Ix, usize, usize)>,
    /// Starts micromark reports at no real source point (see `task_paragraph`).
    starts: Vec<(Ix, Pt)>,
    exits: Vec<Exit>,
}

impl<'s> Builder<'s> {
    fn parent(&self) -> Result<Ix, Error> {
        self.stack
            .last()
            .map(|frame| frame.ix)
            .ok_or(Error::Unbalanced)
    }

    fn kind_mut(&mut self) -> Option<&mut MKind<'s>> {
        let ix = self.stack.last()?.ix;
        Some(&mut self.tree.nodes[ix as usize].kind)
    }

    fn leaf(&mut self, kind: MKind<'s>, at: At) -> Result<(), Error> {
        self.flush_text()?;
        let parent = self.parent()?;
        let ix = self.tree.push(kind, at);
        self.tree.append(parent, ix);
        Ok(())
    }

    fn flush_text(&mut self) -> Result<(), Error> {
        if let Some((value, start, end)) = self.text.take() {
            let parent = self.parent()?;
            let ix = self
                .tree
                .push(MKind::Text(value), At::Bytes(start as u32, end as u32));
            self.tree.append(parent, ix);
        }
        Ok(())
    }

    fn push_text(&mut self, value: Cow<'s, str>, start: usize, end: usize) {
        let source = self.tree.source;
        match &mut self.text {
            Some((text, _, text_end)) => {
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
            None => self.text = Some((value, start, end)),
        }
    }

    fn open(&mut self, kind: MKind<'s>, start: usize, end: usize) -> Result<(), Error> {
        self.flush_text()?;
        let parent = self.parent()?;
        let ix = self.tree.push(kind, At::None);
        self.tree.append(parent, ix);
        self.stack.push(Frame {
            ix,
            start,
            end,
            synthesized: 0,
        });
        Ok(())
    }

    /// Whether a blockquote encloses the node being closed.
    /// Blockquotes enclosing the node being closed.
    fn quote_depth(&self) -> usize {
        self.stack
            .iter()
            .rev()
            .skip(1)
            .filter(|frame| matches!(self.tree.nodes[frame.ix as usize].kind, MKind::Blockquote))
            .count()
    }

    fn in_container(&self) -> bool {
        self.stack.iter().rev().skip(1).any(|frame| {
            matches!(
                self.tree.nodes[frame.ix as usize].kind,
                MKind::Blockquote | MKind::ListItem { .. } | MKind::FootnoteDefinition { .. }
            )
        })
    }

    fn close(&mut self) -> Result<(), Error> {
        self.flush_text()?;
        if self.stack.len() < 2 {
            return Err(Error::Unbalanced);
        }
        let in_container = self.in_container();
        let quote_depth = self.quote_depth();
        let in_blockquote = quote_depth > 0;
        let Frame {
            ix,
            mut start,
            mut end,
            synthesized,
        } = self.stack.pop().ok_or(Error::Unbalanced)?;
        let source = self.tree.source;
        let bytes = source.as_bytes();
        let first_span = {
            let first = self.tree.nodes[ix as usize].first;
            (first != NIL).then(|| self.tree.bytes(first)).flatten()
        };
        let last_span = {
            let last = self.tree.nodes[ix as usize].last;
            (last != NIL).then(|| self.tree.bytes(last)).flatten()
        };
        let spread = self.spread(ix);
        let pulldown_end = end;
        let pending = !self.exits.is_empty();
        let fence_prefixes = match &self.tree.nodes[ix as usize].kind {
            MKind::Code { fenced: true, .. } | MKind::Math { .. } if in_container => {
                self.prefixes()
            }
            _ => Vec::new(),
        };
        let eof_prefixed = matches!(
            self.tree.nodes[ix as usize].kind,
            MKind::List { .. } | MKind::Blockquote | MKind::FootnoteDefinition { .. }
        ) && {
            let (line_start, line_end) = closing_line(source, end);
            line_end == source.len() && self.continued_prefix_end(line_start).0 == source.len()
        };
        let parent = self.stack.last().map_or(NIL, |frame| frame.ix);
        let mut exit = None;
        match &mut self.tree.nodes[ix as usize].kind {
            MKind::Code { value, fenced, .. } => {
                if *fenced {
                    let defer;
                    (end, defer) = fence_end(source, value, start, end, in_container);
                    if defer {
                        exit = Some(Exit {
                            ix,
                            settle: Settle::Fence {
                                in_blockquote,
                                prefixes: fence_prefixes.clone(),
                            },
                            closing: pulldown_end,
                            content_end: 0,
                        });
                    }
                } else {
                    start = indented_code_start(source, start, synthesized);
                    end = trim_blank_lines(source, start, end);
                    // ponytail: top level only; container prefixes would need the spine.
                    if !in_container {
                        let (tail, suffix) = indented_tail(source, end);
                        end = tail;
                        if !suffix.is_empty() {
                            value.to_mut().push_str(&suffix);
                        }
                    }
                }
            }
            MKind::Math { value, .. } => {
                let defer;
                (end, defer) = fence_end(source, value, start, end, in_container);
                if defer {
                    exit = Some(Exit {
                        ix,
                        settle: Settle::Fence {
                            in_blockquote,
                            prefixes: fence_prefixes.clone(),
                        },
                        closing: pulldown_end,
                        content_end: 0,
                    });
                }
            }
            MKind::Html(value) => {
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
            MKind::List {
                spread: list_spread,
                ..
            } => {
                if let Some((first, _)) = first_span {
                    start = first as usize;
                }
                *list_spread = spread;
                let content_end = last_span
                    .map_or(trim_blank_lines(source, start, end), |(_, last)| {
                        last as usize
                    });
                exit = Some(Exit {
                    ix,
                    settle: Settle::Container { eof_prefixed },
                    closing: end,
                    content_end,
                });
                end = content_end;
            }
            MKind::Blockquote => {
                let content_end = trim_blank_lines(source, start, end);
                exit = Some(Exit {
                    ix,
                    settle: Settle::Container { eof_prefixed },
                    closing: end,
                    content_end,
                });
                end = content_end;
            }
            MKind::ListItem {
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
                        ix,
                        settle: Settle::Item { list: parent },
                        closing: end,
                        content_end: end,
                    });
                }
            }
            // gfm-footnote definitions are micromark containers too.
            MKind::FootnoteDefinition { .. } => {
                let content_end = last_span
                    .map_or(trim_blank_lines(source, start, end), |(_, last)| {
                        last as usize
                    });
                exit = Some(Exit {
                    ix,
                    settle: Settle::Container { eof_prefixed },
                    closing: end,
                    content_end,
                });
                end = content_end;
            }
            MKind::TableCell => {
                if start > 0 && bytes[start - 1] == b'|' {
                    start -= 1;
                }
            }
            MKind::TableRow => {
                end = trim_line_ending(source, start, end);
                // The last cell runs through the closing pipe to the row end.
                let last = self.tree.nodes[ix as usize].last;
                if last != NIL
                    && let At::Bytes(_, cell_end) = &mut self.tree.nodes[last as usize].at
                {
                    *cell_end = (*cell_end).max(end as u32);
                }
            }
            MKind::Heading(_) => {
                end = trim_line_ending(source, start, end);
                // micromark: a setext heading's content includes the definitions that
                // opened its paragraph, so the heading starts at the first of them.
                if !atx_heading(&bytes[start..])
                    && let Some(parent) = self.stack.last().map(|frame| frame.ix)
                {
                    let mut previous = NIL;
                    let mut cursor = self.tree.nodes[parent as usize].first;
                    while cursor != NIL && cursor != ix {
                        previous = cursor;
                        cursor = self.tree.nodes[cursor as usize].next;
                    }
                    let mut earliest = start;
                    let mut candidate = previous;
                    // Definitions directly before, each ending on the line before.
                    while candidate != NIL {
                        let MKind::Definition { .. } = self.tree.nodes[candidate as usize].kind
                        else {
                            break;
                        };
                        let Some((def_start, def_end)) = self.tree.bytes(candidate) else {
                            break;
                        };
                        if line_endings(&source[def_end as usize..earliest]) != 1
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
            MKind::Paragraph | MKind::Table(_) | MKind::ThematicBreak => {
                end = trim_line_ending(source, start, end);
            }
            _ => {}
        }
        self.tree.nodes[ix as usize].at = At::Bytes(start as u32, end as u32);
        if let Some((paragraph, check_start, check_end)) = self.task
            && paragraph == ix
        {
            self.task = None;
            self.task_paragraph(ix, check_start, check_end, end);
        }
        if matches!(self.tree.nodes[ix as usize].kind, MKind::Paragraph)
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
        let source = self.tree.source;
        for Exit {
            ix,
            settle,
            closing,
            content_end,
        } in std::mem::take(&mut self.exits)
        {
            if let Settle::Item { list } = settle {
                // Only a sibling's prefix: an outer list's closes this list first.
                let sibling = self.stack.last().is_some_and(|frame| frame.ix == list);
                let last = self.tree.nodes[ix as usize].last;
                if let Some((_, last_end)) = (last != NIL).then(|| self.tree.bytes(last)).flatten()
                    && let At::Bytes(_, item_end) = &mut self.tree.nodes[ix as usize].at
                {
                    // prepareList: with no line ending before the next item's prefix, the
                    // item ends where that prefix does.
                    *item_end = match next {
                        Some((next_start, Opens::Item))
                            if sibling
                                && source.get(last_end as usize..next_start).is_some_and(
                                    |between| {
                                        between.bytes().all(|b| matches!(b, b' ' | b'\t' | b'>'))
                                    },
                                ) =>
                        {
                            list_item_prefix_end(source, next_start) as u32
                        }
                        _ => (*item_end).max(last_end),
                    };
                }
                continue;
            }
            let (line_start, line_end) = closing_line(source, closing);
            let on_line = |start: usize| (line_start..=line_end).contains(&start);
            if let Settle::Fence {
                in_blockquote,
                prefixes,
            } = &settle
            {
                if next.is_none()
                    && let At::Bytes(fence_start, _) = self.tree.nodes[ix as usize].at
                {
                    let (end, eof_value) = fence_at_eof(source, prefixes, fence_start as usize);
                    let node = &mut self.tree.nodes[ix as usize];
                    node.at = At::Bytes(fence_start, end as u32);
                    if let MKind::Code { value, .. } | MKind::Math { value, .. } = &mut node.kind {
                        *value = Cow::Owned(eof_value);
                    }
                    continue;
                }
                let in_blockquote = *in_blockquote;
                // A line ending joins the code only before a non-lazy line, so a lazy
                // closing line leaves the one before it. A blockquote does not continue
                // over a blank line without `>`, so such lines close it earlier.
                let closing_line = line_start;
                let mut line_start = line_start;
                while in_blockquote && line_start > 0 {
                    let previous = source[..line_start - 1]
                        .rfind(['\n', '\r'])
                        .map_or(0, |eol| eol + 1);
                    let line = source[previous..line_start].trim_end_matches(['\n', '\r']);
                    if previous == 0 || !line.trim_matches([' ', '\t']).is_empty() {
                        break;
                    }
                    line_start = previous;
                }
                let opens_container = line_start == closing_line
                    && matches!(next, Some((next_start, Opens::Container | Opens::Item)) if on_line(next_start));
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
                // `onexitcodefenced` strips one final line ending from data that, closed
                // lazily, already stops before the last one: a blank last line goes.
                if !opens_container
                    && let MKind::Code { value, .. } | MKind::Math { value, .. } =
                        &mut self.tree.nodes[ix as usize].kind
                    && value.ends_with(['\n', '\r'])
                {
                    pop_line_ending(value);
                }
                continue;
            }
            let point = match next {
                _ if closing >= source.len() => source.len(),
                Some((next_start, Opens::Container)) if on_line(next_start) => {
                    self.continued_prefix_end(line_start).0
                }
                // `flowStart` at EOF right after the prefixes; anything more is a lazy line.
                None if matches!(settle, Settle::Container { eof_prefixed: true }) => source.len(),
                _ => line_start,
            };
            // The walk back stops at content, which a child's settled end may extend.
            let last = self.tree.nodes[ix as usize].last;
            let content_end = (last != NIL)
                .then(|| self.tree.bytes(last))
                .flatten()
                .map_or(content_end, |(_, last_end)| {
                    content_end.max(last_end as usize)
                });
            let end = container_exit(source, point, content_end).max(content_end);
            if let At::Bytes(_, node_end) = &mut self.tree.nodes[ix as usize].at {
                *node_end = end as u32;
            }
            if let MKind::List { spread, .. } = &mut self.tree.nodes[ix as usize].kind {
                // prepareList: line endings between the content and the exit count toward
                // spread, and the last item reaches the exit unless one follows its content.
                if line_endings(&source[content_end..end]) >= 2 {
                    *spread = true;
                }
                self.extend_last_item(ix, end);
            }
        }
    }

    /// The open containers' line prefixes, outermost first.
    fn prefixes(&self) -> Vec<Prefix> {
        let source = self.tree.source;
        self.stack
            .iter()
            .filter_map(|frame| match self.tree.nodes[frame.ix as usize].kind {
                MKind::Blockquote => Some(Prefix::Quote),
                // ponytail: four columns from the line start, exact at the top level.
                MKind::FootnoteDefinition { .. } => Some(Prefix::Item(4)),
                MKind::ListItem { .. } => {
                    Some(Prefix::Item(item_content_column(source, frame.start)))
                }
                _ => None,
            })
            .collect()
    }

    fn continued_prefix_end(&self, line_start: usize) -> (usize, bool) {
        prefix_end(self.tree.source, &self.prefixes(), line_start)
    }

    fn extend_last_item(&mut self, list: Ix, list_end: usize) {
        let item = self.tree.nodes[list as usize].last;
        if item == NIL {
            return;
        }
        let child = self.tree.nodes[item as usize].last;
        let content_end = (child != NIL)
            .then(|| self.tree.bytes(child))
            .flatten()
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
    fn task_paragraph(&mut self, ix: Ix, check_start: usize, check_end: usize, end: usize) {
        let head = self.tree.nodes[ix as usize].first;
        if head == NIL || !matches!(self.tree.nodes[head as usize].kind, MKind::Text(_)) {
            self.tree.nodes[ix as usize].at = At::Bytes(check_start as u32, end as u32);
            return;
        }
        let bytes = self.tree.source.as_bytes();
        let mut eol = check_end;
        while matches!(bytes.get(eol), Some(b' ' | b'\t')) {
            eol += 1;
        }
        if !matches!(bytes.get(eol), Some(b'\n' | b'\r')) {
            return;
        }
        if bytes[eol..].starts_with(b"\r\n")
            && let MKind::Text(value) = &mut self.tree.nodes[head as usize].kind
        {
            // `slice(1)` drops only the CR.
            value.to_mut().insert(0, '\n');
        }
        let mut start = self.tree.points.pt(eol as u32);
        start.column += 1;
        start.offset += 1;
        self.starts.push((head, start));
        self.starts.push((ix, start));
    }

    /// Removes `ix`, the newest child of the current parent frame.
    fn unlink_last_child(&mut self, ix: Ix) {
        let Some(parent) = self.stack.last().map(|frame| frame.ix) else {
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

    fn previous_sibling(&self, parent: Ix, ix: Ix) -> Ix {
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
    fn spread(&self, ix: Ix) -> bool {
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
                    ix, start, mut end, ..
                } = self.stack.pop().ok_or(Error::Unbalanced)?;
                match &mut self.tree.nodes[ix as usize].kind {
                    MKind::Image { alt: slot, .. } => *slot = alt,
                    MKind::ImageReference {
                        alt: slot, kind, ..
                    } => {
                        *slot = alt;
                        if *kind == ReferenceKind::Collapsed
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
                    Some(
                        MKind::Code { value, .. } | MKind::Html(value) | MKind::Math { value, .. },
                    ) => pop_line_ending(value),
                    _ => return Err(Error::Unbalanced),
                }
                self.close()
            }
            Event::End(TagEnd::Link) => {
                let collapsed = matches!(
                    self.kind_mut(),
                    Some(MKind::LinkReference {
                        kind: ReferenceKind::Collapsed,
                        ..
                    })
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
                    Some(MKind::Code { value, .. } | MKind::Math { value, .. }) => {
                        let synthesized = value.is_empty() && start == end;
                        push_cow(value, &text);
                        if synthesized && let Some(frame) = self.stack.last_mut() {
                            frame.synthesized = text.len();
                        }
                        return Ok(());
                    }
                    Some(MKind::Html(value)) => {
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
                    .map(|frame| self.tree.nodes[frame.ix as usize].last)
                    .filter(|last| *last != NIL)
                    .and_then(|last| self.tree.bytes(last))
                    .map_or(0, |(_, end)| end as usize);
                let escaped = self.text.is_none()
                    && start > previous_end
                    && bytes[start - 1] == b'\\'
                    && bytes.get(start).is_some_and(u8::is_ascii_punctuation);
                self.push_text(cow(text), start - usize::from(escaped), end);
                Ok(())
            }
            Event::Html(html) => match self.kind_mut() {
                Some(MKind::Html(value)) => {
                    push_cow(value, &html);
                    Ok(())
                }
                _ => Err(Error::Unbalanced),
            },
            Event::InlineHtml(html) => self.leaf(MKind::Html(cow(html)), bytes(start, end)),
            Event::Code(code) => self.leaf(MKind::InlineCode(cow(code)), bytes(start, end)),
            Event::InlineMath(value) => self.leaf(MKind::InlineMath(cow(value)), bytes(start, end)),
            Event::FootnoteReference(label) => self.leaf(
                MKind::FootnoteReference {
                    identifier: normalize_identifier(&label),
                    label: label.to_string(),
                },
                bytes(start, end),
            ),
            Event::SoftBreak => {
                let ending = source[start..end].trim_start_matches([' ', '\t']);
                self.push_text(Cow::Borrowed(ending), start, end);
                Ok(())
            }
            Event::HardBreak => self.leaf(MKind::Break, bytes(start, end)),
            Event::Rule => self.leaf(
                MKind::ThematicBreak,
                bytes(start, trim_line_ending(source, start, end)),
            ),
            Event::Definition { label, dest, title } => {
                let mut end = end;
                while matches!(source.as_bytes().get(end), Some(b' ' | b'\t')) {
                    end += 1;
                }
                self.leaf(
                    MKind::Definition {
                        url: cow(dest),
                        title: title.map(cow),
                        identifier: normalize_identifier(&label),
                        label: rill_cmark::decode_string(&label),
                    },
                    bytes(start, end),
                )
            }
            Event::TaskListMarker(checked) => {
                let depth = self.stack.len();
                let item = (depth >= 2)
                    .then(|| (self.stack[depth - 2].ix, self.stack[depth - 1].ix))
                    .filter(|&(item, paragraph)| {
                        matches!(self.tree.nodes[paragraph as usize].kind, MKind::Paragraph)
                            && matches!(self.tree.nodes[item as usize].kind, MKind::ListItem { .. })
                            && self.tree.nodes[item as usize].first == paragraph
                    })
                    .map(|(item, _)| item);
                match item {
                    Some(item) => {
                        if let MKind::ListItem { checked: slot, .. } =
                            &mut self.tree.nodes[item as usize].kind
                        {
                            *slot = Some(checked);
                        }
                        self.task = Some((self.stack[depth - 1].ix, start, end));
                    }
                    None => {
                        // A setext heading swallowed the paragraph: the marker is plain text.
                        let mut end = end;
                        while matches!(source.as_bytes().get(end), Some(b' ' | b'\t')) {
                            end += 1;
                        }
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
            Tag::Paragraph => MKind::Paragraph,
            Tag::Heading(level) => MKind::Heading(level as u8),
            Tag::BlockQuote => MKind::Blockquote,
            Tag::CodeBlock(kind) => {
                let fenced = kind.is_fenced();
                let (lang, meta) = if fenced {
                    fence_info(source, start)
                } else {
                    (None, None)
                };
                MKind::Code {
                    value: Cow::Borrowed(""),
                    lang,
                    meta,
                    fenced,
                }
            }
            Tag::MathBlock(_) => {
                let info = fence_info_raw(source, start);
                MKind::Math {
                    value: Cow::Borrowed(""),
                    meta: (!info.is_empty()).then(|| rill_cmark::decode_string(info)),
                }
            }
            Tag::HtmlBlock => MKind::Html(Cow::Borrowed("")),
            Tag::List(first) => MKind::List {
                ordered: first.is_some(),
                start: first.map(|value| value as u32),
                spread: false,
            },
            Tag::Item => MKind::ListItem {
                spread: false,
                checked: None,
            },
            Tag::FootnoteDefinition(label) => MKind::FootnoteDefinition {
                identifier: normalize_identifier(&label),
                label: label.to_string(),
            },
            Tag::Table(align) => MKind::Table(
                align
                    .iter()
                    .map(|alignment| match alignment {
                        Alignment::None => AlignKind::None,
                        Alignment::Left => AlignKind::Left,
                        Alignment::Center => AlignKind::Center,
                        Alignment::Right => AlignKind::Right,
                    })
                    .collect(),
            ),
            Tag::TableHead | Tag::TableRow => MKind::TableRow,
            Tag::TableCell => MKind::TableCell,
            Tag::Emphasis => MKind::Emphasis,
            Tag::Strong => MKind::Strong,
            Tag::Strikethrough => MKind::Delete,
            Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            } => match reference_kind(link_type) {
                Some(kind) => MKind::LinkReference {
                    kind,
                    identifier: normalize_identifier(&id),
                    label: rill_cmark::decode_string(&id),
                },
                None => MKind::Link {
                    url: if link_type == LinkType::Email {
                        Cow::Owned(format!("mailto:{dest_url}"))
                    } else {
                        cow(dest_url)
                    },
                    title: (!title.is_empty()).then(|| cow(title)),
                },
            },
            Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            } => {
                self.alt = Some(String::new());
                match reference_kind(link_type) {
                    Some(kind) => MKind::ImageReference {
                        kind,
                        identifier: normalize_identifier(&id),
                        label: rill_cmark::decode_string(&id),
                        alt: String::new(),
                    },
                    None => MKind::Image {
                        url: cow(dest_url),
                        title: (!title.is_empty()).then(|| cow(title)),
                        alt: String::new(),
                    },
                }
            }
        };
        self.open(kind, start, end)
    }
}

fn bytes(start: usize, end: usize) -> At {
    At::Bytes(start as u32, end as u32)
}

fn reference_kind(link_type: LinkType) -> Option<ReferenceKind> {
    match link_type {
        LinkType::Reference => Some(ReferenceKind::Full),
        LinkType::Collapsed => Some(ReferenceKind::Collapsed),
        LinkType::Shortcut => Some(ReferenceKind::Shortcut),
        _ => None,
    }
}

/// Parses `source` (NUL-free; see [`without_nul`]) into an arena MDAST, including
/// mdast-util-gfm-autolink-literal's text transform.
pub fn parse_arena(source: &str) -> Result<MTree<'_>, Error> {
    if source.contains('\0') {
        return Err(Error::Nul);
    }
    let mut tree = MTree {
        source,
        nodes: Vec::with_capacity(source.len() / 8 + 16),
        points: Points::new(source),
    };
    tree.push(MKind::Root, bytes(0, source.len()));
    let mut builder = Builder {
        tree,
        stack: vec![Frame {
            ix: MTree::ROOT,
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
    };
    for (event, range) in Parser::new(source).into_offset_iter() {
        builder.event(event, range.start, range.end)?;
    }
    builder.flush_text()?;
    builder.settle_exits(None);
    if builder.stack.len() != 1 {
        return Err(Error::Unbalanced);
    }
    let mut tree = builder.tree;
    for (ix, start) in builder.starts {
        if let At::Bytes(_, end) = tree.nodes[ix as usize].at {
            tree.nodes[ix as usize].at = At::Points(start, tree.points.pt(end));
        }
    }
    autolink_transform(&mut tree);
    crossed_line_endings(&mut tree, MTree::ROOT, &mut Vec::new());
    Ok(tree)
}

/// micromark: a failed code text attempt (an unclosed `` ` `` or `$$` run) scans on to the
/// paragraph's end, and inside containers every line ending it crossed then ends past the
/// next line's container prefixes. Only a text or break node that ends with one, before
/// another node, shows it.
fn crossed_line_endings(tree: &mut MTree<'_>, ix: Ix, prefixes: &mut Vec<Prefix>) {
    let source = tree.source;
    let prefix = match tree.nodes[ix as usize].kind {
        MKind::Paragraph => {
            if !prefixes.is_empty() {
                crossed_in_paragraph(tree, ix, prefixes);
            }
            return;
        }
        MKind::Blockquote => Some(Prefix::Quote),
        MKind::ListItem { .. } => tree
            .bytes(ix)
            .map(|(start, _)| Prefix::Item(item_content_column(source, start as usize))),
        MKind::FootnoteDefinition { .. } => Some(Prefix::Item(4)),
        _ => None,
    };
    prefixes.extend(prefix);
    let mut cursor = tree.nodes[ix as usize].first;
    while cursor != NIL {
        crossed_line_endings(tree, cursor, prefixes);
        cursor = tree.nodes[cursor as usize].next;
    }
    if prefix.is_some() {
        prefixes.pop();
    }
}

fn crossed_in_paragraph(tree: &mut MTree<'_>, paragraph: Ix, prefixes: &[Prefix]) {
    let source = tree.source;
    let mut crossed = false;
    let mut cursor = tree.nodes[paragraph as usize].first;
    while cursor != NIL {
        let next = tree.nodes[cursor as usize].next;
        if let Some((start, end)) = tree.bytes(cursor) {
            let text = &source[start as usize..end as usize];
            let kind = &tree.nodes[cursor as usize].kind;
            if !matches!(kind, MKind::InlineCode(_) | MKind::InlineMath(_)) {
                crossed = crossed || failed_code_run(text);
            }
            if matches!(kind, MKind::Text(_) | MKind::Break)
                && crossed
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

/// Whether `text` (source) holds a `` ` `` or `$$` that no backslash escapes.
fn failed_code_run(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(at, &byte)| {
        let run = (byte == b'`' || byte == b'$' && bytes.get(at + 1) == Some(&b'$')).then(|| {
            bytes[..at]
                .iter()
                .rev()
                .take_while(|b| **b == b'\\')
                .count()
        });
        run.is_some_and(|backslashes| backslashes % 2 == 0)
    })
}

/// Parses `source` into remark's MDAST JSON.
pub fn parse_json(source: &str) -> Result<String, Error> {
    let source = without_nul(source);
    Ok(parse_arena(&source)?.to_json())
}

impl MTree<'_> {
    /// The tree as remark's MDAST JSON, positions in UTF-16 units.
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(self.source.len() * 6 + 64);
        self.write_json(&mut out, MTree::ROOT);
        out
    }

    fn write_json(&self, out: &mut String, ix: Ix) {
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
        let reference = |out: &mut String, kind: ReferenceKind| {
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
            MKind::Heading(depth) => {
                field(out, "depth");
                json_u32(out, u32::from(*depth));
            }
            MKind::List {
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
            MKind::ListItem { spread, checked } => {
                flag(out, "spread", *spread);
                field(out, "checked");
                out.push_str(match checked {
                    Some(true) => "true",
                    Some(false) => "false",
                    None => "null",
                });
            }
            MKind::Html(value)
            | MKind::Text(value)
            | MKind::InlineCode(value)
            | MKind::InlineMath(value) => {
                parent = false;
                string(out, "value", value);
            }
            MKind::Code {
                value, lang, meta, ..
            } => {
                parent = false;
                nullable(out, "lang", lang.as_deref());
                nullable(out, "meta", meta.as_deref());
                string(out, "value", value);
            }
            MKind::Math { value, meta } => {
                parent = false;
                nullable(out, "meta", meta.as_deref());
                string(out, "value", value);
            }
            MKind::Definition {
                url,
                title,
                identifier,
                label,
            } => {
                parent = false;
                string(out, "identifier", identifier);
                string(out, "label", label);
                nullable(out, "title", title.as_deref());
                string(out, "url", url);
            }
            MKind::Link { url, title } => {
                nullable(out, "title", title.as_deref());
                string(out, "url", url);
            }
            MKind::LinkReference {
                kind,
                identifier,
                label,
            } => {
                string(out, "identifier", identifier);
                string(out, "label", label);
                reference(out, *kind);
            }
            MKind::Image { url, title, alt } => {
                parent = false;
                string(out, "alt", alt);
                nullable(out, "title", title.as_deref());
                string(out, "url", url);
            }
            MKind::ImageReference {
                kind,
                identifier,
                label,
                alt,
            } => {
                parent = false;
                string(out, "alt", alt);
                string(out, "identifier", identifier);
                string(out, "label", label);
                reference(out, *kind);
            }
            MKind::FootnoteDefinition { identifier, label } => {
                string(out, "identifier", identifier);
                string(out, "label", label);
            }
            MKind::FootnoteReference { identifier, label } => {
                parent = false;
                string(out, "identifier", identifier);
                string(out, "label", label);
            }
            MKind::Table(align) => {
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
            MKind::ThematicBreak | MKind::Break => parent = false,
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
        if let Some((start, end)) = self.pt_range(node.at) {
            out.push_str(",\"position\":{\"start\":");
            write_point(out, start);
            out.push_str(",\"end\":");
            write_point(out, end);
            out.push('}');
        }
        out.push('}');
    }
}

fn write_point(out: &mut String, point: Pt) {
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

fn autolink_transform(tree: &mut MTree<'_>) {
    let finders: [Finder; 2] = [autolink::find_url, autolink::find_email];
    for finder in finders {
        transform_children(tree, MTree::ROOT, finder);
    }
}

/// mdast-util-find-and-replace: changed text nodes become new, unpositioned nodes.
fn transform_children(tree: &mut MTree<'_>, parent: Ix, finder: Finder) {
    let mut previous = NIL;
    let mut cursor = tree.nodes[parent as usize].first;
    while cursor != NIL {
        let next = tree.nodes[cursor as usize].next;
        match &tree.nodes[cursor as usize].kind {
            MKind::Text(value) => {
                if let Some(pieces) = autolink::replace(value, finder) {
                    let mut chain_first = NIL;
                    let mut chain_last = NIL;
                    for piece in pieces {
                        let ix = match piece {
                            Piece::Text(text) => tree.push(MKind::Text(Cow::Owned(text)), At::None),
                            Piece::Link { url, text } => {
                                let link = tree.push(
                                    MKind::Link {
                                        url: Cow::Owned(url),
                                        title: None,
                                    },
                                    At::None,
                                );
                                let child = tree.push(MKind::Text(Cow::Owned(text)), At::None);
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
                    tree.nodes[chain_last as usize].next = next;
                    if previous == NIL {
                        tree.nodes[parent as usize].first = chain_first;
                    } else {
                        tree.nodes[previous as usize].next = chain_first;
                    }
                    if next == NIL {
                        tree.nodes[parent as usize].last = chain_last;
                    }
                    previous = chain_last;
                    cursor = next;
                    continue;
                }
            }
            MKind::Link { .. } | MKind::LinkReference { .. } => {}
            _ => transform_children(tree, cursor, finder),
        }
        previous = cursor;
        cursor = next;
    }
}

// ---------------------------------------------------------------------------
// micromark position helpers
// ---------------------------------------------------------------------------

/// Whether a blank line separates two sibling blocks: only container prefixes and
/// whitespace occur between them, so two line endings mean a blank line.
fn blank_between(source: &str, end: usize, start: usize) -> bool {
    let between = source.as_bytes().get(end..start).unwrap_or_default();
    let mut endings = 0;
    let mut index = 0;
    while index < between.len() {
        match between[index] {
            b'\r' => {
                endings += 1;
                if between.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
            }
            b'\n' => endings += 1,
            _ => {}
        }
        index += 1;
    }
    endings >= 2
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
        let line_end = source[line_start..]
            .find(['\n', '\r'])
            .map_or(source.len(), |found| line_start + found);
        let mut column = 0;
        let mut cursor = line_start;
        while cursor < line_end && column < 4 && matches!(bytes[cursor], b' ' | b'\t') {
            column = if bytes[cursor] == b'\t' {
                column + 4 - column % 4
            } else {
                column + 1
            };
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
    let mut suffix = Cow::Owned(suffix);
    pop_line_ending(&mut suffix);
    (tail, suffix.into_owned())
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
    let line_start = source[..marker]
        .rfind(['\n', '\r'])
        .map_or(0, |eol| eol + 1);
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

/// Line endings in `text`, a CRLF counting once.
fn line_endings(text: &str) -> usize {
    text.matches('\n').count() + text.matches('\r').count() - text.matches("\r\n").count()
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
    let fence_line = source[..start].rfind(['\n', '\r']).map_or(0, |eol| eol + 1);
    let mut own = fence_line;
    for prefix in prefixes {
        match *prefix {
            Prefix::Quote => own = prefix_end(source, &[Prefix::Quote], own).0,
            Prefix::Item(column) => own = own.max(fence_line + column),
        }
    }
    let indent = start.saturating_sub(own);
    let mut value = String::new();
    let mut at = source[start..]
        .find(['\n', '\r'])
        .map_or(source.len(), |eol| start + eol);
    let mut last_empty = false;
    let mut lines = 0;
    while at < source.len() {
        let eol_end = at + line_ending_len(source, at);
        let line_end = source[eol_end..]
            .find(['\n', '\r'])
            .map_or(source.len(), |found| eol_end + found);
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
        let mut cow = Cow::Owned(value);
        pop_line_ending(&mut cow);
        value = cow.into_owned();
    }
    (at, value)
}

/// The line that closed a node pulldown ends at `closing`: at that line's start, past its
/// prefixes, or at the line ending before it. Returns the line's start and end.
fn closing_line(source: &str, closing: usize) -> (usize, usize) {
    let line_start = if matches!(source.as_bytes().get(closing), Some(b'\n' | b'\r')) {
        closing + line_ending_len(source, closing)
    } else {
        source[..closing.min(source.len())]
            .rfind(['\n', '\r'])
            .map_or(0, |eol| eol + 1)
    };
    let line_end = source[line_start..]
        .find(['\n', '\r'])
        .map_or(source.len(), |at| line_start + at);
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
        let line_start = source[..at].rfind(['\n', '\r']).map_or(0, |eol| eol + 1);
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
    let line_start = source[..start].rfind(['\n', '\r']).map_or(0, |at| at + 1);
    let column_at = |end: usize| {
        let mut column = 0usize;
        for &byte in &bytes[line_start..end] {
            column = if byte == b'\t' {
                column + 4 - column % 4
            } else {
                column + 1
            };
        }
        column
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
    while matches!(bytes.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    let end = source[at..]
        .find(['\n', '\r'])
        .map_or(source.len(), |found| at + found);
    &source[at..end]
}

/// micromark splits fenced-code info at the first space or tab; meta keeps its tail.
fn fence_info(source: &str, start: usize) -> (Option<String>, Option<String>) {
    let info = fence_info_raw(source, start);
    if info.is_empty() {
        return (None, None);
    }
    match info.find([' ', '\t']) {
        Some(split) => {
            let lang = &info[..split];
            let meta = info[split..].trim_start_matches([' ', '\t']);
            (
                Some(rill_cmark::decode_string(lang)),
                (!meta.is_empty()).then(|| rill_cmark::decode_string(meta)),
            )
        }
        None => (Some(rill_cmark::decode_string(info)), None),
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
    let indent = start - source[..start].rfind(['\n', '\r']).map_or(0, |eol| eol + 1);
    let body_start = source[start..]
        .find(['\n', '\r'])
        .map_or(source.len(), |at| start + at);
    let body = &source[body_start..];
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix(['\n', '\r']))
        .unwrap_or(body);
    let body = body
        .strip_suffix("\r\n")
        .or_else(|| body.strip_suffix(['\n', '\r']))
        .unwrap_or(body);
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
                let mut out = Cow::Owned(out);
                pop_line_ending(&mut out);
                return out;
            }
            out.push_str(rest);
            return Cow::Owned(out);
        };
        let next = eol + line_ending_len(rest, eol);
        out.push_str(&rest[..next]);
        rest = &rest[next..];
    }
}

/// Drops one terminal line ending (`\n`, `\r\n` or `\r`).
fn pop_line_ending(value: &mut Cow<'_, str>) {
    let mut length = value.len();
    if value.ends_with('\n') {
        length -= 1;
    }
    if value[..length].ends_with('\r') {
        length -= 1;
    }
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
    let block = &source[start..end];
    let lower = block
        .get(..block.len().min(12))
        .unwrap_or(block)
        .to_ascii_lowercase();
    let terminator = if ["<script", "<pre", "<style", "<textarea"]
        .iter()
        .any(|open| {
            lower.starts_with(open)
                && lower[open.len()..]
                    .bytes()
                    .next()
                    .is_none_or(|byte| matches!(byte, b' ' | b'\t' | b'>' | b'\n' | b'\r'))
        }) {
        None
    } else if lower.starts_with("<!--") {
        Some("-->")
    } else if lower.starts_with("<?") {
        Some("?>")
    } else if lower.starts_with("<![cdata[") {
        Some("]]>")
    } else if lower.starts_with("<!")
        && lower.as_bytes().get(2).is_some_and(u8::is_ascii_alphabetic)
    {
        Some(">")
    } else {
        return false;
    };
    let body = block.to_ascii_lowercase();
    match terminator {
        None => !["</script>", "</pre>", "</style>", "</textarea>"]
            .iter()
            .any(|close| body.contains(close)),
        Some(close) => !body[2..].contains(close),
    }
}

/// Code and HTML blocks end on their last non-blank line, keeping its trailing spaces.
/// `trim_blank_lines` past up to `depth` quote markers: a line that only continues the
/// enclosing quotes is blank (concrete blocks open no containers, so more `>` is content).
fn trim_quoted_blank_lines(source: &str, start: usize, mut end: usize, depth: usize) -> usize {
    loop {
        let line_start = source[..end].rfind(['\n', '\r']).map_or(0, |eol| eol + 1);
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
