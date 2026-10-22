// Copyright 2015 Google Inc. All rights reserved.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

//! rill-cmark: a fork of [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark)
//! 0.13.4 whose grammar follows micromark as remark configures it for Streamdown
//! (remark-gfm, remark-math with `singleDollarTextMath: false`, remark-cjk-friendly).
//! Changes from upstream are marked `rill:`. The `rill` crate builds MDAST and HAST from
//! its events; use it rather than this crate directly.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod utils;

mod attention;
pub mod autolink_literal;
mod entities;
mod firstpass;
mod linklabel;
mod parse;
mod scanners;
mod strings;
mod tree;
mod unicode_tables;

use std::fmt::Display;

pub use crate::parse::{
    BrokenLink, BrokenLinkCallback, DefaultBrokenLinkCallback, OffsetIter, Parser, RefDefs,
};
pub use crate::scanners::decode_string;
pub use crate::strings::{CowStr, InlineStr};
pub use crate::utils::*;

/// Codeblock kind.
#[derive(Clone, Debug, PartialEq)]
pub enum CodeBlockKind<'a> {
    Indented,
    /// The value contained in the tag describes the language of the code, which may be empty.
    Fenced(CowStr<'a>),
}

impl<'a> CodeBlockKind<'a> {
    pub fn is_indented(&self) -> bool {
        matches!(*self, CodeBlockKind::Indented)
    }

    pub fn is_fenced(&self) -> bool {
        matches!(*self, CodeBlockKind::Fenced(_))
    }

    pub fn into_static(self) -> CodeBlockKind<'static> {
        match self {
            CodeBlockKind::Indented => CodeBlockKind::Indented,
            CodeBlockKind::Fenced(s) => CodeBlockKind::Fenced(s.into_static()),
        }
    }
}

/// BlockQuote kind (Note, Tip, Important, Warning, Caution).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlockQuoteKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

/// Metadata block kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MetadataBlockKind {
    YamlStyle,
    PlusesStyle,
}

