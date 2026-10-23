// Copyright 2017 Google Inc. All rights reserved.
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

//! Tree-based two pass parser.

use std::cmp::{max, min};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::{Index, Range};

use crate::firstpass::run_first_pass;
use crate::linklabel::{
    normalize_label, scan_link_label_rest, FootnoteLabel, LinkLabel, ReferenceLabel,
};
use crate::scanners::*;
use crate::strings::CowStr;
use crate::tree::{Tree, TreeIndex};
use crate::{Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Syntax, Tag, TagEnd};

// Allowing arbitrary depth nested parentheses inside link destinations
// can create denial of service vulnerabilities if we're not careful.
// The simplest countermeasure is to limit their depth, which is
// explicitly allowed by the spec as long as the limit is at least 3:
// https://spec.commonmark.org/0.29/#link-destination
pub(crate) const LINK_MAX_NESTED_PARENS: usize = 32;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Item {
    pub start: usize,
    pub end: usize,
    pub body: ItemBody,
}

#[derive(Debug, PartialEq, Clone, Copy, Default)]
pub(crate) enum ItemBody {
    // These are possible inline items, need to be resolved in second pass.

    // repeats, can_open, can_close
    MaybeEmphasis(usize, bool, bool),
    MaybeCode(usize, bool),     // number of backticks, preceded by backslash
    MaybeMathText(usize, bool), // rill: number of dollars, preceded by backslash
    MaybeHtml,
    MaybeLinkOpen,
    // bool indicates whether or not the preceding section could be a reference
    MaybeLinkClose(bool),
    MaybeImage,

    // These are inline items after resolution.
    Emphasis,
    Strong,
    Strikethrough,
    Math(CowIndex),
    Code(CowIndex),
    Link(LinkIndex),
    Image(LinkIndex),
    FootnoteReference(CowIndex),
    TaskListMarker(bool), // true for checked

    // These are also inline items.
    InlineHtml,
    OwnedInlineHtml(CowIndex),
    SynthesizeText(CowIndex),
    SynthesizeChar(char),
    Html,
    Text {
        backslash_escaped: bool,
    },
    SoftBreak,
    // true = is backlash
    HardBreak(bool),

    // Dummy node at the top of the tree - should not be used otherwise!
    #[default]
    Root,

    // These are block items.
    Paragraph,
    TightParagraph,
    Rule,
    Heading(HeadingLevel),
    FencedCodeBlock(CowIndex),
    // rill: `$$` math flow; the index is its meta string.
    MathBlock(CowIndex),
    IndentCodeBlock,
    HtmlBlock,
    BlockQuote,
    List(bool, u8, u64), // is_tight, list character, list start index
    ListItem(usize),     // indent level
    FootnoteDefinition(CowIndex),

    // Tables
    Table(AlignmentIndex),
    TableHead,
    TableRow,
    TableCell,

    // rill: link reference definition, index into `Allocations::definitions`.
    Definition(usize),
}

impl ItemBody {
    fn is_maybe_inline(&self) -> bool {
        use ItemBody::*;
        matches!(
            *self,
            MaybeEmphasis(..)
                | MaybeCode(..)
                | MaybeMathText(..)
                | MaybeHtml
                | MaybeLinkOpen
                | MaybeLinkClose(..)
                | MaybeImage
        )
    }
    fn is_inline(&self) -> bool {
        use ItemBody::*;
        matches!(
            *self,
            MaybeEmphasis(..)
                | MaybeCode(..)
                | MaybeMathText(..)
                | MaybeHtml
                | MaybeLinkOpen
                | MaybeLinkClose(..)
                | MaybeImage
                | Emphasis
                | Strong
                | Strikethrough
                | Math(..)
                | Code(..)
                | Link(..)
                | Image(..)
                | FootnoteReference(..)
                | TaskListMarker(..)
                | InlineHtml
                | OwnedInlineHtml(..)
                | SynthesizeText(..)
                | SynthesizeChar(..)
                | Html
                | Text { .. }
                | SoftBreak
                | HardBreak(..)
        )
    }
}

/// Markdown event iterator.
pub struct Parser<'input> {
    text: &'input str,
    tree: Tree<Item>,
    allocs: Allocations<'input>,
    html_scan_guard: HtmlScanGuard,

    // https://github.com/pulldown-cmark/pulldown-cmark/issues/844
    // Consider this example:
    //
    //     [x]: xxx...
    //     [x]
    //     [x]
    //     [x]
    //
    // Which expands to this HTML:
    //
    //     <a href="xxx...">x</a>
    //     <a href="xxx...">x</a>
    //     <a href="xxx...">x</a>
    //
    // This is quadratic growth, because it's filling in the area of a square.
    // To prevent this, track how much it's expanded and limit it.
    link_ref_expansion_limit: usize,

    // used by inline passes. store them here for reuse
    inline_stack: InlineStack,
    link_stack: LinkStack,
    code_delims: CodeDelims,
    math_text_delims: CodeDelims,
}

impl<'input> std::fmt::Debug for Parser<'input> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only print the fields that have public types.
        f.debug_struct("Parser").field("text", &self.text).finish()
    }
}

impl<'input> Parser<'input> {
    /// Creates a new event iterator for a markdown string, with Streamdown's default
    /// grammar.
    pub fn new(text: &'input str) -> Self {
        Self::with_syntax(text, Syntax::default())
    }

    /// Creates a new event iterator for a markdown string, with the extensions `syntax`
    /// turns on.
    pub fn with_syntax(text: &'input str, syntax: Syntax) -> Self {
        let (mut tree, allocs) = run_first_pass(text, syntax);
        tree.reset();
        let inline_stack = Default::default();
        let link_stack = Default::default();
        let html_scan_guard = Default::default();
        Parser {
            text,
            tree,
            allocs,
            inline_stack,
            link_stack,
            html_scan_guard,
            // always allow 100KiB
            link_ref_expansion_limit: text.len().max(100_000),
            code_delims: CodeDelims::new(),
            math_text_delims: CodeDelims::new(),
        }
    }

    /// Use a link label to fetch a url and title.
    ///
    /// This function enforces the [`link_ref_expansion_limit`].
    /// If it returns Some, it also consumes some of the fuel.
    /// If we're out of fuel, it immediately returns None.
    ///
    /// The URL and title are found in the [`RefDefs`] map.
    ///
    /// [`link_ref_expansion_limit`]: Self::link_ref_expansion_limit
    fn fetch_link_url_title(
        &mut self,
        link_label: CowStr<'input>,
    ) -> Option<(CowStr<'input>, CowStr<'input>)> {
        if self.link_ref_expansion_limit == 0 {
            return None;
        }

        let (url, title) = self
            .allocs
            .refdefs
            .get(link_label.as_ref())
            .map(|matching_def| {
                // found a matching definition!
                let title = matching_def
                    .title
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| "".into());
                let url = matching_def.dest.clone();
                (url, title)
            })?;

        // Limit expansion from link references.
        // This isn't a problem for footnotes, because multiple references to the same one
        // reuse the same node, but links/images get their HREF/SRC copied.
        self.link_ref_expansion_limit = self
            .link_ref_expansion_limit
            .saturating_sub(url.len() + title.len());

