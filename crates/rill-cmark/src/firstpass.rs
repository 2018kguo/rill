//! The first pass resolves all block structure, generating an AST. Within a block, items
//! are in a linear chain with potential inline markup identified.

use std::cmp::max;

use crate::parse::{scan_containers, Allocations, Item, ItemBody, LinkDef, LINK_MAX_NESTED_PARENS};
use crate::scanners::*;
use crate::strings::CowStr;
use crate::tree::{Tree, TreeIndex};
use crate::{
    linklabel::{normalize_label, scan_link_label_rest, LinkLabel},
    HeadingLevel,
};

/// Runs the first pass, which resolves the block structure of the document,
/// and returns the resulting tree.
pub(crate) fn run_first_pass(text: &str) -> (Tree<Item>, Allocations<'_>) {
    // This is a very naive heuristic for the number of nodes
    // we'll need.
    let start_capacity = max(128, text.len() / 32);
    let lookup_table = &create_lut();
    let first_pass = FirstPass {
        text,
        tree: Tree::with_capacity(start_capacity),
        begin_list_item: None,
        last_line_blank: false,
        allocs: Allocations::new(),
        lookup_table,
        paragraph_interrupted: false,
        lazy_html: None,
        literal_cache: (usize::MAX, 0, 0),
    };
    first_pass.run()
}

/// State for the first parsing pass.
struct FirstPass<'a, 'b> {
    text: &'a str,
    tree: Tree<Item>,
    begin_list_item: Option<usize>,
    last_line_blank: bool,
    allocs: Allocations<'a>,
    lookup_table: &'b LookupTable,
    /// rill: the previous line was a paragraph line in the current container, so new
    /// list items on this line follow micromark's interrupt rules.
    paragraph_interrupted: bool,
    /// rill: a lazy line where complete HTML interrupted a paragraph; micromark keeps it
    /// in the open containers.
    lazy_html: Option<usize>,
    /// rill: `in_literal_path`'s line start, scan position and last literal end.
    literal_cache: (usize, usize, usize),
}

impl<'a, 'b> FirstPass<'a, 'b> {
    fn run(mut self) -> (Tree<Item>, Allocations<'a>) {
        let mut ix = 0;
        while ix < self.text.len() {
            ix = self.parse_block(ix);
        }
        while self.tree.spine_len() > 0 {
            self.pop(ix);
        }
        (self.tree, self.allocs)
    }

    /// Returns offset after block.
    fn parse_block(&mut self, mut start_ix: usize) -> usize {
        let bytes = self.text.as_bytes();
        let mut line_start = LineStart::new(&bytes[start_ix..]);

        let i = scan_containers(&self.tree, &mut line_start);
        if self.lazy_html.take() != Some(start_ix) {
            for _ in i..self.tree.spine_len() {
                self.pop(start_ix);
            }
        }
        let interrupting = std::mem::take(&mut self.paragraph_interrupted);

        // Process new containers
        loop {
            let save = line_start.clone();
            let outer_indent = line_start.scan_space_upto(4);
            if outer_indent >= 4 {
                line_start = save;
                break;
            }
            // Footnote definitions
            let container_start = start_ix + line_start.bytes_scanned();
            if let Some(bytecount) = self.parse_footnote(container_start) {
                start_ix = container_start + bytecount;
                line_start = LineStart::new(&bytes[start_ix..]);
                continue;
            }
            let container_start = start_ix + line_start.bytes_scanned();
            let before_marker = line_start.clone();
            if let Some((ch, index, indent)) = line_start
                .scan_list_marker_with_indent(outer_indent)
                .filter(|&(ch, index, _)| {
                    // rill: an interrupting list item is non-empty and bullet or `1.`.
                    !interrupting
                        || ((!matches!(ch, b'.' | b')') || index == 1)
                            && scan_blank_line(&bytes[start_ix + line_start.bytes_scanned()..])
                                .is_none())
                })
                .or_else(|| {
                    line_start = before_marker.clone();
                    None
                })
            {
                let after_marker_index = start_ix + line_start.bytes_scanned();
                self.continue_list(container_start - outer_indent, ch, index);
                self.tree.append(Item {
                    start: container_start - outer_indent,
                    end: after_marker_index, // will get updated later if item not empty
                    body: ItemBody::ListItem(indent),
                });
                self.tree.push();
                if let Some(n) = scan_blank_line(&bytes[after_marker_index..]) {
                    self.begin_list_item = Some(after_marker_index + n);
                    return after_marker_index + n;
                }
                let before_marker = line_start.clone();
                let task_list_marker = line_start.scan_task_list_marker().map(|is_checked| Item {
                    start: after_marker_index,
                    end: start_ix + line_start.bytes_scanned(),
                    body: ItemBody::TaskListMarker(is_checked),
                });
                if let Some(task_list_marker) = task_list_marker {
                    if let Some(n) = scan_blank_line(&bytes[task_list_marker.end..]) {
                        // rill: micromark needs paragraph content after a task marker; it
                        // may start on the next line. Otherwise `[ ]` stays text.
                        let next = task_list_marker.end + n;
                        let content = self
                            .scan_next_line_or_lazy_continuation(&bytes[next..])
                            .map(|line| next + line.bytes_scanned())
                            .filter(|&ix| {
                                ix < bytes.len() && scan_blank_line(&bytes[ix..]).is_none()
                            });
                        match content {
                            Some(ix) => return self.parse_paragraph(ix, Some(task_list_marker)),
                            None => line_start = before_marker,
                        }
                    } else {
                        // rill: the marker takes one space or tab; the rest is text.
                        if !line_start.scan_ch(b' ') {
                            line_start.scan_ch(b'\t');
                        }
                        let ix = start_ix + line_start.bytes_scanned();
                        return self.parse_paragraph(ix, Some(task_list_marker));
                    }
                }
            } else if line_start.scan_blockquote_marker() {
                self.finish_list(start_ix);
                self.tree.append(Item {
                    start: container_start,
                    end: 0, // will get set later
                    body: ItemBody::BlockQuote,
                });
                self.tree.push();
            } else {
                line_start = save;
                break;
            }
        }

        let ix = start_ix + line_start.bytes_scanned();

        if let Some(n) = scan_blank_line(&bytes[ix..]) {
            if let Some(node_ix) = self.tree.peek_up() {
                match &mut self.tree[node_ix].item.body {
                    ItemBody::BlockQuote => (),
                    ItemBody::ListItem(indent) if self.begin_list_item.is_some() => {
                        self.last_line_blank = true;
                        // This is a blank list item.
                        // While the list itself can be continued no matter how many blank lines
                        // there are between this one and the next one, it cannot nest anything
                        // else, so its indentation should not be subtracted from the line start.
                        *indent = 0;
                    }
                    _ => {
                        self.last_line_blank = true;
                    }
                }
            } else {
                self.last_line_blank = true;
            }
            return ix + n;
        }

        // Save `remaining_space` here to avoid needing to backtrack `line_start` for HTML blocks
        let remaining_space = line_start.remaining_space();

        let indent = line_start.scan_space_upto(4);
        if indent == 4 {
            self.finish_list(start_ix);
            let ix = start_ix + line_start.bytes_scanned();
            let remaining_space = line_start.remaining_space();
            return self.parse_indented_code_block(ix, remaining_space);
        }

        let ix = start_ix + line_start.bytes_scanned();

        // HTML Blocks
        if bytes[ix] == b'<' {
            // Types 1-5 are all detected by one function and all end with the same
            // pattern
            if let Some(html_end_tag) = get_html_end_tag(&bytes[(ix + 1)..]) {
                self.finish_list(start_ix);
                return self.parse_html_block_type_1_to_5(
                    ix,
                    html_end_tag,
                    remaining_space,
                    indent,
                );
            }

            // Detect type 6
            if starts_html_block_type_6(&bytes[(ix + 1)..]) {
                self.finish_list(start_ix);
                return self.parse_html_block_type_6_or_7(ix, remaining_space, indent);
            }

            // Detect type 7
            if let Some(_html_bytes) = scan_html_type_7(&bytes[ix..]) {
                self.finish_list(start_ix);
                return self.parse_html_block_type_6_or_7(ix, remaining_space, indent);
            }
        }

        if let Ok(n) = scan_hrule(&bytes[ix..]) {
            self.finish_list(start_ix);
            return self.parse_hrule(n, ix);
        }

        if let Some(atx_size) = scan_atx_heading(&bytes[ix..]) {
            self.finish_list(start_ix);
            return self.parse_atx_heading(ix, atx_size);
        }

        if let Some((n, fence_ch)) = scan_code_fence(&bytes[ix..]) {
            self.finish_list(start_ix);
            return self.parse_fenced_code_block(ix, indent, fence_ch, n);
        }

        // parse refdef
        while let Some((bytecount, label, link_def)) =
            self.parse_refdef_total(start_ix + line_start.bytes_scanned())
        {
            let container_start = start_ix + line_start.bytes_scanned();
            let mut ix = container_start + bytecount;
            // rill: keep the definition in the tree, in source order.
            let label_end = self.text[container_start..]
                .find("]:")
                .map_or(container_start + 1, |at| container_start + at);
            let raw_label = &self.text[container_start + 1..label_end];
            self.finish_list(start_ix);
            let definition_ix = self.allocs.definitions.len();
            self.allocs.definitions.push((raw_label, link_def.clone()));
            self.tree.append(Item {
                start: container_start,
                end: ix,
                body: ItemBody::Definition(definition_ix),
            });
            self.allocs.refdefs.0.entry(label).or_insert(link_def);
            // Refdefs act as if they were contained within a paragraph, for purposes of lazy
            // continuations. For example:
            //
            // ```
            // > [foo]: http://example.com
            // bar
            // ```
            //
            // is equivalent to
            //
            // ```
            // > bar
            //
            // [foo]: http://example.com
            // ```
            if let Some(nl) = scan_blank_line(&bytes[ix..]) {
                ix += nl;
            } else {
                self.finish_list(start_ix);
                return ix;
            }
            if let Some(lazy_line_start) = self.scan_next_line_or_lazy_continuation(&bytes[ix..]) {
                line_start = lazy_line_start;
                start_ix = ix;
            } else {
                self.finish_list(start_ix);
                return ix;
            }
        }

        let ix = start_ix + line_start.bytes_scanned();

        self.parse_paragraph(ix, None)
    }