/// Tags for elements that can contain other elements.
#[derive(Clone, Debug, PartialEq)]
pub enum Tag<'a> {
    /// A paragraph of text and other inline elements.
    Paragraph,

    /// A heading, with optional identifier, classes and custom attributes.
    /// The identifier is prefixed with `#` and the last one in the attributes
    /// list is chosen, classes are prefixed with `.` and custom attributes
    /// have no prefix and can optionally have a value (`myattr` or `myattr=myvalue`).
    Heading {
        level: HeadingLevel,
        id: Option<CowStr<'a>>,
        classes: Vec<CowStr<'a>>,
        /// The first item of the tuple is the attr and second one the value.
        attrs: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    },

    /// A block quote.
    ///
    ///
    /// ```markdown
    /// > regular quote
    ///
    /// > [!NOTE]
    /// > note quote
    /// ```
    BlockQuote(Option<BlockQuoteKind>),
    /// A code block.
    CodeBlock(CodeBlockKind<'a>),

    /// An HTML block.
    ///
    /// A line that begins with some predefined tags (HTML block tags) (see [CommonMark Spec](https://spec.commonmark.org/0.31.2/#html-blocks) for more details) or any tag that is followed only by whitespace.
    ///
    /// Most HTML blocks end on an empty line, though some e.g. `<pre>` like `<script>` or `<!-- Comments -->` don't.
    /// ```markdown
    /// <body> Is HTML block even though here is non-whitespace.
    /// Block ends on an empty line.
    ///
    /// <some-random-tag>
    /// This is HTML block.
    ///
    /// <pre> Doesn't end on empty lines.
    ///
    /// This is still the same block.</pre>
    /// ```
    HtmlBlock,

    /// A list. If the list is ordered the field indicates the number of the first item.
    /// Contains only list items.
    List(Option<u64>), // TODO: add delim and tight for ast (not needed for html)
    /// A list item.
    Item,
    /// A footnote definition. The value contained is the footnote's label by which it can
    /// be referred to.
    ///
    FootnoteDefinition(CowStr<'a>),

    DefinitionList,
    DefinitionListTitle,
    DefinitionListDefinition,

    /// A table. Contains a vector describing the text-alignment for each of its columns.
    Table(Vec<Alignment>),
    /// A table header. Contains only `TableCell`s. Note that the table body starts immediately
    /// after the closure of the `TableHead` tag. There is no `TableBody` tag.
    TableHead,
    /// A table row. Is used both for header rows as body rows. Contains only `TableCell`s.
    TableRow,
    TableCell,

    // span-level tags
    /// [Emphasis](https://spec.commonmark.org/0.31.2/#emphasis-and-strong-emphasis).
    /// ```markdown
    /// half*emph* _strong_ _multi _level__
    /// ```
    Emphasis,
    /// [Strong emphasis](https://spec.commonmark.org/0.31.2/#emphasis-and-strong-emphasis).
    /// ```markdown
    /// half**strong** __strong__ __multi __level____
    /// ```
    Strong,
    ///
    /// ```markdown
    /// ~strike through~
    /// ```
    Strikethrough,
    ///
    /// ```markdown
    /// ^superscript^
    /// ```
    Superscript,
    /// ```markdown
    /// ~subscript~ ~~if also enabled this is strikethrough~~
    /// ```
    Subscript,

    /// A link.
    Link {
        link_type: LinkType,
        dest_url: CowStr<'a>,
        title: CowStr<'a>,
        /// Identifier of reference links, e.g. `world` in the link `[hello][world]`.
        id: CowStr<'a>,
    },

    /// An image. The first field is the link type, the second the destination URL and the third is a title,
    /// the fourth is the link identifier.
    Image {
        link_type: LinkType,
        dest_url: CowStr<'a>,
        title: CowStr<'a>,
        /// Identifier of reference links, e.g. `world` in the link `[hello][world]`.
        id: CowStr<'a>,
    },

    /// A metadata block.
    MetadataBlock(MetadataBlockKind),

    /// rill: a `$$` math flow block with its meta string.
    MathBlock(CowStr<'a>),
}

impl<'a> Tag<'a> {
    pub fn to_end(&self) -> TagEnd {
        match self {
            Tag::Paragraph => TagEnd::Paragraph,
            Tag::Heading { level, .. } => TagEnd::Heading(*level),
            Tag::BlockQuote(kind) => TagEnd::BlockQuote(*kind),
            Tag::CodeBlock(_) => TagEnd::CodeBlock,
            Tag::HtmlBlock => TagEnd::HtmlBlock,
            Tag::List(number) => TagEnd::List(number.is_some()),
            Tag::Item => TagEnd::Item,
            Tag::FootnoteDefinition(_) => TagEnd::FootnoteDefinition,
            Tag::Table(_) => TagEnd::Table,
            Tag::TableHead => TagEnd::TableHead,
            Tag::TableRow => TagEnd::TableRow,
            Tag::TableCell => TagEnd::TableCell,
            Tag::Subscript => TagEnd::Subscript,
            Tag::Superscript => TagEnd::Superscript,
            Tag::Emphasis => TagEnd::Emphasis,
            Tag::Strong => TagEnd::Strong,
            Tag::Strikethrough => TagEnd::Strikethrough,
            Tag::Link { .. } => TagEnd::Link,
            Tag::Image { .. } => TagEnd::Image,
            Tag::MetadataBlock(kind) => TagEnd::MetadataBlock(*kind),
            Tag::MathBlock(_) => TagEnd::MathBlock,
            Tag::DefinitionList => TagEnd::DefinitionList,
            Tag::DefinitionListTitle => TagEnd::DefinitionListTitle,
            Tag::DefinitionListDefinition => TagEnd::DefinitionListDefinition,
        }
    }

    pub fn into_static(self) -> Tag<'static> {
        match self {
            Tag::Paragraph => Tag::Paragraph,
            Tag::Heading {
                level,
                id,
                classes,
                attrs,
            } => Tag::Heading {
                level,
                id: id.map(|s| s.into_static()),
                classes: classes.into_iter().map(|s| s.into_static()).collect(),
                attrs: attrs
                    .into_iter()
                    .map(|(k, v)| (k.into_static(), v.map(|s| s.into_static())))
                    .collect(),
            },
            Tag::BlockQuote(k) => Tag::BlockQuote(k),
            Tag::CodeBlock(kb) => Tag::CodeBlock(kb.into_static()),
            Tag::HtmlBlock => Tag::HtmlBlock,
            Tag::List(v) => Tag::List(v),
            Tag::Item => Tag::Item,
            Tag::FootnoteDefinition(a) => Tag::FootnoteDefinition(a.into_static()),
            Tag::Table(v) => Tag::Table(v),
            Tag::TableHead => Tag::TableHead,
            Tag::TableRow => Tag::TableRow,
            Tag::TableCell => Tag::TableCell,
            Tag::Emphasis => Tag::Emphasis,
            Tag::Strong => Tag::Strong,
            Tag::Strikethrough => Tag::Strikethrough,
            Tag::Superscript => Tag::Superscript,
            Tag::Subscript => Tag::Subscript,
            Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            } => Tag::Link {
                link_type,
                dest_url: dest_url.into_static(),
                title: title.into_static(),
                id: id.into_static(),
            },
            Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            } => Tag::Image {
                link_type,
                dest_url: dest_url.into_static(),
                title: title.into_static(),
                id: id.into_static(),
            },
            Tag::MetadataBlock(v) => Tag::MetadataBlock(v),
            Tag::MathBlock(meta) => Tag::MathBlock(meta.into_static()),
            Tag::DefinitionList => Tag::DefinitionList,
            Tag::DefinitionListTitle => Tag::DefinitionListTitle,
            Tag::DefinitionListDefinition => Tag::DefinitionListDefinition,
        }
    }
}