        Some((url, title))
    }

    /// Handle inline markup.
    ///
    /// When the parser encounters any item indicating potential inline markup, all
    /// inline markup passes are run on the remainder of the chain.
    ///
    /// Note: there's some potential for optimization here, but that's future work.
    fn handle_inline(&mut self) {
        self.handle_inline_pass1();
        self.handle_emphasis_and_hard_break();
    }

    /// Handle inline HTML, code spans, and links.
    ///
    /// This function handles both inline HTML and code spans, because they have
    /// the same precedence. It also handles links, even though they have lower
    /// precedence, because the URL of links must not be processed.
    fn handle_inline_pass1(&mut self) {
        let mut cur = self.tree.cur();
        let mut prev = None;

        let block_end = self.tree[self.tree.peek_up().unwrap()].item.end;
        let block_text = &self.text[..block_end];
        let block_start = cur.map_or(block_end, |ix| self.tree[ix].item.start);
        // rill: literals are found once, in the block's own pass, where an open `[` blocks
        // them (micromark's `previousUnbalanced`); spans and links built from that pass
        // are not rescanned.
        let autolinks = !self.tree[self.tree.peek_up().unwrap()]
            .item
            .body
            .is_inline()
            && crate::autolink_literal::may_contain(&block_text[block_start.min(block_end)..]);

        while let Some(mut cur_ix) = cur {
            match self.tree[cur_ix].item.body {
                ItemBody::MaybeHtml => {
                    let next = self.tree[cur_ix].next;
                    let autolink = if let Some(next_ix) = next {
                        scan_autolink(block_text, self.tree[next_ix].item.start)
                    } else {
                        None
                    };

                    if let Some((ix, uri, link_type)) = autolink {
                        let node = scan_nodes_to_ix(&self.tree, next, ix);
                        let text_node = self.tree.create_node(Item {
                            start: self.tree[cur_ix].item.start + 1,
                            end: ix - 1,
                            body: ItemBody::Text {
                                backslash_escaped: false,
                            },
                        });
                        let link_ix =
                            self.allocs
                                .allocate_link(link_type, uri, "".into(), "".into());
                        self.tree[cur_ix].item.body = ItemBody::Link(link_ix);
                        self.tree[cur_ix].item.end = ix;
                        self.tree[cur_ix].next = node;
                        self.tree[cur_ix].child = Some(text_node);
                        prev = cur;
                        cur = node;
                        if let Some(node_ix) = cur {
                            self.tree[node_ix].item.start = max(self.tree[node_ix].item.start, ix);
                        }
                        continue;
                    } else {
                        let inline_html = next.and_then(|next_ix| {
                            self.scan_inline_html(
                                block_text.as_bytes(),
                                self.tree[next_ix].item.start,
                            )
                        });
                        if let Some((span, ix)) = inline_html {
                            let node = scan_nodes_to_ix(&self.tree, next, ix);
                            self.tree[cur_ix].item.body = if !span.is_empty() {
                                let converted_string =
                                    String::from_utf8(span).expect("invalid utf8");
                                ItemBody::OwnedInlineHtml(
                                    self.allocs.allocate_cow(converted_string.into()),
                                )
                            } else {
                                ItemBody::InlineHtml
                            };
                            self.tree[cur_ix].item.end = ix;
                            self.tree[cur_ix].next = node;
                            prev = cur;
                            cur = node;
                            if let Some(node_ix) = cur {
                                self.tree[node_ix].item.start =
                                    max(self.tree[node_ix].item.start, ix);
                            }
                            continue;
                        }
                    }
                    self.tree[cur_ix].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                }
                ItemBody::MaybeCode(mut search_count, preceded_by_backslash) => {
                    if preceded_by_backslash {
                        search_count -= 1;
                        if search_count == 0 {
                            self.tree[cur_ix].item.body = ItemBody::Text {
                                backslash_escaped: false,
                            };
                            prev = cur;
                            cur = self.tree[cur_ix].next;
                            continue;
                        }
                    }

                    if self.code_delims.is_populated() {
                        // we have previously scanned all codeblock delimiters,
                        // so we can reuse that work
                        if let Some(scan_ix) = self.code_delims.find(cur_ix, search_count) {
                            self.make_code_span(cur_ix, scan_ix, preceded_by_backslash, false);
                        } else {
                            self.tree[cur_ix].item.body = ItemBody::Text {
                                backslash_escaped: false,
                            };
                        }
                    } else {
                        // we haven't previously scanned all codeblock delimiters,
                        // so walk the AST
                        let mut scan = if search_count > 0 {
                            self.tree[cur_ix].next
                        } else {
                            None
                        };
                        while let Some(scan_ix) = scan {
                            if let ItemBody::MaybeCode(delim_count, _) =
                                self.tree[scan_ix].item.body
                            {
                                if search_count == delim_count {
                                    self.make_code_span(
                                        cur_ix,
                                        scan_ix,
                                        preceded_by_backslash,
                                        false,
                                    );
                                    self.code_delims.clear();
                                    break;
                                } else {
                                    self.code_delims.insert(delim_count, scan_ix);
                                }
                            }
                            scan = self.tree[scan_ix].next;
                        }
                        if scan.is_none() {
                            self.tree[cur_ix].item.body = ItemBody::Text {
                                backslash_escaped: false,
                            };
                        }
                    }
                }
                ItemBody::MaybeMathText(mut search_count, preceded_by_backslash) => {
                    if preceded_by_backslash {
                        search_count -= 1;
                        if search_count == 0 {
                            self.tree[cur_ix].item.body = ItemBody::Text {
                                backslash_escaped: false,
                            };
                            prev = cur;
                            cur = self.tree[cur_ix].next;
                            continue;
                        }
                    }

                    if self.math_text_delims.is_populated() {
                        // we have previously scanned all codeblock delimiters,
                        // so we can reuse that work
                        if let Some(scan_ix) = self.math_text_delims.find(cur_ix, search_count) {
                            self.make_code_span(cur_ix, scan_ix, preceded_by_backslash, true);
                        } else {
                            self.tree[cur_ix].item.body = ItemBody::Text {
                                backslash_escaped: false,
                            };
                        }
                    } else {
                        // we haven't previously scanned all codeblock delimiters,
                        // so walk the AST
                        let mut scan = if search_count > 0 {
                            self.tree[cur_ix].next
                        } else {
                            None
                        };
                        while let Some(scan_ix) = scan {
                            if let ItemBody::MaybeMathText(delim_count, _) =
                                self.tree[scan_ix].item.body
                            {
                                if search_count == delim_count {
                                    self.make_code_span(
                                        cur_ix,
                                        scan_ix,
                                        preceded_by_backslash,
                                        true,
                                    );
                                    self.math_text_delims.clear();
                                    break;
                                } else {
                                    self.math_text_delims.insert(delim_count, scan_ix);
                                }
                            }
                            scan = self.tree[scan_ix].next;
                        }
                        if scan.is_none() {
                            self.tree[cur_ix].item.body = ItemBody::Text {
                                backslash_escaped: false,
                            };
                        }
                    }
                }
                ItemBody::MaybeLinkOpen => {
                    self.tree[cur_ix].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                    self.link_stack.push(LinkStackEl {
                        node: cur_ix,
                        ty: LinkStackTy::Link,
                    });
                }
                ItemBody::MaybeImage => {
                    self.tree[cur_ix].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                    self.link_stack.push(LinkStackEl {
                        node: cur_ix,
                        ty: LinkStackTy::Image,
                    });
                }
                ItemBody::MaybeLinkClose(could_be_ref) => {
                    self.tree[cur_ix].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                    let tos_link = self.link_stack.pop();
                    if let Some(tos) = tos_link {
                        // skip rendering if already in a link, unless its an
                        // image
                        if tos.ty != LinkStackTy::Image
                            && matches!(
                                self.tree[self.tree.peek_up().unwrap()].item.body,
                                ItemBody::Link(..)
                            )
                        {
                            continue;
                        }
                        if tos.ty == LinkStackTy::Disabled {
                            continue;
                        }
                        let next = self.tree[cur_ix].next;
                        // rill: a defined `[^label]` is a footnote call before any resource.
                        let label =
                            &self.text[self.tree[tos.node].item.end..self.tree[cur_ix].item.start];
                        let footnote_call = tos.ty == LinkStackTy::Link
                            && label.starts_with('^')
                            && self.allocs.footdefs.contains(&label[1..]);
                        let inline = if footnote_call {
                            None
                        } else {
                            self.scan_inline_link(block_text, self.tree[cur_ix].item.end, next)
                        };
                        if let Some((next_ix, url, title)) = inline {
                            let next_node = scan_nodes_to_ix(&self.tree, next, next_ix);
                            if let Some(prev_ix) = prev {
                                self.tree[prev_ix].next = None;
                            }
                            cur = Some(tos.node);
                            cur_ix = tos.node;
                            let link_ix =
                                self.allocs
                                    .allocate_link(LinkType::Inline, url, title, "".into());
                            self.tree[cur_ix].item.body = if tos.ty == LinkStackTy::Image {
                                ItemBody::Image(link_ix)
                            } else {
                                ItemBody::Link(link_ix)
                            };
                            self.tree[cur_ix].child = self.tree[cur_ix].next;
                            self.tree[cur_ix].next = next_node;
                            self.tree[cur_ix].item.end = next_ix;
                            if let Some(next_node_ix) = next_node {
                                self.tree[next_node_ix].item.start =
                                    max(self.tree[next_node_ix].item.start, next_ix);
                            }

                            if tos.ty == LinkStackTy::Link {
                                self.disable_all_links();
                            }
                        } else {
                            // ok, so its not an inline link. maybe it is a reference
                            // to a defined link?
                            // rill: a footnote call ignores a following `[label]`.
                            let scan_result = if footnote_call {
                                RefScan::Failed
                            } else {
                                scan_reference(&self.tree, block_text, next)
                            };
                            let (node_after_link, link_type) = match scan_result {
                                // [label][reference]
                                RefScan::LinkLabel(_, end_ix) => {
                                    // Toggle reference viability of the last closing bracket,
                                    // so that we can skip it on future iterations in case
                                    // it fails in this one. In particular, we won't call
                                    // the broken link callback twice on one reference.
                                    let reference_close_node = if let Some(node) =
                                        scan_nodes_to_ix(&self.tree, next, end_ix - 1)
                                    {
                                        node
                                    } else {
                                        continue;
                                    };
                                    self.tree[reference_close_node].item.body =
                                        ItemBody::MaybeLinkClose(false);
                                    let next_node = self.tree[reference_close_node].next;

                                    (next_node, LinkType::Reference)
                                }
                                // [reference][]
                                RefScan::Collapsed(next_node) => {
                                    // This reference has already been tried, and it's not
                                    // valid. Skip it.
                                    if !could_be_ref {
                                        continue;
                                    }
                                    (next_node, LinkType::Collapsed)
                                }
                                // [shortcut]
                                //
                                // [shortcut]: /blah
                                RefScan::Failed | RefScan::UnexpectedFootnote => {
                                    // rill: micromark's label end takes a following `[` only
                                    // as a full or collapsed reference, never a shortcut.
                                    let bracket_follows = next.is_some_and(|next| {
                                        self.tree[next].item.start == self.tree[cur_ix].item.end
                                            && block_text.as_bytes().get(self.tree[next].item.start)
                                                == Some(&b'[')
                                    });
                                    if !could_be_ref || bracket_follows && !footnote_call {
                                        continue;
                                    }
                                    (next, LinkType::Shortcut)
                                }
                            };

                            // FIXME: references and labels are mixed in the naming of variables
                            // below. Disambiguate!

                            // (label, source_ix end)
                            let label: Option<(ReferenceLabel<'input>, usize)> = match scan_result {
                                RefScan::LinkLabel(l, end_ix) => {
                                    Some((ReferenceLabel::Link(l), end_ix))
                                }
                                RefScan::Collapsed(..)
                                | RefScan::Failed
                                | RefScan::UnexpectedFootnote => {
                                    // No label? maybe it is a shortcut reference
                                    let label_start = self.tree[tos.node].item.end - 1;
                                    let label_end = self.tree[cur_ix].item.end;
                                    scan_link_label(&self.tree, &self.text[label_start..label_end])
                                        .map(|(ix, label)| (label, label_start + ix))
                                        .filter(|(_, end)| *end == label_end)
                                }
                            };

                            let id = match &label {
                                Some(
                                    (ReferenceLabel::Link(l), _) | (ReferenceLabel::Footnote(l), _),
                                ) => l.clone(),
                                None => "".into(),
                            };

                            // see if it's a footnote reference
                            if let Some((ReferenceLabel::Footnote(l), end)) = label {
                                let footref = self.allocs.allocate_cow(l);
                                if self.allocs.footdefs.contains(&self.allocs.cows[footref.0]) {
                                    // If this came from a MaybeImage, then the `!` prefix
                                    // isn't part of the footnote reference.
                                    let footnote_ix = if tos.ty == LinkStackTy::Image {
                                        self.tree[tos.node].next = Some(cur_ix);
                                        self.tree[tos.node].child = None;
                                        self.tree[tos.node].item.body =
                                            ItemBody::SynthesizeChar('!');
                                        self.tree[cur_ix].item.start =
                                            self.tree[tos.node].item.start + 1;
                                        self.tree[tos.node].item.end =
                                            self.tree[tos.node].item.start + 1;
                                        cur_ix
                                    } else {
                                        tos.node
                                    };
                                    // use `next` instead of `node_after_link` because
                                    // node_after_link is calculated for a [collapsed][] link,
                                    // which footnotes don't support.
                                    self.tree[footnote_ix].next = next;
                                    self.tree[footnote_ix].child = None;
                                    self.tree[footnote_ix].item.body =
                                        ItemBody::FootnoteReference(footref);
                                    self.tree[footnote_ix].item.end = end;
                                    prev = Some(footnote_ix);
                                    cur = next;
                                    // rill: brackets opened before the call stay open, as
                                    // micromark's label starts do.
                                    continue;
                                }
                            } else if let Some((ReferenceLabel::Link(link_label), end)) = label {
                                if let Some((url, title)) = self.fetch_link_url_title(link_label) {
                                    let link_ix =
                                        self.allocs.allocate_link(link_type, url, title, id);
                                    self.tree[tos.node].item.body = if tos.ty == LinkStackTy::Image
                                    {
                                        ItemBody::Image(link_ix)
                                    } else {
                                        ItemBody::Link(link_ix)
                                    };
                                    let label_node = self.tree[tos.node].next;

                                    // lets do some tree surgery to add the link to the tree
                                    // 1st: skip the label node and close node
                                    self.tree[tos.node].next = node_after_link;

                                    // then, if it exists, add the label node as a child to the link node
                                    if label_node != cur {
                                        self.tree[tos.node].child = label_node;

                                        // finally: disconnect list of children
                                        if let Some(prev_ix) = prev {
                                            self.tree[prev_ix].next = None;
                                        }
                                    }

                                    self.tree[tos.node].item.end = end;

                                    // set up cur so next node will be node_after_link
                                    cur = Some(tos.node);
                                    cur_ix = tos.node;

                                    if tos.ty == LinkStackTy::Link {
                                        self.disable_all_links();
                                    }
                                }
                            }
                        }
                    }
                }
                ItemBody::Text { .. } if autolinks && self.link_stack.is_empty() => {
                    if let Some((link, after)) = self.literal_autolink(cur_ix, prev, block_text) {
                        prev = Some(link);
                        cur = after;
                        continue;
                    }
                }
                _ => {}
            }
            prev = cur;
            cur = self.tree[cur_ix].next;
        }
        self.link_stack.clear();
        self.code_delims.clear();
        self.math_text_delims.clear();
    }

    /// rill: GFM literal autolinks (micromark-extension-gfm-autolink-literal), found in
    /// the same left-to-right pass as code spans and links so their precedence matches.
    /// Returns the link node and the node after it.
    fn literal_autolink(
        &mut self,
        cur_ix: TreeIndex,
        prev: Option<TreeIndex>,
        block_text: &'input str,
    ) -> Option<(TreeIndex, Option<TreeIndex>)> {
        use crate::autolink_literal::{scan, LiteralKind};
        let item = self.tree[cur_ix].item;
        let line_start = prev.map_or(true, |prev_ix| {
            matches!(
                self.tree[prev_ix].item.body,
                ItemBody::SoftBreak | ItemBody::HardBreak(_)
            )
        });
        let escaped = matches!(
            item.body,
            ItemBody::Text {
                backslash_escaped: true
            }
        );
        let mut p = item.start;
        while p < item.end {
            let c = block_text[p..].chars().next()?;
            // An escaped character is never a literal's start.
            if escaped && p == item.start {
                p += c.len_utf8();
                continue;
            }
            // An email literal may start at `_` (GFM's construct comes before attention).
            let atext = c == '+' || c == '-' || c == '.' || c == '_' || c.is_ascii_alphanumeric();
            if atext {
                let previous = if p == item.start && line_start {
                    None
                } else {
                    block_text[..p].chars().next_back()
                };
                if let Some((end, kind)) = scan(block_text, p, previous) {
                    let next = self.tree[cur_ix].next;
                    let raw = &block_text[p..end];
                    let url: CowStr<'input> = if kind == LiteralKind::Www {
                        format!("http://{raw}").into()
                    } else {
                        raw.into()
                    };
                    let link_type = if kind == LiteralKind::Email {
                        LinkType::Email
                    } else {
                        LinkType::Autolink
                    };
                    let link_ix = self
                        .allocs
                        .allocate_link(link_type, url, "".into(), "".into());
                    let text_node = self.tree.create_node(Item {
                        start: p,
                        end,
                        body: ItemBody::Text {
                            backslash_escaped: false,
                        },
                    });
                    let link_item = Item {
                        start: p,
                        end,
                        body: ItemBody::Link(link_ix),
                    };
                    let link = if p == item.start {
                        self.tree[cur_ix].item = link_item;
                        cur_ix
                    } else {
                        self.tree[cur_ix].item.end = p;
                        let link = self.tree.create_node(link_item);
                        self.tree[cur_ix].next = Some(link);
                        link
                    };
                    self.tree[link].child = Some(text_node);
                    let after = if end < item.end {
                        let rest = self.tree.create_node(Item {
                            start: end,
                            end: item.end,
                            body: ItemBody::Text {
                                backslash_escaped: false,
                            },
                        });
                        self.tree[rest].next = next;
                        Some(rest)
                    } else {
                        let after = scan_nodes_to_ix(&self.tree, next, end);
                        if let Some(after_ix) = after {
                            if self.tree[after_ix].item.start < end {
                                self.tree[after_ix].item.start = end;
                                self.tree[after_ix].item.body = ItemBody::Text {
                                    backslash_escaped: false,
                                };
                            }
                        }
                        after
                    };
                    self.tree[link].next = after;
                    // Delimiter caches may point at nodes the link just swallowed.
                    self.code_delims.clear();
                    self.math_text_delims.clear();
                    return Some((link, after));
                }
            }
            p += c.len_utf8();
        }
        None
    }

    // rill: micromark resolves attention and strikethrough as separate passes, in the order
    // each first occurs in the paragraph (`resolveAllConstructs`). A resolved span's interior
    // then resolves in isolation (`insideSpan`: attention, then strikethrough), which the
    // iterator's lazy `handle_inline` on each child chain already does.
    fn handle_emphasis_and_hard_break(&mut self) {
        let nested = self.tree.peek_up().is_some_and(|ix| {
            matches!(
                self.tree[ix].item.body,
                ItemBody::Link(..)
                    | ItemBody::Image(..)
                    | ItemBody::Emphasis
                    | ItemBody::Strong
                    | ItemBody::Strikethrough
            )
        });
        let strikethrough_first = !nested && self.first_delimiter(self.tree.cur()) == Some(b'~');
        self.resolve_delimiters(!strikethrough_first);
        self.resolve_delimiters(strikethrough_first);
    }

    /// The first `*`, `_`, or `~` delimiter run under `node` and its siblings, in source order.
    fn first_delimiter(&self, mut node: Option<TreeIndex>) -> Option<u8> {
        while let Some(ix) = node {
            if let ItemBody::MaybeEmphasis(..) = self.tree[ix].item.body {
                return Some(self.text.as_bytes()[self.tree[ix].item.start]);
            }
            if let Some(c) = self.first_delimiter(self.tree[ix].child) {
                return Some(c);
            }
            node = self.tree[ix].next;
        }
        None
    }

    /// One delimiter pass over the current chain: `*`/`_` runs when `attention`, else `~`.
    fn resolve_delimiters(&mut self, attention: bool) {
        let mut prev = None;
        let mut prev_ix: TreeIndex;
        let mut cur = self.tree.cur();

        while let Some(mut cur_ix) = cur {
            match self.tree[cur_ix].item.body {
                ItemBody::MaybeEmphasis(mut count, can_open, can_close)
                    if (self.text.as_bytes()[self.tree[cur_ix].item.start] != b'~')
                        == attention =>
                {
                    let run_length = count;
                    let c = self.text.as_bytes()[self.tree[cur_ix].item.start];
                    let both = can_open && can_close;
                    if can_close {
                        while let Some(el) =
                            self.inline_stack
                                .find_match(&mut self.tree, c, run_length, count, both)
                        {
                            // have a match!
                            if let Some(prev_ix) = prev {
                                self.tree[prev_ix].next = None;
                            }
                            // rill: micromark matches two at most per step, then re-applies
                            // the rule of 3 to what remains.
                            let match_count = if c == b'~' {
                                min(count, el.count)
                            } else {
                                min(count, el.count).min(2)
                            };
                            // start, end are tree node indices
                            let mut end = cur_ix - 1;
                            let mut start = el.start + el.count;

                            // work from the inside out
                            while start > el.start + el.count - match_count {
                                let inc = if start > el.start + el.count - match_count + 1 {
                                    2
                                } else {
                                    1
                                };
                                let ty = if c == b'~' {
                                    ItemBody::Strikethrough
                                } else if inc == 2 {
                                    ItemBody::Strong
                                } else {
                                    ItemBody::Emphasis
                                };

                                let root = start - inc;
                                end = end + inc;
                                self.tree[root].item.body = ty;
                                self.tree[root].item.end = self.tree[end].item.end;
                                self.tree[root].child = Some(start);
                                self.tree[root].next = None;
                                start = root;
                            }

                            // set next for top most emph level
                            prev_ix = el.start + el.count - match_count;
                            prev = Some(prev_ix);
                            cur = self.tree[cur_ix + match_count - 1].next;
                            self.tree[prev_ix].next = cur;

                            if el.count > match_count {
                                self.inline_stack.push(InlineEl {
                                    start: el.start,
                                    count: el.count - match_count,
                                    run_length: el.run_length,
                                    c: el.c,
                                    both: el.both,
                                })
                            }
                            count -= match_count;
                            if count > 0 {
                                cur_ix = cur.unwrap();
                            } else {
                                break;
                            }
                        }
                    }
                    if count > 0 {
                        if can_open {
                            self.inline_stack.push(InlineEl {
                                start: cur_ix,
                                run_length,
                                count,
                                c,
                                both,
                            });
                        } else {
                            for i in 0..count {
                                self.tree[cur_ix + i].item.body = ItemBody::Text {
                                    backslash_escaped: false,
                                };
                            }
                        }
                        prev_ix = cur_ix + count - 1;
                        prev = Some(prev_ix);
                        cur = self.tree[prev_ix].next;
                    }
                }
                ItemBody::HardBreak(true) => {
                    if self.tree[cur_ix].next.is_none() {
                        self.tree[cur_ix].item.body = ItemBody::SynthesizeChar('\\');
                    }
                    prev = cur;
                    cur = self.tree[cur_ix].next;
                }
                _ => {
                    prev = cur;
                    cur = self.tree[cur_ix].next;
                }
            }
        }
        self.inline_stack.pop_all(&mut self.tree);
    }

    fn disable_all_links(&mut self) {
        self.link_stack.disable_all_links();
    }

    /// Returns next byte index, url and title.
    fn scan_inline_link(
        &self,
        underlying: &'input str,
        mut ix: usize,
        node: Option<TreeIndex>,
    ) -> Option<(usize, CowStr<'input>, CowStr<'input>)> {
        if underlying.as_bytes().get(ix) != Some(&b'(') {
            return None;
        }
        ix += 1;

        let scan_separator = |ix: &mut usize| {
            *ix += scan_while(&underlying.as_bytes()[*ix..], is_ascii_whitespace_no_nl);
            if let Some(bl) = scan_eol(&underlying.as_bytes()[*ix..]) {
                *ix += bl;
                *ix += skip_container_prefixes(&self.tree, &underlying.as_bytes()[*ix..]);
            }
            *ix += scan_while(&underlying.as_bytes()[*ix..], is_ascii_whitespace_no_nl);
        };

        scan_separator(&mut ix);

        let (dest_length, dest) = scan_link_dest(underlying, ix, LINK_MAX_NESTED_PARENS)?;
        let dest = unescape(dest, self.tree.is_in_table());
        ix += dest_length;

        scan_separator(&mut ix);

        let title = if let Some((bytes_scanned, t)) = self.scan_link_title(underlying, ix, node) {
            ix += bytes_scanned;
            scan_separator(&mut ix);
            t
        } else {
            "".into()
        };
        if underlying.as_bytes().get(ix) != Some(&b')') {
            return None;
        }
        ix += 1;

        Some((ix, dest, title))
    }

    // returns (bytes scanned, title cow)
    fn scan_link_title(
        &self,
        text: &'input str,
        start_ix: usize,
        node: Option<TreeIndex>,
    ) -> Option<(usize, CowStr<'input>)> {
        let bytes = text.as_bytes();
        let open = match bytes.get(start_ix) {
            Some(b @ b'\'') | Some(b @ b'\"') | Some(b @ b'(') => *b,
            _ => return None,
        };
        let close = if open == b'(' { b')' } else { open };

        let mut title = String::new();
        let mut mark = start_ix + 1;
        let mut i = start_ix + 1;

        while i < bytes.len() {
            let c = bytes[i];

            if c == close {
                let cow = if mark == 1 {
                    (i - start_ix + 1, text[mark..i].into())
                } else {
                    title.push_str(&text[mark..i]);
                    (i - start_ix + 1, title.into())
                };

                return Some(cow);
            }
            if c == open {
                return None;
            }

            if c == b'\n' || c == b'\r' {
                if let Some(node_ix) = scan_nodes_to_ix(&self.tree, node, i + 1) {
                    if self.tree[node_ix].item.start > i {
                        title.push_str(&text[mark..i]);
                        title.push('\n');
                        i = self.tree[node_ix].item.start;
                        mark = i;
                        continue;
                    }
                }
            }
            if c == b'&' {
                if let (n, Some(value)) = scan_entity(&bytes[i..]) {
                    title.push_str(&text[mark..i]);
                    title.push_str(&value);
                    i += n;
                    mark = i;
                    continue;
                }
            }
            if self.tree.is_in_table()
                && c == b'\\'
                && i + 2 < bytes.len()
                && bytes[i + 1] == b'\\'
                && bytes[i + 2] == b'|'
            {
                // this runs if there are an even number of pipes in a table
                // if it's odd, then it gets parsed as normal
                title.push_str(&text[mark..i]);
                i += 2;
                mark = i;
            }
            if c == b'\\' && i + 1 < bytes.len() && is_ascii_punctuation(bytes[i + 1]) {
                title.push_str(&text[mark..i]);
                i += 1;
                mark = i;
            }

            i += 1;
        }

        None
    }

    /// Make a code span.
    ///
    /// Both `open` and `close` are matching MaybeCode items.
    /// rill: MDAST code (and `$$` math) text keeps its raw line endings without container
    /// prefixes, and drops one padding space or line ending from each side.
    fn make_code_span(
        &mut self,
        open: TreeIndex,
        close: TreeIndex,
        preceding_backslash: bool,
        math: bool,
    ) {
        let span_start = self.tree[open].item.end;
        let span_end = self.tree[close].item.start;
        let mut buf: Option<String> = None;

        let spanned_text = &self.text[span_start..span_end];
        let spanned_bytes = spanned_text.as_bytes();
        let mut start_ix = 0;
        let mut ix = 0;
        while ix < spanned_bytes.len() {
            let c = spanned_bytes[ix];
            if c == b'\r' || c == b'\n' {
                ix += 1;
                if c == b'\r' && spanned_bytes.get(ix) == Some(&b'\n') {
                    ix += 1;
                }
                // rill: scan the whole line; the span's end is no line end.
                let skip =
                    skip_container_prefixes(&self.tree, &self.text.as_bytes()[span_start + ix..])
                        .min(spanned_bytes.len() - ix);
                if skip > 0 {
                    let buf = buf.get_or_insert_with(|| String::with_capacity(spanned_bytes.len()));
                    buf.push_str(&spanned_text[start_ix..ix]);
                    ix += skip;
                    start_ix = ix;
                }
            } else if !math
                && c == b'\\'
                && spanned_bytes.get(ix + 1) == Some(&b'|')
                && self.tree.is_in_table()
            {
                let buf = buf.get_or_insert_with(|| String::with_capacity(spanned_bytes.len()));
                buf.push_str(&spanned_text[start_ix..ix]);
                buf.push('|');
                ix += 2;
                start_ix = ix;
            } else {
                ix += 1;
            }
        }
        let value: CowStr<'input> = match buf {
            Some(mut buf) => {
                buf.push_str(&spanned_text[start_ix..]);
                buf.into()
            }
            None => spanned_text.into(),
        };
        let bytes = value.as_bytes();
        let pad = |byte: Option<&u8>| matches!(byte, Some(b' ' | b'\n' | b'\r'));
        let cow: CowStr<'input> = if pad(bytes.first())
            && pad(bytes.last())
            && !bytes.iter().all(|b| matches!(b, b' ' | b'\n' | b'\r'))
        {
            let head = if bytes.starts_with(b"\r\n") { 2 } else { 1 };
            let tail = if bytes.len() >= head + 2 && bytes.ends_with(b"\r\n") {
                2
            } else {
                1
            };
            value[head..value.len() - tail].to_owned().into()
        } else {
            value
        };
        let body = |allocs: &mut Allocations<'input>| {
            let cow_ix = allocs.allocate_cow(cow);
            if math {
                ItemBody::Math(cow_ix)
            } else {
                ItemBody::Code(cow_ix)
            }
        };

        if preceding_backslash {
            self.tree[open].item.body = ItemBody::Text {
                backslash_escaped: true,
            };
            self.tree[open].item.end = self.tree[open].item.start + 1;
            self.tree[open].next = Some(close);
            self.tree[close].item.body = body(&mut self.allocs);
            self.tree[close].item.start = self.tree[open].item.start + 1;
        } else {
            self.tree[open].item.body = body(&mut self.allocs);
            self.tree[open].item.end = self.tree[close].item.end;
            self.tree[open].next = self.tree[close].next;
        }
    }

    /// On success, returns a buffer containing the inline html and byte offset.
    /// When no bytes were skipped, the buffer will be empty and the html can be
    /// represented as a subslice of the input string.
    fn scan_inline_html(&mut self, bytes: &[u8], ix: usize) -> Option<(Vec<u8>, usize)> {
        let c = *bytes.get(ix)?;
        if c == b'!' {
            Some((
                vec![],
                scan_inline_html_comment(bytes, ix + 1, &mut self.html_scan_guard)?,
            ))
        } else if c == b'?' {
            Some((
                vec![],
                scan_inline_html_processing(bytes, ix + 1, &mut self.html_scan_guard)?,
            ))
        } else {
            let (span, i) = scan_html_block_inner(
                // Subtract 1 to include the < character
                &bytes[(ix - 1)..],
                Some(&|bytes| skip_container_prefixes(&self.tree, bytes)),
            )?;
            Some((span, i + ix - 1))
        }
    }

    /// Consumes the event iterator and produces an iterator that produces
    /// `(Event, Range)` pairs, where the `Range` value maps to the corresponding
    /// range in the markdown source.
    pub fn into_offset_iter(self) -> OffsetIter<'input> {
        OffsetIter { inner: self }
    }
}