    /// footnote definitions and GFM quote markers can be "interrupted"
    /// like paragraphs, but otherwise can't have other blocks after them.
    ///
    /// Call this at the end of the line to parse that. If it succeeeds,
    /// this returns the LineStart for the new line.
    fn scan_next_line_or_lazy_continuation<'input>(
        &mut self,
        bytes: &'input [u8],
    ) -> Option<LineStart<'input>> {
        let mut line_start = LineStart::new(bytes);
        let tree_position = scan_containers(&self.tree, &mut line_start);
        let current_container = tree_position == self.tree.spine_len();
        // rill: a blank line ends the content however far it is indented.
        let mut rest = line_start.clone();
        rest.scan_all_space();
        if rest.is_at_eol() {
            return None;
        }
        if !line_start.scan_space(4)
            && self
                .scan_paragraph_interrupt(&bytes[line_start.bytes_scanned()..], current_container)
        {
            // rill: micromark's definitions are content, so the next line's containers
            // open under the interrupt rules, as after a paragraph.
            self.paragraph_interrupted = current_container;
            None
        } else {
            line_start.scan_all_space();
            Some(line_start)
        }
    }

    /// Returns the offset of the first line after the table.
    /// Assumptions: current focus is a table element and the table header
    /// matches the separator line (same number of columns).
    fn parse_table(
        &mut self,
        table_cols: usize,
        head_start: usize,
        body_start: usize,
    ) -> Option<usize> {
        // filled empty cells are limited to protect against quadratic growth
        // https://github.com/raphlinus/pulldown-cmark/issues/832
        let mut missing_empty_cells = 0;
        // parse header. this shouldn't fail because we made sure the table header is ok
        let (_sep_start, thead_ix) =
            self.parse_table_row_inner(head_start, table_cols, &mut missing_empty_cells)?;
        self.tree[thead_ix].item.body = ItemBody::TableHead;

        // parse body
        let mut ix = body_start;
        while let Some((next_ix, _row_ix)) =
            self.parse_table_row(ix, table_cols, &mut missing_empty_cells)
        {
            ix = next_ix;
        }

        self.pop(ix);
        Some(ix)
    }

    /// Call this when containers are taken care of.
    /// Returns bytes scanned, row_ix
    fn parse_table_row_inner(
        &mut self,
        mut ix: usize,
        row_cells: usize,
        missing_empty_cells: &mut usize,
    ) -> Option<(usize, TreeIndex)> {
        // Limit to prevent a malicious input from causing a denial of service.
        const MAX_AUTOCOMPLETED_CELLS: usize = 1 << 18; // = 0x40000

        let bytes = self.text.as_bytes();
        let mut cells = 0;
        let mut final_cell_ix = None;

        let old_cur = self.tree.cur();
        let row_ix = self.tree.append(Item {
            start: ix,
            end: 0, // set at end of this function
            body: ItemBody::TableRow,
        });
        self.tree.push();

        let row_start = ix;
        loop {
            ix += scan_ch(&bytes[ix..], b'|');
            let start_ix = ix;
            ix += scan_whitespace_no_nl(&bytes[ix..]);

            if let Some(eol_bytes) = scan_eol(&bytes[ix..]) {
                // rill: micromark reads a lone `|` line as a row with one empty cell.
                if cells == 0 && start_ix > row_start {
                    self.tree.append(Item {
                        start: start_ix,
                        end: ix,
                        body: ItemBody::TableCell,
                    });
                    cells = 1;
                    if cells == row_cells {
                        final_cell_ix = self.tree.cur();
                    }
                }
                ix += eol_bytes;
                break;
            }

            let cell_ix = self.tree.append(Item {
                start: start_ix,
                end: ix,
                body: ItemBody::TableCell,
            });
            self.tree.push();
            let (next_ix, _brk) = self.parse_line(ix, None, TableParseMode::Active);

            self.tree[cell_ix].item.end = next_ix;
            self.tree.pop();

            ix = next_ix;
            cells += 1;

            if cells == row_cells {
                final_cell_ix = Some(cell_ix);
            }
        }

        if let (Some(cur), 0) = (old_cur, cells) {
            self.pop(ix);
            self.tree[cur].next = None;
            return None;
        }

        // rill: MDAST keeps only written cells; mdast-util-to-hast fills missing ones.
        let _ = (missing_empty_cells, MAX_AUTOCOMPLETED_CELLS);

        // rill: MDAST keeps excess cells; mdast-util-to-hast drops them.
        let _ = final_cell_ix;

        self.pop(ix);

        Some((ix, row_ix))
    }

    /// Returns first offset after the row and the tree index of the row.
    fn parse_table_row(
        &mut self,
        mut ix: usize,
        row_cells: usize,
        missing_empty_cells: &mut usize,
    ) -> Option<(usize, TreeIndex)> {
        let bytes = self.text.as_bytes();
        let mut line_start = LineStart::new(&bytes[ix..]);
        let tree_position = scan_containers(&self.tree, &mut line_start);
        let current_container = tree_position == self.tree.spine_len();
        if !current_container {
            return None;
        }
        // rill: an indented line (code) ends the table.
        if line_start.clone().scan_space(4) {
            return None;
        }
        line_start.scan_all_space();
        ix += line_start.bytes_scanned();
        if scan_paragraph_interrupt_no_table(&bytes[ix..], current_container, &self.tree) {
            return None;
        }
        // rill: a table is no paragraph (micromark's `_gfmTableDynamicInterruptHack`), so
        // any list item and complete HTML end it as well.
        let rest = &bytes[ix..];
        if scan_listitem(rest).is_some()
            || rest.first() == Some(&b'<') && scan_html_type_7(rest).is_some()
        {
            return None;
        }

        let (ix, row_ix) = self.parse_table_row_inner(ix, row_cells, missing_empty_cells)?;
        Some((ix, row_ix))
    }

    /// Returns offset of line start after paragraph.
    fn parse_paragraph(&mut self, start_ix: usize, tasklist_marker: Option<Item>) -> usize {
        self.finish_list(start_ix);
        let node_ix = self.tree.append(Item {
            start: start_ix,
            end: 0, // will get set later
            body: ItemBody::Paragraph,
        });
        self.tree.push();

        if let Some(item) = tasklist_marker {
            self.tree.append(item);
        }

        let bytes = self.text.as_bytes();
        let mut ix = start_ix;
        // rill: the tree position before the last line break and the previous line's end.
        let mut previous_line: Option<(Option<TreeIndex>, usize, Option<usize>)> = None;
        // rill: the current line's start, before its container prefixes.
        let mut line_begin = start_ix;
        loop {
            // rill: micromark's delimiter row takes any paragraph line as its head.
            let (next_ix, brk) = self.parse_line(ix, None, TableParseMode::Scan);

            if let (
                Some(Item {
                    body: ItemBody::Table(alignment_ix),
                    ..
                }),
                Some((before_break, paragraph_end, backslash)),
            ) = (brk, previous_line)
            {
                // The lines before stay a paragraph ending with the previous line, where a
                // trailing backslash is text. The head line then parses afresh, where a
                // block that starts there (HTML of any kind, a heading, a fence, a break)
                // comes before the table.
                let table_cols = self.allocs[alignment_ix].len();
                self.tree.truncate_after(before_break);
                if let Some(pos) = backslash {
                    self.tree.append_text(pos, pos + 1, false);
                }
                self.pop(paragraph_end);
                if starts_block(&bytes[ix..]) {
                    return line_begin;
                }
                self.tree.append(Item {
                    start: ix,
                    end: 0, // set by parse_table
                    body: ItemBody::Table(alignment_ix),
                });
                self.tree.push();
                if let Some(ix) = self.parse_table(table_cols, ix, next_ix) {
                    return ix;
                }
            }

            // break out when we find a table
            if let Some(Item {
                body: ItemBody::Table(alignment_ix),
                ..
            }) = brk
            {
                let table_cols = self.allocs[alignment_ix].len();
                self.tree[node_ix].item.body = ItemBody::Table(alignment_ix);
                // this clears out any stuff we may have appended - but there may
                // be a cleaner way
                self.tree[node_ix].child = None;
                self.tree.pop();
                self.tree.push();
                if let Some(ix) = self.parse_table(table_cols, ix, next_ix) {
                    return ix;
                }
            }

            ix = next_ix;
            let mut line_start = LineStart::new(&bytes[ix..]);
            let tree_position = scan_containers(&self.tree, &mut line_start);
            let current_container = tree_position == self.tree.spine_len();
            let trailing_backslash_pos = match brk {
                Some(Item {
                    start,
                    body: ItemBody::HardBreak(true),
                    ..
                }) if bytes[start] == b'\\' => Some(start),
                _ => None,
            };
            if !line_start.scan_space(4) {
                let ix_new = ix + line_start.bytes_scanned();
                if current_container {
                    if let Some(ix_setext) =
                        self.parse_setext_heading(ix_new, node_ix, trailing_backslash_pos.is_some())
                    {
                        if let Some(pos) = trailing_backslash_pos {
                            self.tree.append_text(pos, pos + 1, false);
                        }
                        self.pop(ix_setext);
                        return ix_setext;
                    }
                }
                // first check for non-empty lists, then for other interrupts
                let suffix = &bytes[ix_new..];
                if self.scan_paragraph_interrupt(suffix, current_container) {
                    if let Some(pos) = trailing_backslash_pos {
                        self.tree.append_text(pos, pos + 1, false);
                    }
                    self.paragraph_interrupted = current_container;
                    // Only complete HTML (type 7) not written with EOF.
                    let complete_html = suffix.strip_prefix(b"<").is_some_and(|tag| {
                        !starts_html_block_type_6(tag)
                            && get_html_end_tag(tag).is_none()
                            && scan_html_type_7(suffix).is_some()
                    });
                    if complete_html
                        && !current_container
                        && suffix.iter().any(|b| matches!(b, b'\n' | b'\r'))
                    {
                        self.lazy_html = Some(ix);
                    }
                    break;
                }
            }
            line_start.scan_all_space();
            if line_start.is_at_eol() {
                if let Some(pos) = trailing_backslash_pos {
                    self.tree.append_text(pos, pos + 1, false);
                }
                break;
            }
            line_begin = next_ix;
            ix = next_ix + line_start.bytes_scanned();
            if let Some(item) = brk {
                let mut line_end = item.end;
                while line_end > item.start && matches!(bytes[line_end - 1], b'\n' | b'\r') {
                    line_end -= 1;
                }
                previous_line = Some((self.tree.cur(), line_end, trailing_backslash_pos));
                self.tree.append(item);
            }
        }

        self.pop(ix);
        ix
    }

    /// rill: whether the backslash at `ix` lies in a GFM literal autolink's path, which
    /// micromark's tokenizer claims before any escape; `from` is the line's start. Scans
    /// each line's candidates once.
    /// micromark's `previousUnbalanced` blocks literals after an open `[` or `![`; every
    /// `]` closes the latest one, link or not.
    /// ponytail: brackets in code spans still count; spans are not resolved yet.
    fn in_literal_path(&mut self, from: usize, ix: usize) -> bool {
        if self.literal_cache.0 != from {
            self.literal_cache = (from, from, from);
        }
        let (_, mut scanned, mut end) = self.literal_cache;
        let text = self.text;
        let bytes = text.as_bytes();
        let atext = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.' | b'_');
        // Candidates: protocol and www starts, and email starts (an atext run's first byte).
        let candidate = |at: usize| {
            matches!(bytes[at], b'h' | b'H' | b'w' | b'W')
                || atext(bytes[at]) && (at == from || !atext(bytes[at - 1]))
        };
        while scanned < ix && end <= ix {
            let Some(at) = (scanned..ix).find(|&at| candidate(at)) else {
                scanned = ix;
                break;
            };
            let previous = text[..at].chars().next_back().filter(|_| at > from);
            match crate::autolink_literal::scan(text, at, previous) {
                Some((literal_end, _)) if !self.bracket_open(at) => {
                    end = literal_end;
                    scanned = literal_end.max(at + 1);
                }
                _ => scanned = at + 1,
            }
        }
        self.literal_cache = (from, scanned, end);
        end > ix
    }

    /// Whether a `[` or `![` before `at` in the current block is still open.
    fn bracket_open(&self, at: usize) -> bool {
        let Some(block) = self.tree.peek_up() else {
            return false;
        };
        let mut open = 0usize;
        let mut node = self.tree[block].child;
        while let Some(ix) = node.filter(|&ix| self.tree[ix].item.start < at) {
            match self.tree[ix].item.body {
                ItemBody::MaybeLinkOpen | ItemBody::MaybeImage => open += 1,
                ItemBody::MaybeLinkClose(..) => open = open.saturating_sub(1),
                _ => {}
            }
            node = self.tree[ix].next;
        }
        open > 0
    }

    /// Returns end ix of setext_heading on success.
    fn parse_setext_heading(
        &mut self,
        ix: usize,
        node_ix: TreeIndex,
        has_trailing_content: bool,
    ) -> Option<usize> {
        let bytes = self.text.as_bytes();
        let (n, level) = scan_setext_heading(&bytes[ix..])?;

        if let Some(cur_ix) = self.tree.cur() {
            let parent_ix = self.tree.peek_up().unwrap();
            let header_start = self.tree[parent_ix].item.start;
            // Note that `self.tree[parent_ix].item.end` might be zero at this point.
            // Use the end position of the current node (i.e. the last known child
            // of the parent) instead.
            let header_end = self.tree[cur_ix].item.end;

            // strip trailing whitespace
            if !has_trailing_content {
                let trailing_ws =
                    scan_rev_while(&bytes[header_start..header_end], is_ascii_whitespace_no_nl);
                self.tree[cur_ix].item.end = header_end - trailing_ws;
            }
        }

        self.tree[node_ix].item.body = ItemBody::Heading(level);

        Some(ix + n)
    }

    /// Parse a line of input, appending text and items to tree.
    ///
    /// Returns: index after line and an item representing the break.
    fn parse_line(
        &mut self,
        start: usize,
        end: Option<usize>,
        mode: TableParseMode,
    ) -> (usize, Option<Item>) {
        let bytes = self.text.as_bytes();
        let bytes = match end {
            Some(end) => &bytes[..end],
            None => bytes,
        };
        let bytes_len = bytes.len();
        let mut pipes = 0;
        let mut last_pipe_ix = start;
        let mut begin_text = start;
        let mut backslash_escaped = false;

        let (final_ix, brk) = iterate_special_bytes(self.lookup_table, bytes, start, |ix, byte| {
            match byte {
                b'\n' | b'\r' => {
                    if let TableParseMode::Active = mode {
                        return LoopInstruction::BreakAtWith(ix, None);
                    }

                    let mut i = ix;
                    let eol_bytes = scan_eol(&bytes[ix..]).unwrap();

                    let end_ix = ix + eol_bytes;
                    // rill: micromark accepts a pipeless one-column header row, and tries a
                    // table head row before a trailing backslash break.
                    if mode == TableParseMode::Scan {
                        // check if we may be parsing a table
                        let next_line_ix = ix + eol_bytes;
                        let mut line_start = LineStart::new(&bytes[next_line_ix..]);
                        if scan_containers(&self.tree, &mut line_start) == self.tree.spine_len() {
                            let table_head_ix = next_line_ix + line_start.bytes_scanned();
                            // rill: `- x` there is a list item, which micromark opens first.
                            let head = &bytes[table_head_ix..];
                            let indent = head.iter().take_while(|b| **b == b' ').count();
                            let list_item = indent < 4
                                && head.get(indent) == Some(&b'-')
                                && matches!(head.get(indent + 1), Some(b' ' | b'\t'))
                                && scan_blank_line(&head[indent + 2..]).is_none();
                            let (table_head_bytes, alignment) = if list_item {
                                (0, Vec::new())
                            } else {
                                scan_table_head(head)
                            };

                            if table_head_bytes > 0 {
                                // computing header count from number of pipes
                                let header_count = if pipes == 0 {
                                    1
                                } else {
                                    count_header_cols(bytes, pipes, start, last_pipe_ix)
                                };

                                // make sure they match the number of columns we find in separator line
                                if alignment.len() == header_count {
                                    let alignment_ix = self.allocs.allocate_alignment(alignment);
                                    let end_ix = table_head_ix + table_head_bytes;
                                    return LoopInstruction::BreakAtWith(
                                        end_ix,
                                        Some(Item {
                                            start: i,
                                            end: end_ix, // must update later
                                            body: ItemBody::Table(alignment_ix),
                                        }),
                                    );
                                }
                            }
                        }
                    }

                    let trailing_backslashes = scan_rev_while(&bytes[..ix], |b| b == b'\\');
                    if trailing_backslashes % 2 == 1 && end_ix < bytes_len {
                        i -= 1;
                        self.tree.append_text(begin_text, i, backslash_escaped);
                        backslash_escaped = false;
                        return LoopInstruction::BreakAtWith(
                            end_ix,
                            Some(Item {
                                start: i,
                                end: end_ix,
                                body: ItemBody::HardBreak(true),
                            }),
                        );
                    }

                    let trailing_whitespace =
                        scan_rev_while(&bytes[..ix], is_ascii_whitespace_no_nl);
                    // rill: micromark's trailing hard break is two or more spaces, no tabs.
                    let tabs = bytes[ix - trailing_whitespace..ix].contains(&b'\t');
                    if trailing_whitespace >= 2 && !tabs {
                        i -= trailing_whitespace;
                        self.tree.append_text(begin_text, i, backslash_escaped);
                        backslash_escaped = false;
                        return LoopInstruction::BreakAtWith(
                            end_ix,
                            Some(Item {
                                start: i,
                                end: end_ix,
                                body: ItemBody::HardBreak(false),
                            }),
                        );
                    }

                    self.tree
                        .append_text(begin_text, ix - trailing_whitespace, backslash_escaped);
                    backslash_escaped = false;

                    LoopInstruction::BreakAtWith(
                        end_ix,
                        Some(Item {
                            start: i,
                            end: end_ix,
                            body: ItemBody::SoftBreak,
                        }),
                    )
                }
                b'\\' => {
                    if ix + 1 < bytes_len
                        && is_ascii_punctuation(bytes[ix + 1])
                        && !self.in_literal_path(start, ix)
                    {
                        self.tree.append_text(begin_text, ix, backslash_escaped);
                        if bytes[ix + 1] == b'$' && bytes.get(ix + 2) == Some(&b'$') {
                            let count = 1 + scan_ch_repeat(&bytes[(ix + 2)..], b'$');
                            self.tree.append(Item {
                                start: ix + 1,
                                end: ix + count + 1,
                                body: ItemBody::MaybeMathText(count, true),
                            });
                            begin_text = ix + 1 + count;
                            backslash_escaped = false;
                            LoopInstruction::ContinueAndSkip(count)
                        } else if bytes[ix + 1] == b'`' {
                            let count = 1 + scan_ch_repeat(&bytes[(ix + 2)..], b'`');
                            self.tree.append(Item {
                                start: ix + 1,
                                end: ix + count + 1,
                                body: ItemBody::MaybeCode(count, true),
                            });
                            begin_text = ix + 1 + count;
                            backslash_escaped = false;
                            LoopInstruction::ContinueAndSkip(count)
                        } else if bytes[ix + 1] == b'|' && TableParseMode::Active == mode {
                            // Yeah, it's super weird that backslash escaped pipes in tables aren't "real"
                            // backslash escapes.
                            //
                            // This tree structure is intended for the benefit of inline analysis, and it
                            // is supposed to operate as-if backslash escaped pipes were stripped out in a
                            // separate pass.
                            begin_text = ix + 1;
                            backslash_escaped = false;
                            LoopInstruction::ContinueAndSkip(1)
                        } else {
                            // rill: micromark reads `\\|` as an escaped backslash, then a divider.
                            begin_text = ix + 1;
                            backslash_escaped = true;
                            LoopInstruction::ContinueAndSkip(1)
                        }
                    } else {
                        LoopInstruction::ContinueAndSkip(0)
                    }
                }
                c @ b'*' | c @ b'_' | c @ b'~' => {
                    // rill: remark-cjk-friendly flanking (see `attention`).
                    let string_suffix = &self.text[ix..];
                    let count = 1 + scan_ch_repeat(&string_suffix.as_bytes()[1..], c);
                    let mut before = self.text[start..ix].chars().rev();
                    let previous = before.next();
                    let two_previous = before.next();
                    let next = string_suffix[count..].chars().next();
                    let flanking = if c == b'~' {
                        crate::attention::strikethrough(count, two_previous, previous, next)
                    } else {
                        Some(crate::attention::emphasis(c, two_previous, previous, next))
                    };
                    // A run that can neither open nor close still orders the resolvers.
                    if let Some((can_open, can_close)) = flanking {
                        self.tree.append_text(begin_text, ix, backslash_escaped);
                        backslash_escaped = false;
                        for i in 0..count {
                            self.tree.append(Item {
                                start: ix + i,
                                end: ix + i + 1,
                                body: ItemBody::MaybeEmphasis(count - i, can_open, can_close),
                            });
                        }
                        begin_text = ix + count;
                    }
                    LoopInstruction::ContinueAndSkip(count - 1)
                }
                b'$' => {
                    // rill: micromark-extension-math text with `singleDollarTextMath: false`
                    // is a code span of `$$`+ runs; a lone `$` is text.
                    let count = 1 + scan_ch_repeat(&bytes[(ix + 1)..], b'$');
                    if count >= 2 {
                        self.tree.append_text(begin_text, ix, backslash_escaped);
                        backslash_escaped = false;
                        self.tree.append(Item {
                            start: ix,
                            end: ix + count,
                            body: ItemBody::MaybeMathText(count, false),
                        });
                        begin_text = ix + count;
                    }
                    LoopInstruction::ContinueAndSkip(count - 1)
                }
                b'`' => {
                    self.tree.append_text(begin_text, ix, backslash_escaped);
                    backslash_escaped = false;
                    let count = 1 + scan_ch_repeat(&bytes[(ix + 1)..], b'`');
                    self.tree.append(Item {
                        start: ix,
                        end: ix + count,
                        body: ItemBody::MaybeCode(count, false),
                    });
                    begin_text = ix + count;
                    LoopInstruction::ContinueAndSkip(count - 1)
                }
                b'<' if bytes.get(ix + 1) != Some(&b'\\') => {
                    // Note: could detect some non-HTML cases and early escape here, but not
                    // clear that's a win.
                    self.tree.append_text(begin_text, ix, backslash_escaped);
                    backslash_escaped = false;
                    self.tree.append(Item {
                        start: ix,
                        end: ix + 1,
                        body: ItemBody::MaybeHtml,
                    });
                    begin_text = ix + 1;
                    LoopInstruction::ContinueAndSkip(0)
                }
                b'!' => {
                    if ix + 1 < bytes_len && bytes[ix + 1] == b'[' {
                        self.tree.append_text(begin_text, ix, backslash_escaped);
                        backslash_escaped = false;
                        self.tree.append(Item {
                            start: ix,
                            end: ix + 2,
                            body: ItemBody::MaybeImage,
                        });
                        begin_text = ix + 2;
                        LoopInstruction::ContinueAndSkip(1)
                    } else {
                        LoopInstruction::ContinueAndSkip(0)
                    }
                }
                b'[' => {
                    self.tree.append_text(begin_text, ix, backslash_escaped);
                    backslash_escaped = false;
                    self.tree.append(Item {
                        start: ix,
                        end: ix + 1,
                        body: ItemBody::MaybeLinkOpen,
                    });
                    begin_text = ix + 1;
                    LoopInstruction::ContinueAndSkip(0)
                }
                b']' => {
                    self.tree.append_text(begin_text, ix, backslash_escaped);
                    backslash_escaped = false;
                    self.tree.append(Item {
                        start: ix,
                        end: ix + 1,
                        body: ItemBody::MaybeLinkClose(true),
                    });
                    begin_text = ix + 1;
                    LoopInstruction::ContinueAndSkip(0)
                }
                b'&' => match scan_entity(&bytes[ix..]) {
                    (n, Some(value)) => {
                        self.tree.append_text(begin_text, ix, backslash_escaped);
                        backslash_escaped = false;
                        self.tree.append(Item {
                            start: ix,
                            end: ix + n,
                            body: ItemBody::SynthesizeText(self.allocs.allocate_cow(value)),
                        });
                        begin_text = ix + n;
                        LoopInstruction::ContinueAndSkip(n - 1)
                    }
                    _ => LoopInstruction::ContinueAndSkip(0),
                },
                b'|' => {
                    // rill: escaped only by an odd run of backslashes.
                    if scan_rev_while(&bytes[..ix], |b| b == b'\\') % 2 == 1 {
                        LoopInstruction::ContinueAndSkip(0)
                    } else if let TableParseMode::Active = mode {
                        LoopInstruction::BreakAtWith(ix, None)
                    } else {
                        last_pipe_ix = ix;
                        pipes += 1;
                        LoopInstruction::ContinueAndSkip(0)
                    }
                }
                _ => LoopInstruction::ContinueAndSkip(0),
            }
        });

        if brk.is_none() {
            let trailing_whitespace =
                scan_rev_while(&bytes[begin_text..final_ix], is_ascii_whitespace_no_nl);
            // need to close text at eof
            self.tree.append_text(
                begin_text,
                final_ix - trailing_whitespace,
                backslash_escaped,
            );
        }
        (final_ix, brk)
    }

    /// When start_ix is at the beginning of an HTML block of type 1 to 5,
    /// this will find the end of the block, adding the block itself to the
    /// tree and also keeping track of the lines of HTML within the block.
    ///
    /// The html_end_tag is the tag that must be found on a line to end the block.
    fn parse_html_block_type_1_to_5(
        &mut self,
        start_ix: usize,
        html_end_tag: &str,
        mut remaining_space: usize,
        mut indent: usize,
    ) -> usize {
        self.tree.append(Item {
            start: start_ix,
            end: 0, // set later
            body: ItemBody::HtmlBlock,
        });
        self.tree.push();

        let bytes = self.text.as_bytes();
        let mut ix = start_ix;
        let end_ix;
        loop {
            let line_start_ix = ix;
            ix += scan_nextline(&bytes[ix..]);
            self.append_html_line(remaining_space.max(indent), line_start_ix, ix);

            let mut line_start = LineStart::new(&bytes[ix..]);
            let n_containers = scan_containers(&self.tree, &mut line_start);
            if n_containers < self.tree.spine_len() {
                end_ix = ix;
                break;
            }

            if self.text[line_start_ix..ix].contains(html_end_tag) {
                end_ix = ix;
                break;
            }

            let next_line_ix = ix + line_start.bytes_scanned();
            if next_line_ix == self.text.len() {
                end_ix = next_line_ix;
                break;
            }
            ix = next_line_ix;
            remaining_space = line_start.remaining_space();
            indent = 0;
        }
        self.pop(end_ix);
        ix
    }

    /// When start_ix is at the beginning of an HTML block of type 6 or 7,
    /// this will consume lines until there is a blank line and keep track of
    /// the HTML within the block.
    fn parse_html_block_type_6_or_7(
        &mut self,
        start_ix: usize,
        mut remaining_space: usize,
        mut indent: usize,
    ) -> usize {
        self.tree.append(Item {
            start: start_ix,
            end: 0, // set later
            body: ItemBody::HtmlBlock,
        });
        self.tree.push();

        let bytes = self.text.as_bytes();
        let mut ix = start_ix;
        let end_ix;
        loop {
            let line_start_ix = ix;
            ix += scan_nextline(&bytes[ix..]);
            self.append_html_line(remaining_space.max(indent), line_start_ix, ix);

            let mut line_start = LineStart::new(&bytes[ix..]);
            let n_containers = scan_containers(&self.tree, &mut line_start);
            if n_containers < self.tree.spine_len() || line_start.is_at_eol() {
                end_ix = ix;
                break;
            }

            let next_line_ix = ix + line_start.bytes_scanned();
            if next_line_ix == self.text.len() || scan_blank_line(&bytes[next_line_ix..]).is_some()
            {
                end_ix = next_line_ix;
                break;
            }
            ix = next_line_ix;
            remaining_space = line_start.remaining_space();
            indent = 0;
        }
        self.pop(end_ix);
        ix
    }

    fn parse_indented_code_block(&mut self, start_ix: usize, mut remaining_space: usize) -> usize {
        self.tree.append(Item {
            start: start_ix,
            end: 0, // will get set later
            body: ItemBody::IndentCodeBlock,
        });
        self.tree.push();
        let bytes = self.text.as_bytes();
        let mut last_nonblank_child = None;
        let mut last_nonblank_ix = 0;
        let mut end_ix = 0;
        self.last_line_blank = false;

        let mut ix = start_ix;
        loop {
            let line_start_ix = ix;
            ix += scan_nextline(&bytes[ix..]);
            self.append_code_text(remaining_space, line_start_ix, ix);
            // TODO(spec clarification): should we synthesize newline at EOF?

            if !self.last_line_blank {
                last_nonblank_child = self.tree.cur();
                last_nonblank_ix = ix;
                end_ix = ix;
            }

            let mut line_start = LineStart::new(&bytes[ix..]);
            let n_containers = scan_containers(&self.tree, &mut line_start);
            if n_containers < self.tree.spine_len()
                || !(line_start.scan_space(4) || line_start.is_at_eol())
            {
                // rill: micromark's flow is still inside the indented code for the next
                // line's containers, so new list items follow the interrupt rules.
                self.paragraph_interrupted = n_containers == self.tree.spine_len();
                break;
            }
            let next_line_ix = ix + line_start.bytes_scanned();
            if next_line_ix == self.text.len() {
                break;
            }
            ix = next_line_ix;
            remaining_space = line_start.remaining_space();
            self.last_line_blank = scan_blank_line(&bytes[ix..]).is_some();
        }

        // Trim trailing blank lines.
        if let Some(child) = last_nonblank_child {
            self.tree[child].next = None;
            self.tree[child].item.end = last_nonblank_ix;
        }
        self.pop(end_ix);
        ix
    }

    fn parse_fenced_code_block(
        &mut self,
        start_ix: usize,
        indent: usize,
        fence_ch: u8,
        n_fence_char: usize,
    ) -> usize {
        let bytes = self.text.as_bytes();
        let mut info_start = start_ix + n_fence_char;
        info_start += scan_whitespace_no_nl(&bytes[info_start..]);
        // TODO: info strings are typically very short. wouldn't it be faster
        // to just do a forward scan here?
        let mut ix = info_start + scan_nextline(&bytes[info_start..]);
        let info_end = ix - scan_rev_while(&bytes[info_start..ix], is_ascii_whitespace);
        let info_string = unescape(&self.text[info_start..info_end], self.tree.is_in_table());
        self.tree.append(Item {
            start: start_ix,
            end: 0, // will get set later
            body: if fence_ch == b'$' {
                ItemBody::MathBlock(self.allocs.allocate_cow(info_string))
            } else {
                ItemBody::FencedCodeBlock(self.allocs.allocate_cow(info_string))
            },
        });
        self.tree.push();
        loop {
            let mut line_start = LineStart::new(&bytes[ix..]);
            let n_containers = scan_containers(&self.tree, &mut line_start);
            if n_containers < self.tree.spine_len() {
                // this line will get parsed again as not being part of the code
                // if it's blank, it should be parsed as a blank line
                self.pop(ix);
                return ix;
            }
            line_start.scan_space(indent);
            let mut close_line_start = line_start.clone();
            if !close_line_start.scan_space(4 - indent) {
                let close_ix = ix + close_line_start.bytes_scanned();
                if let Some(n) = scan_closing_code_fence(&bytes[close_ix..], fence_ch, n_fence_char)
                {
                    ix = close_ix + n;
                    self.pop(ix);
                    // try to read trailing whitespace or it will register as a completely blank line
                    return ix + scan_blank_line(&bytes[ix..]).unwrap_or(0);
                }
            }
            let remaining_space = line_start.remaining_space();
            ix += line_start.bytes_scanned();
            let next_ix = ix + scan_nextline(&bytes[ix..]);
            self.append_code_text(remaining_space, ix, next_ix);
            ix = next_ix;
        }
    }

    fn append_code_text(&mut self, remaining_space: usize, start: usize, end: usize) {
        if remaining_space > 0 {
            let cow_ix = self.allocs.allocate_cow("   "[..remaining_space].into());
            self.tree.append(Item {
                start,
                end: start,
                body: ItemBody::SynthesizeText(cow_ix),
            });
        }
        // rill: MDAST keeps raw line endings in code values.
        self.tree.append_text(start, end, false);
    }

    /// Appends a line of HTML to the tree.
    fn append_html_line(&mut self, remaining_space: usize, start: usize, end: usize) {
        if remaining_space > 0 {
            let cow_ix = self.allocs.allocate_cow("   "[..remaining_space].into());
            self.tree.append(Item {
                start,
                end: start,
                body: ItemBody::SynthesizeText(cow_ix),
            });
        }
        // rill: MDAST keeps raw line endings in HTML values.
        self.tree.append(Item {
            start,
            end,
            body: ItemBody::Html,
        });
    }

    /// Pop a container, setting its end.
    fn pop(&mut self, ix: usize) {
        let cur_ix = self.tree.pop().unwrap();
        self.tree[cur_ix].item.end = ix;
        if let ItemBody::List(true, _, _) = self.tree[cur_ix].item.body {
            surgerize_tight_list(&mut self.tree, cur_ix);
            self.begin_list_item = None;
        }
    }

    /// Close a list if it's open. Also set loose if last line was blank
    /// and end current list if it's a lone, empty item
    fn finish_list(&mut self, ix: usize) {
        self.finish_empty_list_item();
        if let Some(node_ix) = self.tree.peek_up() {
            if let ItemBody::List(_, _, _) = self.tree[node_ix].item.body {
                self.pop(ix);
            }
        }
        if self.last_line_blank {
            if let Some(node_ix) = self.tree.peek_grandparent() {
                if let ItemBody::List(ref mut is_tight, _, _) = self.tree[node_ix].item.body {
                    *is_tight = false;
                }
            }
            self.last_line_blank = false;
        }
    }

    fn finish_empty_list_item(&mut self) {
        if let Some(begin_list_item) = self.begin_list_item {
            if self.last_line_blank {
                // A list item can begin with at most one blank line.
                if let Some(node_ix) = self.tree.peek_up() {
                    if let ItemBody::ListItem(_) = self.tree[node_ix].item.body {
                        self.pop(begin_list_item);
                    }
                }
            }
        }
        self.begin_list_item = None;
    }

    /// Continue an existing list or start a new one if there's not an open
    /// list that matches.
    fn continue_list(&mut self, start: usize, ch: u8, index: u64) {
        self.finish_empty_list_item();
        if let Some(node_ix) = self.tree.peek_up() {
            if let ItemBody::List(ref mut is_tight, existing_ch, _) = self.tree[node_ix].item.body {
                if existing_ch == ch {
                    if self.last_line_blank {
                        *is_tight = false;
                        self.last_line_blank = false;
                    }
                    return;
                }
            }
            // TODO: this is not the best choice for end; maybe get end from last list item.
            self.finish_list(start);
        }
        self.tree.append(Item {
            start,
            end: 0, // will get set later
            body: ItemBody::List(true, ch, index),
        });
        self.tree.push();
        self.last_line_blank = false;
    }

    /// Parse a thematic break.
    ///
    /// Returns index of start of next line.
    fn parse_hrule(&mut self, hrule_size: usize, ix: usize) -> usize {
        self.tree.append(Item {
            start: ix,
            end: ix + hrule_size,
            body: ItemBody::Rule,
        });
        ix + hrule_size
    }

    /// Parse an ATX heading.
    ///
    /// Returns index of start of next line.
    fn parse_atx_heading(&mut self, start: usize, atx_level: HeadingLevel) -> usize {
        let mut ix = start;
        let heading_ix = self.tree.append(Item {
            start,
            end: 0,                    // set later
            body: ItemBody::default(), // set later
        });
        ix += atx_level as usize;
        // next char is space or eol (guaranteed by scan_atx_heading)
        let bytes = self.text.as_bytes();
        if let Some(eol_bytes) = scan_eol(&bytes[ix..]) {
            self.tree[heading_ix].item.end = ix + eol_bytes;
            self.tree[heading_ix].item.body = ItemBody::Heading(atx_level);
            return ix + eol_bytes;
        }
        // skip leading spaces
        let skip_spaces = scan_whitespace_no_nl(&bytes[ix..]);
        ix += skip_spaces;

        // now handle the header text
        let header_start = ix;
        let header_node_idx = self.tree.push(); // so that we can set the endpoint later

        let (line_ix, line_brk) = self.parse_line(ix, None, TableParseMode::Disabled);
        ix = line_ix;
        // Backslash at end is actually hard line break
        if let Some(Item {
            start,
            end,
            body: ItemBody::HardBreak(true),
        }) = line_brk
        {
            self.tree.append_text(start, end, false);
        }
        self.tree[header_node_idx].item.end = ix;

        // remove trailing matter from header text
        let mut empty_text_node = false;
        if let Some(cur_ix) = self.tree.cur() {
            // remove closing of the ATX heading
            let header_text = &bytes[header_start..ix];
            let mut limit = header_text
                .iter()
                .rposition(|&b| !(b == b'\n' || b == b'\r' || b == b' '))
                .map_or(0, |i| i + 1);
            let closer = header_text[..limit]
                .iter()
                .rposition(|&b| b != b'#')
                .map_or(0, |i| i + 1);
            if closer == 0 {
                limit = closer;
            } else {
                let spaces = scan_rev_while(&header_text[..closer], |b| b == b' ');
                if spaces > 0 {
                    limit = closer - spaces;
                }
            }
            // if text is only spaces, then remove them
            self.tree[cur_ix].item.end = limit + header_start;

            // limit = 0 when text is empty after removing spaces
            if limit == 0 {
                empty_text_node = true;
            }
        }

        if empty_text_node {
            self.tree.remove_node();
        } else {
            self.tree.pop();
        }
        self.tree[heading_ix].item.body = ItemBody::Heading(atx_level);

        ix
    }

    /// Returns the number of bytes scanned on success.
    fn parse_footnote(&mut self, start: usize) -> Option<usize> {
        let bytes = &self.text.as_bytes()[start..];
        if !bytes.starts_with(b"[^") {
            return None;
        }
        // GitHub doesn't allow footnote definition labels to contain line breaks.
        // It actually does allow this for link definitions under certain circumstances,
        // but for this it's simpler to avoid it.
        let (mut i, label) =
            scan_link_label_rest(&self.text[start + 2..], &|_| None, self.tree.is_in_table())?;
        if label.bytes().any(|b| b == b'\r' || b == b'\n') {
            // GitHub doesn't allow footnote definition labels to contain line breaks,
            // even if they're escaped.
            return None;
        }
        i += 2;
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        self.finish_list(start);
        if let Some(node_ix) = self.tree.peek_up() {
            if let ItemBody::FootnoteDefinition(..) = self.tree[node_ix].item.body {
                // finish previous footnote if it's still open
                self.pop(start);
            }
        }
        i += scan_whitespace_no_nl(&bytes[i..]);
        self.allocs.footdefs.0.insert(normalize_label(&label));
        self.tree.append(Item {
            start,
            end: 0, // will get set later
            // TODO: check whether the label here is strictly necessary
            body: ItemBody::FootnoteDefinition(self.allocs.allocate_cow(label)),
        });
        self.tree.push();
        Some(i)
    }

    /// Tries to parse a reference label, which can be interrupted by new blocks.
    /// On success, returns the number of bytes of the label and the label itself.
    fn parse_refdef_label(&self, start: usize) -> Option<(usize, CowStr<'a>)> {
        scan_link_label_rest(
            &self.text[start..],
            &|bytes| {
                let mut line_start = LineStart::new(bytes);
                let tree_position = scan_containers(&self.tree, &mut line_start);
                let current_container = tree_position == self.tree.spine_len();
                if line_start.scan_space(4) {
                    return Some(line_start.bytes_scanned());
                }
                let bytes_scanned = line_start.bytes_scanned();
                let suffix = &bytes[bytes_scanned..];
                if self.scan_paragraph_interrupt(suffix, current_container)
                    || (current_container && scan_setext_heading(suffix).is_some())
                {
                    None
                } else {
                    Some(bytes_scanned)
                }
            },
            self.tree.is_in_table(),
        )
    }

    /// Returns number of bytes scanned, label and definition on success.
    fn parse_refdef_total(&mut self, start: usize) -> Option<(usize, LinkLabel<'a>, LinkDef<'a>)> {
        let bytes = &self.text.as_bytes()[start..];
        if bytes.get(0) != Some(&b'[') {
            return None;
        }
        let (mut i, label) = self.parse_refdef_label(start + 1)?;
        i += 1;
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        let (bytecount, link_def) = self.scan_refdef(start + i)?;
        Some((bytecount + i, normalize_label(&label), link_def))
    }

    /// Returns number of bytes and number of newlines
    fn scan_refdef_space(&self, bytes: &[u8], mut i: usize) -> Option<(usize, usize)> {
        let mut newlines = 0;
        loop {
            let whitespaces = scan_whitespace_no_nl(&bytes[i..]);
            i += whitespaces;
            if let Some(eol_bytes) = scan_eol(&bytes[i..]) {
                i += eol_bytes;
                newlines += 1;
                if newlines > 1 {
                    return None;
                }
            } else {
                break;
            }
            let mut line_start = LineStart::new(&bytes[i..]);
            let tree_position = scan_containers(&self.tree, &mut line_start);
            let current_container = tree_position == self.tree.spine_len();
            if !line_start.scan_space(4) {
                let suffix = &bytes[i + line_start.bytes_scanned()..];
                if self.scan_paragraph_interrupt(suffix, current_container)
                    || scan_setext_heading(suffix).is_some()
                {
                    return None;
                }
            }
            i += line_start.bytes_scanned();
        }
        Some((i, newlines))
    }

    // returns (bytelength, title_str)
    fn scan_refdef_title<'t>(&self, text: &'t str) -> Option<(usize, CowStr<'t>)> {
        let bytes = text.as_bytes();
        let closing_delim = match bytes.first()? {
            b'\'' => b'\'',
            b'"' => b'"',
            b'(' => b')',
            _ => return None,
        };
        let mut bytecount = 1;
        let mut linestart = 1;

        let mut linebuf = None;

        while let Some(&c) = bytes.get(bytecount) {
            match c {
                b'(' if closing_delim == b')' => {
                    // https://spec.commonmark.org/0.30/#link-title
                    // a sequence of zero or more characters between matching parentheses ((...)),
                    // including a ( or ) character only if it is backslash-escaped.
                    return None;
                }
                b'\n' | b'\r' => {
                    // push text to line buffer
                    // this is used to strip the block formatting:
                    //
                    // > [first]: http://example.com "
                    // > second"
                    //
                    // should get turned into `<a href="example.com" title=" second">first</a>`
                    let linebuf = if let Some(linebuf) = &mut linebuf {
                        linebuf
                    } else {
                        linebuf = Some(String::new());
                        linebuf.as_mut().unwrap()
                    };
                    linebuf.push_str(&text[linestart..bytecount]);
                    linebuf.push('\n'); // normalize line breaks
                                        // skip line break
                    bytecount += 1;
                    if c == b'\r' && bytes.get(bytecount) == Some(&b'\n') {
                        bytecount += 1;
                    }
                    let mut line_start = LineStart::new(&bytes[bytecount..]);
                    let tree_position = scan_containers(&self.tree, &mut line_start);
                    let current_container = tree_position == self.tree.spine_len();
                    if !line_start.scan_space(4) {
                        let suffix = &bytes[bytecount + line_start.bytes_scanned()..];
                        if self.scan_paragraph_interrupt(suffix, current_container)
                            || scan_setext_heading(suffix).is_some()
                        {
                            return None;
                        }
                    }
                    line_start.scan_all_space();
                    bytecount += line_start.bytes_scanned();
                    linestart = bytecount;
                    if scan_blank_line(&bytes[bytecount..]).is_some() {
                        // blank line - not allowed
                        return None;
                    }
                }
                b'\\' => {
                    bytecount += 1;
                    if let Some(c) = bytes.get(bytecount) {
                        if c != &b'\r' && c != &b'\n' {
                            bytecount += 1;
                        }
                    }
                }
                c if c == closing_delim => {
                    let cow = if let Some(mut linebuf) = linebuf {
                        linebuf.push_str(&text[linestart..bytecount]);
                        CowStr::from(linebuf)
                    } else {
                        CowStr::from(&text[linestart..bytecount])
                    };
                    return Some((bytecount + 1, cow));
                }
                _ => {
                    bytecount += 1;
                }
            }
        }
        None
    }

    /// Returns # of bytes and definition.
    /// Assumes the label of the reference including colon has already been scanned.
    fn scan_refdef(&self, start: usize) -> Option<(usize, LinkDef<'a>)> {
        let bytes = self.text.as_bytes();

        // whitespace between label and url (including up to one newline)
        let (mut i, _newlines) = self.scan_refdef_space(bytes, start)?;

        // scan link dest
        let (dest_length, dest) = scan_link_dest(self.text, i, LINK_MAX_NESTED_PARENS)?;
        if dest_length == 0 {
            return None;
        }
        let dest = unescape(dest, self.tree.is_in_table());
        i += dest_length;

        // no title
        let mut backup = (i - start, LinkDef { dest, title: None });

        // scan whitespace between dest and label
        let (mut i, newlines) =
            if let Some((new_i, mut newlines)) = self.scan_refdef_space(bytes, i) {
                if i == self.text.len() {
                    newlines += 1;
                }
                if new_i == i && newlines == 0 {
                    return None;
                }
                if newlines > 1 {
                    return Some(backup);
                };
                (new_i, newlines)
            } else {
                return Some(backup);
            };

        // scan title
        // if this fails but newline == 1, return also a refdef without title
        if let Some((title_length, title)) = self.scan_refdef_title(&self.text[i..]) {
            i += title_length;
            if scan_blank_line(&bytes[i..]).is_some() {
                backup.0 = i - start;
                backup.1.title = Some(unescape(title, self.tree.is_in_table()));
                return Some(backup);
            }
        }
        if newlines > 0 {
            Some(backup)
        } else {
            None
        }
    }

    /// Checks whether we should break a paragraph on the given input.
    fn scan_paragraph_interrupt(&self, bytes: &[u8], current_container: bool) -> bool {
        if scan_paragraph_interrupt_no_table(bytes, current_container, &self.tree) {
            return true;
        }
        // pulldown-cmark allows heavy tables, that have a `|` on the header row,
        // to interrupt paragraphs.
        //
        // ```markdown
        // This is a table
        // | a | b | c |
        // |---|---|---|
        // | d | e | f |
        //
        // This is not a table
        //  a | b | c
        // ---|---|---
        //  d | e | f
        // ```
        if !bytes.starts_with(b"|") {
            return false;
        }

        // Checking if something's a valid table or not requires looking at two lines.
        // First line, count unescaped pipes.
        let mut pipes = 0;
        let mut next_line_ix = 0;
        let mut bsesc = false;
        let mut last_pipe_ix = 0;
        for (i, &byte) in bytes.iter().enumerate() {
            match byte {
                b'\\' => {
                    bsesc = true;
                    continue;
                }
                b'|' if !bsesc => {
                    pipes += 1;
                    last_pipe_ix = i;
                }
                b'\r' | b'\n' => {
                    next_line_ix = i + scan_eol(&bytes[i..]).unwrap();
                    break;
                }
                _ => {}
            }
            bsesc = false;
        }

        // scan_eol can't return 0, so this can't be zero
        if next_line_ix == 0 {
            return false;
        }

        // Scan the table head. The part that looks like:
        //
        //     |---|---|---|
        //
        // Also scan any containing items, since it's on its own line, and
        // might be nested inside a block quote or something
        //
        //     > Table: First
        //     > | first col | second col |
        //     > |-----------|------------|
        //     ^
        //     | need to skip over the `>` when checking for the table
        let mut line_start = LineStart::new(&bytes[next_line_ix..]);
        if scan_containers(&self.tree, &mut line_start) != self.tree.spine_len() {
            return false;
        }
        let table_head_ix = next_line_ix + line_start.bytes_scanned();
        let (table_head_bytes, alignment) = scan_table_head(&bytes[table_head_ix..]);

        if table_head_bytes == 0 {
            return false;
        }

        // computing header count from number of pipes
        let header_count = count_header_cols(bytes, pipes, 0, last_pipe_ix);

        // make sure they match the number of columns we find in separator line
        alignment.len() == header_count
    }
}