/// The end of a `Tag`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum TagEnd {
    Paragraph,
    Heading(HeadingLevel),

    BlockQuote(Option<BlockQuoteKind>),
    CodeBlock,

    HtmlBlock,

    /// A list, `true` for ordered lists.
    List(bool),
    Item,
    FootnoteDefinition,

    DefinitionList,
    DefinitionListTitle,
    DefinitionListDefinition,

    Table,
    TableHead,
    TableRow,
    TableCell,

    Emphasis,
    Strong,
    Strikethrough,
    Superscript,
    Subscript,

    Link,
    Image,

    MetadataBlock(MetadataBlockKind),
    MathBlock,
}

/// Make sure `TagEnd` is no more than two bytes in size.
/// This is why it's used instead of just using `Tag`.
#[cfg(target_pointer_width = "64")]
const _STATIC_ASSERT_TAG_END_SIZE: [(); 2] = [(); std::mem::size_of::<TagEnd>()];

impl<'a> From<Tag<'a>> for TagEnd {
    fn from(value: Tag) -> Self {
        value.to_end()
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum HeadingLevel {
    H1 = 1,
    H2,
    H3,
    H4,
    H5,
    H6,
}

impl Display for HeadingLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::H1 => write!(f, "h1"),
            Self::H2 => write!(f, "h2"),
            Self::H3 => write!(f, "h3"),
            Self::H4 => write!(f, "h4"),
            Self::H5 => write!(f, "h5"),
            Self::H6 => write!(f, "h6"),
        }
    }
}

/// Returned when trying to convert a `usize` into a `Heading` but it fails
/// because the usize isn't a valid heading level
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct InvalidHeadingLevel(usize);

impl TryFrom<usize> for HeadingLevel {
    type Error = InvalidHeadingLevel;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::H1),
            2 => Ok(Self::H2),
            3 => Ok(Self::H3),
            4 => Ok(Self::H4),
            5 => Ok(Self::H5),
            6 => Ok(Self::H6),
            _ => Err(InvalidHeadingLevel(value)),
        }
    }
}

/// Type specifier for inline links. See [the Tag::Link](enum.Tag.html#variant.Link) for more information.
#[derive(Clone, Debug, PartialEq, Copy)]
pub enum LinkType {
    /// Inline link like `[foo](bar)`
    Inline,
    /// Reference link like `[foo][bar]`
    Reference,
    /// Reference without destination in the document, but resolved by the broken_link_callback
    ReferenceUnknown,
    /// Collapsed link like `[foo][]`
    Collapsed,
    /// Collapsed link without destination in the document, but resolved by the broken_link_callback
    CollapsedUnknown,
    /// Shortcut link like `[foo]`
    Shortcut,
    /// Shortcut without destination in the document, but resolved by the broken_link_callback
    ShortcutUnknown,
    /// Autolink like `<http://foo.bar/baz>`
    Autolink,
    /// Email address in autolink like `<john@example.org>`
    Email,
    /// Wikilink link like `[[foo]]` or `[[foo|bar]]`
    WikiLink {
        /// `true` if the wikilink was piped.
        ///
        /// * `true` - `[[foo|bar]]`
        /// * `false` - `[[foo]]`
        has_pothole: bool,
    },
}

impl LinkType {
    /// Map the link type to an equivalent _Unknown link type.
    fn to_unknown(self) -> Self {
        match self {
            LinkType::Reference => LinkType::ReferenceUnknown,
            LinkType::Collapsed => LinkType::CollapsedUnknown,
            LinkType::Shortcut => LinkType::ShortcutUnknown,
            _ => unreachable!(),
        }
    }
}