/// Returns number of containers scanned.
pub(crate) fn scan_containers(tree: &Tree<Item>, line_start: &mut LineStart<'_>) -> usize {
    let mut i = 0;
    for &node_ix in tree.walk_spine() {
        match tree[node_ix].item.body {
            ItemBody::BlockQuote => {
                let save = line_start.clone();
                let _ = line_start.scan_space(3);
                if !line_start.scan_blockquote_marker() {
                    *line_start = save;
                    break;
                }
            }
            ItemBody::ListItem(indent) => {
                let save = line_start.clone();
                if !line_start.scan_space(indent) && !line_start.is_at_eol() {
                    *line_start = save;
                    break;
                }
            }
            ItemBody::FootnoteDefinition(..) => {
                let save = line_start.clone();
                if !line_start.scan_space(4) && !line_start.is_at_eol() {
                    *line_start = save;
                    break;
                }
            }
            _ => (),
        }
        i += 1;
    }
    i
}
pub(crate) fn skip_container_prefixes(tree: &Tree<Item>, bytes: &[u8]) -> usize {
    let mut line_start = LineStart::new(bytes);
    let _ = scan_containers(tree, &mut line_start);
    line_start.bytes_scanned()
}

impl Tree<Item> {
    pub(crate) fn append_text(&mut self, start: usize, end: usize, backslash_escaped: bool) {
        if end > start {
            if let Some(ix) = self.cur() {
                if matches!(self[ix].item.body, ItemBody::Text { .. }) && self[ix].item.end == start
                {
                    self[ix].item.end = end;
                    return;
                }
            }
            self.append(Item {
                start,
                end,
                body: ItemBody::Text { backslash_escaped },
            });
        }
    }
    /// Returns true if the current node is inside a table.
    ///
    /// If `cur` is an ItemBody::Table, it would return false,
    /// but since the `TableRow` and `TableHead` and `TableCell`
    /// are children of the table, anything doing inline parsing
    /// doesn't need to care about that.
    pub(crate) fn is_in_table(&self) -> bool {
        fn might_be_in_table(item: &Item) -> bool {
            item.body.is_inline()
                || matches!(item.body, |ItemBody::TableHead| ItemBody::TableRow
                    | ItemBody::TableCell)
        }
        for &ix in self.walk_spine().rev() {
            if matches!(self[ix].item.body, ItemBody::Table(_)) {
                return true;
            }
            if !might_be_in_table(&self[ix].item) {
                return false;
            }
        }
        false
    }
}