/// Scanning modes for `Parser`'s `parse_line` method.
#[derive(PartialEq, Eq, Copy, Clone)]
enum TableParseMode {
    /// Inside a paragraph, scanning for table headers.
    Scan,
    /// Inside a table.
    Active,
    /// Inside a paragraph, not scanning for table headers.
    Disabled,
}

/// Computes the number of header columns in a table line by computing the number of dividing pipes
/// that aren't followed or preceded by whitespace.
fn count_header_cols(
    bytes: &[u8],
    mut pipes: usize,
    mut start: usize,
    last_pipe_ix: usize,
) -> usize {
    // was first pipe preceded by whitespace? if so, subtract one
    start += scan_whitespace_no_nl(&bytes[start..]);
    if bytes[start] == b'|' {
        pipes -= 1;
    }

    // was last pipe followed by whitespace? if so, sub one
    if scan_blank_line(&bytes[(last_pipe_ix + 1)..]).is_some() {
        pipes
    } else {
        pipes + 1
    }
}

/// rill: whether a block other than a paragraph or table starts at `bytes` (a line start
/// past container prefixes).
fn starts_block(bytes: &[u8]) -> bool {
    let indent = bytes.iter().take_while(|b| **b == b' ').count();
    if indent > 3 {
        return false;
    }
    let line = &bytes[indent..];
    scan_atx_heading(line).is_some()
        || scan_code_fence(line).is_some()
        || scan_hrule(line).is_ok()
        || line.strip_prefix(b"<").is_some_and(|tag| {
            get_html_end_tag(tag).is_some()
                || starts_html_block_type_6(tag)
                || scan_html_type_7(line).is_some()
        })
}