/// Markdown events that are generated in a preorder traversal of the document
/// tree, with additional `End` events whenever all of an inner node's children
/// have been visited.
#[derive(Clone, Debug, PartialEq)]
pub enum Event<'a> {
    /// Start of a tagged element. Events that are yielded after this event
    /// and before its corresponding `End` event are inside this element.
    /// Start and end events are guaranteed to be balanced.
    Start(Tag<'a>),
    /// End of a tagged element.
    End(TagEnd),
    /// A text node.
    ///
    /// All text, outside and inside [`Tag`]s.
    Text(CowStr<'a>),
    /// An [inline code node](https://spec.commonmark.org/0.31.2/#code-spans).
    ///
    /// ```markdown
    /// `code`
    /// ```
    Code(CowStr<'a>),
    /// An inline math environment node.
    ///
    /// ```markdown
    /// $math$
    /// ```
    InlineMath(CowStr<'a>),
    /// A display math environment node.
    ///
    /// ```markdown
    /// $$math$$
    /// ```
    DisplayMath(CowStr<'a>),
    /// An HTML node.
    ///
    /// A line of HTML inside [`Tag::HtmlBlock`] includes the line break.
    Html(CowStr<'a>),
    /// An [inline HTML node](https://spec.commonmark.org/0.31.2/#raw-html).
    ///
    /// Contains only the tag itself, e.g. `<open-tag>`, `</close-tag>` or `<!-- comment -->`.
    ///
    /// **Note**: Under some conditions HTML can also be parsed as an HTML Block, see [`Tag::HtmlBlock`] for details.
    InlineHtml(CowStr<'a>),
    /// A reference to a footnote with given label, which may or may not be defined
    /// by an event with a [`Tag::FootnoteDefinition`] tag. Definitions and references to them may
    /// occur in any order.
    ///
    /// ```markdown
    /// [^1]
    /// ```
    FootnoteReference(CowStr<'a>),
    /// A [soft line break](https://spec.commonmark.org/0.31.2/#soft-line-breaks).
    ///
    /// Any line break that isn't a [`HardBreak`](Self::HardBreak), or the end of e.g. a paragraph.
    SoftBreak,
    /// A [hard line break](https://spec.commonmark.org/0.31.2/#hard-line-breaks).
    ///
    /// A line ending that is either preceded by at least two spaces or `\`.
    ///
    /// ```markdown
    /// hard··
    /// line\
    /// breaks
    /// ```
    /// *`·` is a space*
    HardBreak,
    /// A horizontal ruler.
    ///
    /// ```markdown
    /// ***
    /// ···---
    /// _·_··_····_··
    /// ```
    /// *`·` is any whitespace*
    Rule,
    /// A task list marker, rendered as a checkbox in HTML. Contains a true when it is checked.
    /// ```markdown
    /// - [ ] unchecked
    /// - [x] checked
    /// ```
    TaskListMarker(bool),
    /// rill: a link reference definition with its raw label source.
    Definition {
        label: CowStr<'a>,
        dest: CowStr<'a>,
        title: Option<CowStr<'a>>,
    },
}

impl<'a> Event<'a> {
    pub fn into_static(self) -> Event<'static> {
        match self {
            Event::Start(t) => Event::Start(t.into_static()),
            Event::End(e) => Event::End(e),
            Event::Text(s) => Event::Text(s.into_static()),
            Event::Code(s) => Event::Code(s.into_static()),
            Event::InlineMath(s) => Event::InlineMath(s.into_static()),
            Event::DisplayMath(s) => Event::DisplayMath(s.into_static()),
            Event::Html(s) => Event::Html(s.into_static()),
            Event::InlineHtml(s) => Event::InlineHtml(s.into_static()),
            Event::FootnoteReference(s) => Event::FootnoteReference(s.into_static()),
            Event::SoftBreak => Event::SoftBreak,
            Event::HardBreak => Event::HardBreak,
            Event::Rule => Event::Rule,
            Event::TaskListMarker(b) => Event::TaskListMarker(b),
            Event::Definition { label, dest, title } => Event::Definition {
                label: label.into_static(),
                dest: dest.into_static(),
                title: title.map(CowStr::into_static),
            },
        }
    }
}

/// Table column text alignment.
#[derive(Copy, Clone, Debug, PartialEq)]

pub enum Alignment {
    /// Default text alignment.
    None,
    Left,
    Center,
    Right,
}