#[derive(Copy, Clone, Debug)]
struct InlineEl {
    /// offset of tree node
    start: TreeIndex,
    /// number of delimiters available for matching
    count: usize,
    /// length of the run that these delimiters came from
    run_length: usize,
    /// b'*', b'_', or b'~'
    c: u8,
    /// can both open and close
    both: bool,
}

#[derive(Debug, Clone, Default)]
struct InlineStack {
    stack: Vec<InlineEl>,
    // Lower bounds for matching indices in the stack. For example
    // a strikethrough delimiter will never match with any element
    // in the stack with index smaller than
    // `lower_bounds[InlineStack::TILDES]`.
    lower_bounds: [usize; 10],
}

impl InlineStack {
    /// These are indices into the lower bounds array.
    /// Not both refers to the property that the delimiter can not both
    /// be opener as a closer.
    const UNDERSCORE_NOT_BOTH: usize = 0;
    const ASTERISK_NOT_BOTH: usize = 1;
    const ASTERISK_BASE: usize = 2;
    const TILDES: usize = 5;
    const UNDERSCORE_BASE: usize = 6;
    // rill: runs of `~` only pair with runs of the same length, so a failed `~` must
    // not hide `~~` openers below it: one bound per length (GFM accepts only 1 and 2).
    const TILDES_2: usize = 9;