/// Checks whether we should break a paragraph on the given input.
///
/// Use `FirstPass::scan_paragraph_interrupt` in any context that allows
/// tables to interrupt the paragraph.
fn scan_paragraph_interrupt_no_table(
    bytes: &[u8],
    current_container: bool,
    tree: &Tree<Item>,
) -> bool {
    scan_eol(bytes).is_some()
        || scan_hrule(bytes).is_ok()
        || scan_atx_heading(bytes).is_some()
        || scan_code_fence(bytes).is_some()
        || scan_blockquote_start(bytes).is_some()
        || scan_listitem(bytes).map_or(false, |(ix, delim, index, _)| {
            ! current_container ||
            tree.is_in_table() ||
            // we don't allow interruption by either empty lists or
            // numbered lists starting at an index other than 1
            (delim == b'*' || delim == b'-' || delim == b'+' || index == 1)
                && (scan_blank_line(&bytes[ix..]).is_none())
        })
        || bytes.starts_with(b"<")
            && (get_html_end_tag(&bytes[1..]).is_some()
                || starts_html_block_type_6(&bytes[1..])
                // rill: micromark lets complete HTML interrupt on a lazy line.
                || !current_container && scan_html_type_7(bytes).is_some())
        || (bytes.starts_with(b"[^")
            && scan_link_label_rest(
                std::str::from_utf8(&bytes[2..]).unwrap(),
                &|_| None,
                tree.is_in_table(),
            )
            .map_or(false, |(len, _)| bytes.get(2 + len) == Some(&b':')))
}