    fn tildes(count: usize) -> usize {
        if count == 2 {
            InlineStack::TILDES_2
        } else {
            InlineStack::TILDES
        }
    }

    fn pop_all(&mut self, tree: &mut Tree<Item>) {
        for el in self.stack.drain(..) {
            for i in 0..el.count {
                tree[el.start + i].item.body = ItemBody::Text {
                    backslash_escaped: false,
                };
            }
        }
        self.lower_bounds = [0; 10];
    }

    fn get_lowerbound(&self, c: u8, count: usize, both: bool) -> usize {
        if c == b'_' {
            let mod3_lower = self.lower_bounds[InlineStack::UNDERSCORE_BASE + count % 3];
            if both {
                mod3_lower
            } else {
                min(
                    mod3_lower,
                    self.lower_bounds[InlineStack::UNDERSCORE_NOT_BOTH],
                )
            }
        } else if c == b'*' {
            let mod3_lower = self.lower_bounds[InlineStack::ASTERISK_BASE + count % 3];
            if both {
                mod3_lower
            } else {
                min(
                    mod3_lower,
                    self.lower_bounds[InlineStack::ASTERISK_NOT_BOTH],
                )
            }
        } else {
            self.lower_bounds[InlineStack::tildes(count)]
        }
    }

    fn set_lowerbound(&mut self, c: u8, count: usize, both: bool, new_bound: usize) {
        if c == b'_' {
            if both {
                self.lower_bounds[InlineStack::UNDERSCORE_BASE + count % 3] = new_bound;
            } else {
                self.lower_bounds[InlineStack::UNDERSCORE_NOT_BOTH] = new_bound;
            }
        } else if c == b'*' {
            self.lower_bounds[InlineStack::ASTERISK_BASE + count % 3] = new_bound;
            if !both {
                self.lower_bounds[InlineStack::ASTERISK_NOT_BOTH] = new_bound;
            }
        } else {
            self.lower_bounds[InlineStack::tildes(count)] = new_bound;
        }
    }

    fn truncate(&mut self, new_bound: usize) {
        self.stack.truncate(new_bound);
        for lower_bound in &mut self.lower_bounds {
            if *lower_bound > new_bound {
                *lower_bound = new_bound;
            }
        }
    }

    fn find_match(
        &mut self,
        tree: &mut Tree<Item>,
        c: u8,
        run_length: usize,
        count: usize,
        both: bool,
    ) -> Option<InlineEl> {
        let lowerbound = min(self.stack.len(), self.get_lowerbound(c, count, both));
        // rill: micromark applies the rule of 3 to the runs' remaining sizes.
        let res = self.stack[lowerbound..]
            .iter()
            .cloned()
            .enumerate()
            .rfind(|(_, el)| {
                if c == b'~' && run_length != el.run_length {
                    return false;
                }
                el.c == c && (!both && !el.both || (count + el.count) % 3 != 0 || count % 3 == 0)
            });

        if let Some((matching_ix, matching_el)) = res {
            let matching_ix = matching_ix + lowerbound;
            for el in &self.stack[(matching_ix + 1)..] {
                for i in 0..el.count {
                    tree[el.start + i].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                }
            }
            self.truncate(matching_ix);
            Some(matching_el)
        } else {
            self.set_lowerbound(c, count, both, self.stack.len());
            None
        }
    }

    fn trim_lower_bound(&mut self, ix: usize) {
        self.lower_bounds[ix] = self.lower_bounds[ix].min(self.stack.len());
    }

    fn push(&mut self, el: InlineEl) {
        if el.c == b'~' {
            self.trim_lower_bound(InlineStack::tildes(el.count));
        }
        self.stack.push(el)
    }
}

#[derive(Debug, Clone)]
enum RefScan<'a> {
    // label, source ix of label end
    LinkLabel(CowStr<'a>, usize),
    // contains next node index
    Collapsed(Option<TreeIndex>),
    UnexpectedFootnote,
    Failed,
}

/// Skips forward within a block to a node which spans (ends inclusive) the given
/// index into the source.
fn scan_nodes_to_ix(
    tree: &Tree<Item>,
    mut node: Option<TreeIndex>,
    ix: usize,
) -> Option<TreeIndex> {
    while let Some(node_ix) = node {
        if tree[node_ix].item.end <= ix {
            node = tree[node_ix].next;
        } else {
            break;
        }
    }
    node
}

/// rill: micromark's GFM footnote labels refuse spaces, tabs, and line endings.
pub(crate) fn footnote_label_has_whitespace(label: &str) -> bool {
    label
        .bytes()
        .any(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}

/// Scans an inline link label, which cannot be interrupted.
/// Returns number of bytes (including brackets) and label on success.
fn scan_link_label<'text>(
    tree: &Tree<Item>,
    text: &'text str,
) -> Option<(usize, ReferenceLabel<'text>)> {
    let bytes = text.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'[' {
        return None;
    }
    let linebreak_handler = |bytes: &[u8]| Some(skip_container_prefixes(tree, bytes));
    if b'^' == bytes[1] && bytes.get(2) != Some(&b']') {
        if let Some((byte_index, cow)) =
            scan_link_label_rest(&text[2..], &|_| None, tree.is_in_table())
                .filter(|(byte_index, _)| !footnote_label_has_whitespace(&text[2..1 + byte_index]))
        {
            return Some((byte_index + 2, ReferenceLabel::Footnote(cow)));
        }
    }
    let (byte_index, cow) =
        scan_link_label_rest(&text[1..], &linebreak_handler, tree.is_in_table())?;
    Some((byte_index + 1, ReferenceLabel::Link(cow)))
}

fn scan_reference<'b>(tree: &Tree<Item>, text: &'b str, cur: Option<TreeIndex>) -> RefScan<'b> {
    let cur_ix = match cur {
        None => return RefScan::Failed,
        Some(cur_ix) => cur_ix,
    };
    let start = tree[cur_ix].item.start;
    let tail = &text.as_bytes()[start..];

    if tail.starts_with(b"[]") {
        // TODO: this unwrap is sus and should be looked at closer
        let closing_node = tree[cur_ix].next.unwrap();
        RefScan::Collapsed(tree[closing_node].next)
    } else {
        let label = scan_link_label(tree, &text[start..]);
        match label {
            Some((ix, ReferenceLabel::Link(label))) => RefScan::LinkLabel(label, start + ix),
            Some((_ix, ReferenceLabel::Footnote(_label))) => RefScan::UnexpectedFootnote,
            None => RefScan::Failed,
        }
    }
}

#[derive(Clone, Default)]
struct LinkStack {
    inner: Vec<LinkStackEl>,
    disabled_ix: usize,
}

impl LinkStack {
    fn push(&mut self, el: LinkStackEl) {
        self.inner.push(el);
    }

    fn pop(&mut self) -> Option<LinkStackEl> {
        let el = self.inner.pop();
        self.disabled_ix = std::cmp::min(self.disabled_ix, self.inner.len());
        el
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.disabled_ix = 0;
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn disable_all_links(&mut self) {
        for el in &mut self.inner[self.disabled_ix..] {
            if el.ty == LinkStackTy::Link {
                el.ty = LinkStackTy::Disabled;
            }
        }
        self.disabled_ix = self.inner.len();
    }
}

#[derive(Clone, Debug)]
struct LinkStackEl {
    node: TreeIndex,
    ty: LinkStackTy,
}

#[derive(PartialEq, Clone, Debug)]
enum LinkStackTy {
    Link,
    Image,
    Disabled,
}

/// Contains the destination URL and title of a reference definition.
#[derive(Clone, Debug)]
pub struct LinkDef<'a> {
    pub dest: CowStr<'a>,
    pub title: Option<CowStr<'a>>,
}

/// Tracks tree indices of code span delimiters of each length. It should prevent
/// quadratic scanning behaviours by providing (amortized) constant time lookups.
struct CodeDelims {
    inner: HashMap<usize, VecDeque<TreeIndex>>,
    seen_first: bool,
}

impl CodeDelims {
    fn new() -> Self {
        Self {
            inner: Default::default(),
            seen_first: false,
        }
    }

    fn insert(&mut self, count: usize, ix: TreeIndex) {
        if self.seen_first {
            self.inner.entry(count).or_default().push_back(ix);
        } else {
            // Skip the first insert, since that delimiter will always
            // be an opener and not a closer.
            self.seen_first = true;
        }
    }

    fn is_populated(&self) -> bool {
        !self.inner.is_empty()
    }

    fn find(&mut self, open_ix: TreeIndex, count: usize) -> Option<TreeIndex> {
        while let Some(ix) = self.inner.get_mut(&count)?.pop_front() {
            if ix > open_ix {
                return Some(ix);
            }
        }
        None
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.seen_first = false;
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct LinkIndex(usize);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct CowIndex(usize);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct AlignmentIndex(usize);

#[derive(Clone)]
pub(crate) struct Allocations<'a> {
    pub refdefs: RefDefs<'a>,
    pub footdefs: FootnoteDefs<'a>,
    links: Vec<(LinkType, CowStr<'a>, CowStr<'a>, CowStr<'a>)>,
    cows: Vec<CowStr<'a>>,
    alignments: Vec<Vec<Alignment>>,
    /// rill: every definition in source order (MDAST keeps duplicates); raw label source.
    pub definitions: Vec<(&'a str, LinkDef<'a>)>,
}

/// Keeps track of the reference definitions defined in the document.
#[derive(Clone, Default, Debug)]
pub struct RefDefs<'input>(pub(crate) HashMap<LinkLabel<'input>, LinkDef<'input>>);

/// Keeps track of the footnote definitions defined in the document.
#[derive(Clone, Default, Debug)]
pub struct FootnoteDefs<'input>(pub(crate) HashSet<FootnoteLabel<'input>>);

impl<'input, 'b, 's> RefDefs<'input>
where
    's: 'b,
{
    /// Performs a lookup on a reference label, normalized as micromark does.
    pub fn get(&'s self, key: &'b str) -> Option<&'b LinkDef<'input>> {
        self.0.get(&normalize_label(key))
    }
}

impl<'input, 'b, 's> FootnoteDefs<'input>
where
    's: 'b,
{
    /// Performs a lookup on a footnote label, normalized as micromark does.
    pub fn contains(&'s self, key: &'b str) -> bool {
        self.0.contains(&normalize_label(key))
    }
}

impl<'a> Allocations<'a> {
    pub fn new() -> Self {
        Self {
            refdefs: RefDefs::default(),
            footdefs: FootnoteDefs::default(),
            links: Vec::with_capacity(128),
            cows: Vec::new(),
            alignments: Vec::new(),
            definitions: Vec::new(),
        }
    }