/// Assumes `text_bytes` is preceded by `<`.
fn get_html_end_tag(text_bytes: &[u8]) -> Option<&'static str> {
    static BEGIN_TAGS: &[&[u8]; 4] = &[b"pre", b"style", b"script", b"textarea"];
    static ST_BEGIN_TAGS: &[&[u8]; 3] = &[b"!--", b"?", b"![CDATA["];

    for (beg_tag, end_tag) in BEGIN_TAGS
        .iter()
        .zip(["</pre>", "</style>", "</script>", "</textarea>"].iter())
    {
        let tag_len = beg_tag.len();

        if text_bytes.len() < tag_len {
            // begin tags are increasing in size
            break;
        }

        if !text_bytes[..tag_len].eq_ignore_ascii_case(beg_tag) {
            continue;
        }

        // Must either be the end of the line...
        if text_bytes.len() == tag_len {
            return Some(end_tag);
        }

        // ...or be followed by whitespace, newline, or '>'.
        let s = text_bytes[tag_len];
        if is_ascii_whitespace(s) || s == b'>' {
            return Some(end_tag);
        }
    }

    for (beg_tag, end_tag) in ST_BEGIN_TAGS.iter().zip(["-->", "?>", "]]>"].iter()) {
        if text_bytes.starts_with(beg_tag) {
            return Some(end_tag);
        }
    }

    if text_bytes.len() > 1 && text_bytes[0] == b'!' && text_bytes[1].is_ascii_alphabetic() {
        Some(">")
    } else {
        None
    }
}