    pub fn allocate_cow(&mut self, cow: CowStr<'a>) -> CowIndex {
        let ix = self.cows.len();
        self.cows.push(cow);
        CowIndex(ix)
    }

    pub fn allocate_link(
        &mut self,
        ty: LinkType,
        url: CowStr<'a>,
        title: CowStr<'a>,
        id: CowStr<'a>,
    ) -> LinkIndex {
        let ix = self.links.len();
        self.links.push((ty, url, title, id));
        LinkIndex(ix)
    }

    pub fn allocate_alignment(&mut self, alignment: Vec<Alignment>) -> AlignmentIndex {
        let ix = self.alignments.len();
        self.alignments.push(alignment);
        AlignmentIndex(ix)
    }

    pub fn take_cow(&mut self, ix: CowIndex) -> CowStr<'a> {
        std::mem::replace(&mut self.cows[ix.0], "".into())
    }

    pub fn take_link(&mut self, ix: LinkIndex) -> (LinkType, CowStr<'a>, CowStr<'a>, CowStr<'a>) {
        let default_link = (LinkType::Shortcut, "".into(), "".into(), "".into());
        std::mem::replace(&mut self.links[ix.0], default_link)
    }

    pub fn take_alignment(&mut self, ix: AlignmentIndex) -> Vec<Alignment> {
        std::mem::take(&mut self.alignments[ix.0])
    }
}

impl<'a> Index<CowIndex> for Allocations<'a> {
    type Output = CowStr<'a>;

    fn index(&self, ix: CowIndex) -> &Self::Output {
        self.cows.index(ix.0)
    }
}

impl<'a> Index<LinkIndex> for Allocations<'a> {
    type Output = (LinkType, CowStr<'a>, CowStr<'a>, CowStr<'a>);

    fn index(&self, ix: LinkIndex) -> &Self::Output {
        self.links.index(ix.0)
    }
}

impl<'a> Index<AlignmentIndex> for Allocations<'a> {
    type Output = Vec<Alignment>;

    fn index(&self, ix: AlignmentIndex) -> &Self::Output {
        self.alignments.index(ix.0)
    }
}

/// A struct containing information on the reachability of certain inline HTML
/// elements. In particular, for cdata elements (`<![CDATA[`), processing
/// elements (`<?`) and declarations (`<!DECLARATION`). The respectives usizes
/// represent the indices before which a scan will always fail and can hence
/// be skipped.
#[derive(Clone, Default)]
pub(crate) struct HtmlScanGuard {
    pub cdata: usize,
    pub processing: usize,
    pub declaration: usize,
    pub comment: usize,
}

/// Markdown event and source range iterator.
///
/// Generates tuples where the first element is the markdown event and the second
/// is a the corresponding range in the source string.
///
/// Constructed from a `Parser` using its
/// [`into_offset_iter`](struct.Parser.html#method.into_offset_iter) method.
#[derive(Debug)]
pub struct OffsetIter<'a> {
    inner: Parser<'a>,
}

impl<'a> Iterator for OffsetIter<'a> {
    type Item = (Event<'a>, Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.tree.cur() {
            None => {
                // rill: MDAST keeps tight paragraphs; list spread carries tightness.
                let ix = self.inner.tree.pop()?;
                let tag_end = body_to_tag_end(&self.inner.tree[ix].item.body);
                self.inner.tree.next_sibling(ix);
                let span = self.inner.tree[ix].item.start..self.inner.tree[ix].item.end;
                debug_assert!(span.start <= span.end);
                Some((Event::End(tag_end), span))
            }
            Some(cur_ix) => {
                if self.inner.tree[cur_ix].item.body.is_maybe_inline() {
                    self.inner.handle_inline();
                }

                let node = self.inner.tree[cur_ix];
                let item = node.item;
                let event = item_to_event(item, self.inner.text, &mut self.inner.allocs);
                if let Event::Start(..) = event {
                    self.inner.tree.push();
                    // rill: a block of plain text can still hold GFM literal autolinks.
                    if let Some(child_ix) = self.inner.tree.cur() {
                        if !self.inner.tree[child_ix].item.body.is_maybe_inline()
                            && matches!(
                                item.body,
                                ItemBody::Paragraph
                                    | ItemBody::TightParagraph
                                    | ItemBody::Heading(..)
                                    | ItemBody::TableCell
                            )
                            && crate::autolink_literal::may_contain(
                                &self.inner.text[self.inner.tree[child_ix].item.start..item.end],
                            )
                        {
                            self.inner.handle_inline();
                        }
                    }
                } else {
                    self.inner.tree.next_sibling(cur_ix);
                }
                debug_assert!(item.start <= item.end);
                Some((event, item.start..item.end))
            }
        }
    }
}

fn body_to_tag_end(body: &ItemBody) -> TagEnd {
    match *body {
        ItemBody::Paragraph | ItemBody::TightParagraph => TagEnd::Paragraph,
        ItemBody::Emphasis => TagEnd::Emphasis,
        ItemBody::Strong => TagEnd::Strong,
        ItemBody::Strikethrough => TagEnd::Strikethrough,
        ItemBody::Link(..) => TagEnd::Link,
        ItemBody::Image(..) => TagEnd::Image,
        ItemBody::Heading(level) => TagEnd::Heading(level),
        ItemBody::IndentCodeBlock | ItemBody::FencedCodeBlock(..) => TagEnd::CodeBlock,
        ItemBody::MathBlock(..) => TagEnd::MathBlock,
        ItemBody::BlockQuote => TagEnd::BlockQuote,
        ItemBody::HtmlBlock => TagEnd::HtmlBlock,
        ItemBody::List(_, c, _) => {
            let is_ordered = c == b'.' || c == b')';
            TagEnd::List(is_ordered)
        }
        ItemBody::ListItem(_) => TagEnd::Item,
        ItemBody::TableHead => TagEnd::TableHead,
        ItemBody::TableCell => TagEnd::TableCell,
        ItemBody::TableRow => TagEnd::TableRow,
        ItemBody::Table(..) => TagEnd::Table,
        ItemBody::FootnoteDefinition(..) => TagEnd::FootnoteDefinition,
        _ => panic!("unexpected item body {:?}", body),
    }
}

fn item_to_event<'a>(item: Item, text: &'a str, allocs: &mut Allocations<'a>) -> Event<'a> {
    let tag = match item.body {
        ItemBody::Text { .. } => return Event::Text(text[item.start..item.end].into()),
        ItemBody::Code(cow_ix) => return Event::Code(allocs.take_cow(cow_ix)),
        ItemBody::SynthesizeText(cow_ix) => return Event::Text(allocs.take_cow(cow_ix)),
        ItemBody::SynthesizeChar(c) => return Event::Text(c.into()),
        ItemBody::Definition(ix) => {
            let (label, def) = &allocs.definitions[ix];
            return Event::Definition {
                label: CowStr::Borrowed(label),
                dest: def.dest.clone(),
                title: def.title.clone(),
            };
        }
        ItemBody::HtmlBlock => Tag::HtmlBlock,
        ItemBody::Html => return Event::Html(text[item.start..item.end].into()),
        ItemBody::InlineHtml => return Event::InlineHtml(text[item.start..item.end].into()),
        ItemBody::OwnedInlineHtml(cow_ix) => return Event::InlineHtml(allocs.take_cow(cow_ix)),
        ItemBody::SoftBreak => return Event::SoftBreak,
        ItemBody::HardBreak(_) => return Event::HardBreak,
        ItemBody::FootnoteReference(cow_ix) => {
            return Event::FootnoteReference(allocs.take_cow(cow_ix))
        }
        ItemBody::TaskListMarker(checked) => return Event::TaskListMarker(checked),
        ItemBody::Rule => return Event::Rule,
        ItemBody::Paragraph | ItemBody::TightParagraph => Tag::Paragraph,
        ItemBody::Emphasis => Tag::Emphasis,
        ItemBody::Strong => Tag::Strong,
        ItemBody::Strikethrough => Tag::Strikethrough,
        ItemBody::Link(link_ix) => {
            let (link_type, dest_url, title, id) = allocs.take_link(link_ix);
            Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }
        }
        ItemBody::Image(link_ix) => {
            let (link_type, dest_url, title, id) = allocs.take_link(link_ix);
            Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }
        }
        ItemBody::Heading(level) => Tag::Heading(level),
        ItemBody::FencedCodeBlock(cow_ix) => {
            Tag::CodeBlock(CodeBlockKind::Fenced(allocs.take_cow(cow_ix)))
        }
        ItemBody::IndentCodeBlock => Tag::CodeBlock(CodeBlockKind::Indented),
        ItemBody::MathBlock(cow_ix) => Tag::MathBlock(allocs.take_cow(cow_ix)),
        ItemBody::BlockQuote => Tag::BlockQuote,
        ItemBody::List(_, c, listitem_start) => {
            if c == b'.' || c == b')' {
                Tag::List(Some(listitem_start))
            } else {
                Tag::List(None)
            }
        }
        ItemBody::ListItem(_) => Tag::Item,
        ItemBody::TableHead => Tag::TableHead,
        ItemBody::TableCell => Tag::TableCell,
        ItemBody::TableRow => Tag::TableRow,
        ItemBody::Table(alignment_ix) => Tag::Table(allocs.take_alignment(alignment_ix)),
        ItemBody::FootnoteDefinition(cow_ix) => Tag::FootnoteDefinition(allocs.take_cow(cow_ix)),
        ItemBody::Math(cow_ix) => return Event::InlineMath(allocs.take_cow(cow_ix)),
        _ => panic!("unexpected item body {:?}", item.body),
    };

    Event::Start(tag)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::tree::Node;

    // TODO: move these tests to tests/html.rs?

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn node_size() {
        let node_size = std::mem::size_of::<Node<Item>>();
        assert_eq!(48, node_size);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn body_size() {
        let body_size = std::mem::size_of::<ItemBody>();
        assert_eq!(16, body_size);
    }

    #[test]
    fn single_open_fish_bracket() {
        // dont crash
        assert_eq!(3, Parser::new("<").into_offset_iter().count());
    }

    #[test]
    fn lone_hashtag() {
        // dont crash
        assert_eq!(2, Parser::new("#").into_offset_iter().count());
    }

    #[test]
    fn lots_of_backslashes() {
        // dont crash
        Parser::new("\\\\\r\r").into_offset_iter().count();
        Parser::new("\\\r\r\\.\\\\\r\r\\.\\")
            .into_offset_iter()
            .count();
    }

    #[test]
    fn issue_1095() {
        let s = "- [n]:Z\r\n\t\t";
        for _ in Parser::new(s).into_offset_iter() {}
    }

    #[test]
    fn issue_320() {
        // dont crash
        Parser::new(":\r\t> |\r:\r\t> |\r")
            .into_offset_iter()
            .count();
    }

    #[test]
    fn issue_319() {
        // dont crash
        Parser::new("|\r-]([^|\r-]([^").into_offset_iter().count();
        Parser::new("|\r\r=][^|\r\r=][^car")
            .into_offset_iter()
            .count();
    }

    #[test]
    fn issue_303() {
        // dont crash
        Parser::new("[^\r\ra]").into_offset_iter().count();
        Parser::new("\r\r]Z[^\x00\r\r]Z[^\x00")
            .into_offset_iter()
            .count();
    }

    #[test]
    fn issue_313() {
        // dont crash
        Parser::new("*]0[^\r\r*]0[^").into_offset_iter().count();
        Parser::new("[^\r> `][^\r> `][^\r> `][")
            .into_offset_iter()
            .count();
    }

    #[test]
    fn issue_311() {
        // dont crash
        Parser::new("\\\u{0d}-\u{09}\\\u{0d}-\u{09}")
            .into_offset_iter()
            .count();
    }

    #[test]
    fn issue_283() {
        let input = std::str::from_utf8(b"\xf0\x9b\xb2\x9f<td:^\xf0\x9b\xb2\x9f").unwrap();
        // dont crash
        Parser::new(input).into_offset_iter().count();
    }

    #[test]
    fn issue_289() {
        // dont crash
        Parser::new("> - \\\n> - ").into_offset_iter().count();
        Parser::new("- \n\n").into_offset_iter().count();
    }

    #[test]
    fn issue_306() {
        // dont crash
        Parser::new("*\r_<__*\r_<__*\r_<__*\r_<__")
            .into_offset_iter()
            .count();
    }

    #[test]
    fn issue_305() {
        // dont crash
        Parser::new("_6**6*_*").into_offset_iter().count();
    }

    #[test]
    fn another_emphasis_panic() {
        Parser::new("*__#_#__*").into_offset_iter().count();
    }

    #[test]
    fn offset_iter() {
        let event_offsets: Vec<_> = Parser::new("*hello* world")
            .into_offset_iter()
            .map(|(_ev, range)| range)
            .collect();
        let expected_offsets = vec![(0..13), (0..7), (1..6), (0..7), (7..13), (0..13)];
        assert_eq!(expected_offsets, event_offsets);
    }

    #[test]
    fn reference_link_offsets() {
        let range =
            Parser::new("# H1\n[testing][Some reference]\n\n[Some reference]: https://github.com")
                .into_offset_iter()
                .filter_map(|(ev, range)| match ev {
                    Event::Start(
                        Tag::Link {
                            link_type: LinkType::Reference,
                            ..
                        },
                        ..,
                    ) => Some(range),
                    _ => None,
                })
                .next()
                .unwrap();
        assert_eq!(5..30, range);
    }

    #[test]
    fn footnote_offsets() {
        let range = Parser::new("Testing this[^1] out.\n\n[^1]: Footnote.")
            .into_offset_iter()
            .filter_map(|(ev, range)| match ev {
                Event::FootnoteReference(..) => Some(range),
                _ => None,
            })
            .next()
            .unwrap();
        assert_eq!(12..16, range);
    }

    #[test]
    fn footnote_offsets_exclamation() {
        let mut immediately_before_footnote = None;
        let range = Parser::new("Testing this![^1] out.\n\n[^1]: Footnote.")
            .into_offset_iter()
            .filter_map(|(ev, range)| match ev {
                Event::FootnoteReference(..) => Some(range),
                _ => {
                    immediately_before_footnote = Some((ev, range));
                    None
                }
            })
            .next()
            .unwrap();
        assert_eq!(13..17, range);
        if let (Event::Text(exclamation), range_exclamation) =
            immediately_before_footnote.as_ref().unwrap()
        {
            assert_eq!("!", &exclamation[..]);
            assert_eq!(&(12..13), range_exclamation);
        } else {
            panic!("what came first, then? {immediately_before_footnote:?}");
        }
    }

    #[test]
    fn table_offset() {
        let markdown = "a\n\nTesting|This|Outtt\n--|:--:|--:\nSome Data|Other data|asdf";
        let event_offset = Parser::new(markdown)
            .into_offset_iter()
            .map(|(_ev, range)| range)
            .nth(3)
            .unwrap();
        let expected_offset = 3..59;
        assert_eq!(expected_offset, event_offset);
    }

    #[test]
    fn table_cell_span() {
        let markdown = "a|b|c\n--|--|--\na|  |c";
        let event_offset = Parser::new(markdown)
            .into_offset_iter()
            .filter_map(|(ev, span)| match ev {
                Event::Start(Tag::TableCell) => Some(span),
                _ => None,
            })
            .nth(4)
            .unwrap();
        let expected_offset_start = "a|b|c\n--|--|--\na|".len();
        assert_eq!(
            expected_offset_start..(expected_offset_start + 2),
            event_offset
        );
    }

    #[test]
    fn offset_iter_issue_378() {
        let event_offsets: Vec<_> = Parser::new("a [b](c) d")
            .into_offset_iter()
            .map(|(_ev, range)| range)
            .collect();
        let expected_offsets = vec![(0..10), (0..2), (2..8), (3..4), (2..8), (8..10), (0..10)];
        assert_eq!(expected_offsets, event_offsets);
    }

    #[test]
    fn offset_iter_issue_404() {
        let event_offsets: Vec<_> = Parser::new("###\n")
            .into_offset_iter()
            .map(|(_ev, range)| range)
            .collect();
        let expected_offsets = vec![(0..4), (0..4)];
        assert_eq!(expected_offsets, event_offsets);
    }

    #[test]
    fn code_block_kind_check_fenced() {
        let parser = Parser::new("hello\n```test\ntadam\n```");
        let mut found = 0;
        for (ev, _range) in parser.into_offset_iter() {
            match ev {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(syntax))) => {
                    assert_eq!(syntax.as_ref(), "test");
                    found += 1;
                }
                _ => {}
            }
        }
        assert_eq!(found, 1);
    }

    #[test]
    fn code_block_kind_check_indented() {
        let parser = Parser::new("hello\n\n    ```test\n    tadam\nhello");
        let mut found = 0;
        for (ev, _range) in parser.into_offset_iter() {
            match ev {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Indented)) => {
                    found += 1;
                }
                _ => {}
            }
        }
        assert_eq!(found, 1);
    }

    #[test]
    fn inline_html_inside_blockquote() {
        // Regression for #960
        let input = "> <foo\n> bar>";
        let events: Vec<_> = Parser::new(input)
            .into_offset_iter()
            .map(|(ev, _)| ev)
            .collect();
        let expected = [
            Event::Start(Tag::BlockQuote),
            Event::Start(Tag::Paragraph),
            Event::InlineHtml(CowStr::Boxed("<foo\nbar>".to_string().into())),
            Event::End(TagEnd::Paragraph),
            Event::End(TagEnd::BlockQuote),
        ];
        assert_eq!(&events, &expected);
    }
}