fn surgerize_tight_list(tree: &mut Tree<Item>, list_ix: TreeIndex) {
    let mut list_item = tree[list_ix].child;
    while let Some(listitem_ix) = list_item {
        let mut node_ix = tree[listitem_ix].child;
        while let Some(node) = node_ix {
            if let ItemBody::Paragraph = tree[node].item.body {
                tree[node].item.body = ItemBody::TightParagraph;
            }
            node_ix = tree[node].next;
        }

        list_item = tree[listitem_ix].next;
    }
}

fn create_lut() -> LookupTable {
    let mut bytes = [false; 256];
    let standard_bytes = [
        b'\n', b'\r', b'*', b'_', b'&', b'\\', b'[', b']', b'<', b'!', b'`', b'|', b'~', b'$',
    ];

    for &byte in &standard_bytes {
        bytes[byte as usize] = true;
    }

    bytes
}

enum LoopInstruction<T> {
    /// Continue looking for more special bytes, but skip next few bytes.
    ContinueAndSkip(usize),
    /// Break looping immediately, returning with the given index and value.
    BreakAtWith(usize, T),
}

type LookupTable = [bool; 256];

/// This function walks the byte slices from the given index and
/// calls the callback function on all bytes (and their indices) that are in the following set:
/// `` ` ``, `\`, `&`, `*`, `_`, `~`, `!`, `<`, `[`, `]`, `|`, `\r`, `\n`
/// It is guaranteed not call the callback on other bytes.
/// Whenever `callback(ix, byte)` returns a `ContinueAndSkip(n)` value, the callback
/// will not be called with an index that is less than `ix + n + 1`.
/// When the callback returns a `BreakAtWith(end_ix, opt+val)`, no more callbacks will be
/// called and the function returns immediately with the return value `(end_ix, opt_val)`.
/// If `BreakAtWith(..)` is never returned, this function will return the first
/// index that is outside the byteslice bound and a `None` value.
fn iterate_special_bytes<F, T>(
    lut: &LookupTable,
    bytes: &[u8],
    mut ix: usize,
    mut callback: F,
) -> (usize, Option<T>)
where
    F: FnMut(usize, u8) -> LoopInstruction<Option<T>>,
{
    while ix < bytes.len() {
        let b = bytes[ix];
        if lut[b as usize] {
            match callback(ix, b) {
                LoopInstruction::ContinueAndSkip(skip) => {
                    ix += skip;
                }
                LoopInstruction::BreakAtWith(ix, val) => {
                    return (ix, val);
                }
            }
        }
        ix += 1;
    }

    (ix, None)
}
