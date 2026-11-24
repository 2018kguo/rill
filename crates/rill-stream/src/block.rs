//! Blocks: CommonMark's block structure as micromark parses it (block quotes, lists and
//! their items, paragraphs, headings, thematic breaks), then Streamdown's DOM for it.
//!
//! One pass over the lines builds flat records: each line goes through the open
//! containers, may open new ones, and adds its content to the deepest open block. A
//! second pass writes the tokens, because a list's looseness (whether its items' content
//! sits in `<p>`) is known only once it closes.

use crate::inline::Inline;
use crate::tokens::{Out, TAGS, kind, tag};
use crate::{
    Fallback, NL, Result, SP, Split, ascii, atx, list_marker, may_define, newline, run, setext,
    thematic,
};

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Root,
    Quote,
    List {
        ordered: bool,
        marker: u8,
        start: u32,
    },
    /// `column`: where its content starts, from where the line's content within the
    /// item's container starts. `empty`: its marker line has no content.
    Item {
        column: usize,
        empty: bool,
        task: u8,
    },
    Para,
    Heading(u8),
    Rule,
    /// Fenced code: its language and meta (ranges of the source, empty when absent).
    /// Its content lines are `lines`.
    Code {
        lang: (u32, u32),
        meta: (u32, u32),
    },
    /// A table: its header and body rows are `lines`; its columns' alignments a range
    /// of `aligns`.
    Table {
        aligns: (u32, u32),
    },
    /// A footnote definition (`[^label]:`, at the top level), its label a range of the
    /// source. Its content continues on lines indented 4 columns.
    Footnote {
        label: (u32, u32),
    },
}

struct Node {
    kind: Kind,
    first: u32,
    last: u32,
    next: u32,
    /// Where the line it starts on starts, and where its content starts.
    line: usize,
    start: usize,
    /// Where its content ends (its last line, or its last child's end).
    end: usize,
    /// A paragraph's or heading's lines: a range of `lines`.
    lines: (u32, u32),
    /// A list: the point the line that ended it ended it at (the text's end if none
    /// did), from which `container_exit` finds its exit.
    exit: usize,
}

struct Parser<'s> {
    source: &'s [u16],
    nodes: Vec<Node>,
    /// Content lines of paragraphs and headings, from the end of their container
    /// prefixes (a paragraph's first line from its first character) to their end.
    lines: Vec<(usize, usize)>,
    /// Open containers, the root first.
    stack: Vec<u32>,
    /// The open paragraph (in the deepest open container).
    para: Option<u32>,
    /// The line just before was blank.
    blank: bool,
    /// The open fenced code: its node, fence character and length, and indent.
    fence: Option<(u32, u16, usize, usize)>,
    /// The open table, and its columns' alignments (0 none, 1 left, 2 right, 3 center).
    table: Option<u32>,
    aligns: Vec<u8>,
    /// The open paragraph's last line was lazy (it cannot be a table's header).
    lazy: bool,
    /// The open indented code, and the blank lines since its last line (its content if
    /// more code follows).
    indented: Option<u32>,
    blanks: Vec<(usize, usize)>,
    /// The text is one block with its definitions and footnotes (see `Options::whole`).
    whole: bool,
    /// Link definitions: normalized label, destination.
    links: Vec<(Vec<u16>, String, Option<String>)>,
    /// The line before was a definition: micromark's content goes on, so this line
    /// continues it as a paragraph's would (an indented line is its text).
    def_open: bool,
}

impl<'s> Parser<'s> {
    fn add(&mut self, parent: u32, kind: Kind, line: usize, start: usize) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            kind,
            first: NONE,
            last: NONE,
            next: NONE,
            line,
            start,
            end: start,
            lines: (0, 0),
            exit: 0,
        });
        let last = self.nodes[parent as usize].last;
        if last == NONE {
            self.nodes[parent as usize].first = id;
        } else {
            self.nodes[last as usize].next = id;
        }
        self.nodes[parent as usize].last = id;
        id
    }

    fn deepest(&self) -> u32 {
        *self.stack.last().expect("the root")
    }

    /// Closes the open containers from `depth` on (and the paragraph), the line at `line`
    /// ending them.
    fn close_to(&mut self, depth: usize, line: usize) {
        self.para = None;
        self.fence = None;
        self.table = None;
        self.indented = None;
        self.blanks.clear();
        while self.stack.len() > depth {
            let id = self.stack.pop().expect("open") as usize;
            if let Kind::List { .. } = self.nodes[id].kind {
                self.nodes[id].exit = line;
            }
            self.extend(id);
        }
    }

    /// A container's end reaches its last child's.
    fn extend(&mut self, id: usize) {
        let last = self.nodes[id].last;
        if last != NONE {
            let end = end_of(self.source, &self.nodes[last as usize]);
            self.nodes[id].end = self.nodes[id].end.max(end);
        }
    }

    fn line(&mut self, line_start: usize, line: &[u16]) -> Result {
        let def_open = std::mem::take(&mut self.def_open);
        let mut pos = 0;
        // 1. The open containers this line continues.
        let mut matched = 1;
        while matched < self.stack.len() {
            let id = self.stack[matched] as usize;
            match self.nodes[id].kind {
                Kind::Quote => {
                    let spaces = run(&line[pos..], SP);
                    if spaces <= 3 && line.get(pos + spaces) == Some(&u16::from(b'>')) {
                        // A quote ends with the last line that continues it.
                        self.nodes[id].end = line_start + line.len();
                        pos += spaces + 1;
                        if line.get(pos) == Some(&SP) {
                            pos += 1;
                        }
                    } else {
                        break;
                    }
                }
                Kind::List { .. } => {}
                Kind::Item { column, empty, .. } => {
                    let rest = &line[pos..];
                    if rest.iter().all(|&unit| unit == SP) {
                        // An item can start with at most one blank line.
                        if empty
                            && self.nodes[id].first == NONE
                            && self.blank_after_marker(id, line_start)
                        {
                            break;
                        }
                        // Its indent, as far as the line has it (code shows the rest).
                        pos += rest.len().min(column);
                    } else if run(rest, SP) >= column {
                        pos += column;
                    } else {
                        break;
                    }
                }
                Kind::Footnote { .. } => {
                    let rest = &line[pos..];
                    if rest.iter().all(|&unit| unit == SP) {
                        pos += rest.len().min(4);
                    } else if run(rest, SP) >= 4 {
                        pos += 4;
                    } else {
                        break;
                    }
                }
                _ => unreachable!("containers only"),
            }
            matched += 1;
        }
        // Open fenced code takes the line when every container continues, as content or
        // its closing fence; else it ends.
        if let Some((code, fence, length, indent)) = self.fence {
            if matched == self.stack.len() {
                let rest = &line[pos..];
                let spaces = run(rest, SP);
                let marks = run(&rest[spaces..], fence);
                let end = line_start + line.len();
                let node = &mut self.nodes[code as usize];
                node.end = end;
                if spaces <= 3
                    && marks >= length
                    && rest[spaces + marks..].iter().all(|&unit| unit == SP)
                {
                    self.fence = None;
                } else {
                    node.lines.1 += 1;
                    self.lines
                        .push((line_start + pos + spaces.min(indent), end));
                }
                return Ok(());
            }
            self.fence = None;
        }
        // 2. New containers. micromark tests list starts against an open paragraph only
        // when every open container continued (then for the whole line, even inside a
        // quote the line opens); a line that leaves a container may start any list.
        let all = matched == self.stack.len();
        if def_open && !all && !line[pos..].iter().all(|&unit| unit == SP) {
            return Err(Fallback); // ponytail: a lazy line after a definition
        }
        let interrupting = all && (self.para.is_some() || self.indented.is_some() || def_open);
        let mut closed = all;
        let mut opened = false;
        loop {
            let spaces = run(&line[pos..], SP);
            let rest = &line[pos + spaces..];
            if rest.is_empty() || spaces >= 4 {
                break;
            }
            // A footnote definition: a container at the top level, interrupting a paragraph.
            if self.whole
                && rest.len() > 2
                && rest[0] == u16::from(b'[')
                && rest[1] == u16::from(b'^')
            {
                let label = rest[2..]
                    .iter()
                    .take_while(|&&unit| crate::inline::note_unit(unit))
                    .count();
                if rest.get(2 + label) == Some(&u16::from(b']'))
                    && rest.get(3 + label) == Some(&u16::from(b':'))
                {
                    // (A list with no item continued ends here too.)
                    let nested = self.stack[1..matched]
                        .iter()
                        .any(|&id| !matches!(self.nodes[id as usize].kind, Kind::List { .. }));
                    if !(1..=200).contains(&label) || nested || opened {
                        return Err(Fallback); // ponytail: nested, or a long label
                    }
                    self.close_to(1, line_start + pos);
                    closed = true;
                    let at = line_start + pos + spaces;
                    let id = self.add(
                        0,
                        Kind::Footnote {
                            label: ((at + 2) as u32, (at + 2 + label) as u32),
                        },
                        line_start,
                        at,
                    );
                    self.stack.push(id);
                    opened = true;
                    // (The spaces after `]:` are the content's indent, as micromark counts it.)
                    pos += spaces + label + 4;
                    let after = run(&line[pos..], SP);
                    if after >= 4 && pos + after < line.len() {
                        return Err(Fallback); // ponytail: indented code on the marker line
                    }
                    continue;
                }
                if rest[2..].iter().any(|&unit| unit == u16::from(b']'))
                    && rest
                        .windows(2)
                        .any(|pair| pair == [u16::from(b']'), u16::from(b':')])
                {
                    return Err(Fallback); // ponytail: other labels
                }
            }
            let quote = rest[0] == u16::from(b'>');
            let marker = if quote { None } else { list_marker(rest)? };
            if !quote {
                let Some(marker) = &marker else { break };
                if matches!(ascii(rest[0]), b'*' | b'-' | b'_') && thematic(rest) {
                    break; // a thematic break wins over an item
                }
                // ponytail: in a text kept whole, an item that is also a delimiter row
                // under a paragraph (micromark tries the table first).
                if self.whole && self.para.is_some() && delimiter_row(rest).is_some() {
                    return Err(Fallback);
                }
                // An empty item, or an ordered one not starting at 1, does not interrupt
                // a paragraph.
                if interrupting
                    && !(marker.content.is_some() && (!marker.ordered || marker.start == 1))
                {
                    break;
                }
            }
            // A new container ends those this line does not continue, at the end of the
            // prefixes the line continues.
            if !closed {
                self.close_to(matched, line_start + pos);
                closed = true;
            }
            self.para = None;
            self.table = None;
            self.indented = None;
            self.blanks.clear();
            if quote {
                self.close_lists_above(line_start);
                let parent = self.deepest();
                let id = self.add(parent, Kind::Quote, line_start, line_start + pos + spaces);
                self.nodes[id as usize].end = line_start + line.len();
                self.stack.push(id);
                pos += spaces + 1;
                if line.get(pos) == Some(&SP) {
                    pos += 1;
                }
                opened = true;
                continue;
            }
            let marker = marker.expect("an item");
            let kind = Kind::List {
                ordered: marker.ordered,
                marker: marker.marker,
                start: marker.start,
            };
            // The item joins the open list of its kind, or starts a list.
            let deepest = self.deepest() as usize;
            let list = match self.nodes[deepest].kind {
                Kind::List {
                    ordered, marker: m, ..
                } if ordered == marker.ordered && m == marker.marker => deepest as u32,
                other => {
                    if let Kind::List { .. } = other {
                        self.close_to(self.stack.len() - 1, line_start);
                    }
                    let parent = self.deepest();
                    let list = self.add(parent, kind, line_start, line_start + pos + spaces);
                    self.stack.push(list);
                    list
                }
            };
            let empty = marker.content.is_none();
            let item = self.add(
                list,
                Kind::Item {
                    column: spaces + marker.width,
                    empty,
                    task: 0,
                },
                line_start,
                line_start + pos + spaces,
            );
            self.stack.push(item);
            opened = true;
            match marker.content {
                Some(at) => {
                    pos += spaces + at;
                    // A task (`[ ]`, `[x]`, then a space and content): its paragraph holds
                    // the rest of the line, from that space. A definition: the exact path.
                    let mark = line.get(pos + 1).map(|&unit| ascii(unit));
                    let checkbox = line[pos] == u16::from(b'[')
                        && line.get(pos + 2) == Some(&u16::from(b']'))
                        && matches!(mark, Some(b' ' | b'x' | b'X'));
                    if line[pos] == u16::from(b'[')
                        && !checkbox
                        && line
                            .windows(2)
                            .any(|pair| pair == [u16::from(b']'), u16::from(b':')])
                    {
                        return Err(Fallback); // ponytail: maybe a definition
                    }
                    // `[x]` then content is a task; then nothing more in the block, or other
                    // text right after, it is text; then content on a later line (rare), a
                    // task too: the exact path.
                    let blank_rest = checkbox && line[pos + 3..].iter().all(|&unit| unit == SP);
                    let block_rest =
                        &self.source[(line_start + line.len()).min(self.source.len())..];
                    if checkbox
                        && blank_rest
                        && !block_rest.iter().all(|&unit| unit == SP || unit == NL)
                    {
                        return Err(Fallback);
                    }
                    if checkbox && !blank_rest && line.get(pos + 3) == Some(&SP) {
                        if let Kind::Item { task, .. } = &mut self.nodes[item as usize].kind {
                            *task = if mark == Some(b' ') { 1 } else { 2 };
                        }
                        let para = self.add(item, Kind::Para, line_start, line_start + pos + 3);
                        self.para = Some(para);
                        self.lazy = false;
                        self.blank = false;
                        self.para_line(para, line_start + pos + 3, line_start + line.len());
                        return Ok(());
                    }
                }
                None => {
                    pos = line.len();
                    break;
                }
            }
        }
        if !closed {
            // No new container: a lazy line continues the paragraph, unless it starts a
            // block.
            let rest = &line[pos..];
            if let Some(para) = self.para
                && !rest.iter().all(|&unit| unit == SP)
                && !self.interrupts(rest)?
            {
                if setext(&rest[run(rest, SP)..]).is_some() {
                    return Err(Fallback); // ponytail: a lazy `===`/`---` line
                }
                // ponytail: a table may start on a lazy line (its delimiter row next).
                let next = line_start + line.len() + 1;
                if next <= self.source.len() {
                    let after = &self.source[next..newline(self.source, next)];
                    let skip = after
                        .iter()
                        .take_while(|&&unit| unit == SP || unit == u16::from(b'>'))
                        .count();
                    if delimiter_row(&after[skip..]).is_some() {
                        return Err(Fallback);
                    }
                }
                self.para_line(para, line_start + pos, line_start + line.len());
                self.lazy = true;
                self.blank = false;
                return Ok(());
            }
            self.close_to(matched, line_start);
        }
        // 3. The leaf.
        let spaces = run(&line[pos..], SP);
        let rest = &line[pos + spaces..];
        // (At the top level only, micromark keeps a whitespace line's spaces past four
        // columns as code; in a container the line is blank.)
        if rest.is_empty() && !(self.indented.is_some() && spaces > 4 && self.stack.len() == 1) {
            if self.indented.is_some() {
                // A blank line is code if more code follows.
                self.blanks.push((line_start, line_start));
            }
            self.para = None;
            self.table = None;
            self.blank = true;
            return Ok(());
        }
        if let Some(code) = self.indented {
            // Past four columns of indent, the rest (spaces too) is code.
            if spaces >= 4 {
                let end = line_start + line.len();
                let node = &mut self.nodes[code as usize];
                node.lines.1 += 1 + self.blanks.len() as u32;
                node.end = end;
                self.lines.append(&mut self.blanks);
                self.lines.push((line_start + pos + 4, end));
                return Ok(());
            }
            self.indented = None;
            self.blanks.clear();
        }
        // An open table takes the line as a row unless it starts another block.
        if let Some(table) = self.table {
            if spaces < 4 && !table_ends(rest)? {
                if rest.contains(&u16::from(b'\\')) {
                    return Err(Fallback); // ponytail: Streamdown shows escaped pipes oddly
                }
                let node = &mut self.nodes[table as usize];
                node.lines.1 += 1;
                node.end = line_start + line.len();
                self.lines
                    .push((line_start + pos + spaces, line_start + line.len()));
                return Ok(());
            }
            self.table = None;
        }
        let content = line_start + pos + spaces;
        let end = line_start + line.len();
        if def_open && !opened && setext(rest).is_some() {
            return Err(Fallback); // ponytail: an underline after a definition
        }
        if spaces >= 4 {
            match self.para {
                Some(para) if !opened => self.para_line(para, line_start + pos, end),
                None if def_open && !opened => return Err(Fallback), // ponytail: an indented line after a definition
                _ => {
                    if self.whole {
                        return Err(Fallback); // ponytail: indented code in a text kept whole
                    }
                    // Indented code (a list with no item open ends on this line).
                    let ends_list =
                        matches!(self.nodes[self.deepest() as usize].kind, Kind::List { .. });
                    self.close_lists_above(line_start);
                    self.para = None;
                    let parent = self.deepest();
                    let code = self.add(
                        parent,
                        Kind::Code {
                            lang: (0, 0),
                            meta: (0, 0),
                        },
                        line_start,
                        line_start + pos + 4,
                    );
                    let n = self.lines.len() as u32;
                    self.nodes[code as usize].lines = (n, n + 1);
                    self.nodes[code as usize].end = end;
                    self.lines.push((line_start + pos + 4, end));
                    // micromark: code on a line that closed containers is that line alone
                    // (and interrupts nothing).
                    self.indented = (all && !ends_list).then_some(code);
                }
            }
            self.blank = false;
            return Ok(());
        }
        self.blank = false;
        // A delimiter row makes the paragraph's last line a table's header, when their
        // cells are as many.
        if !opened
            && !self.lazy
            && let Some(para) = self.para
            && let Some(aligns) = delimiter_row(rest)
        {
            let (start, end_of_header) = self.lines[self.lines.len() - 1];
            let header = &self.source[start..end_of_header];
            if header.contains(&u16::from(b'\\')) || rest.contains(&u16::from(b'\\')) {
                return Err(Fallback); // ponytail: escaped pipes
            }
            if self.whole && split_cells(header).len() != aligns.len() {
                return Err(Fallback); // ponytail: micromark ends the paragraph there
            }
            if split_cells(header).len() == aligns.len() {
                if run(header, SP) >= 4 {
                    // micromark: the table ends the paragraph there, then fails, and the
                    // line is indented code.
                    return Err(Fallback);
                }
                let from = self.aligns.len() as u32;
                self.aligns.extend_from_slice(&aligns);
                let kind = Kind::Table {
                    aligns: (from, self.aligns.len() as u32),
                };
                let n = self.lines.len() as u32;
                let node = &self.nodes[para as usize];
                let table = if node.lines.1 - node.lines.0 == 1 {
                    self.nodes[para as usize].kind = kind;
                    para
                } else {
                    // The paragraph keeps its other lines.
                    let previous = self.lines[self.lines.len() - 2].1;
                    let node = &mut self.nodes[para as usize];
                    node.lines.1 -= 1;
                    node.end = previous;
                    let header_line = self.source[..start]
                        .iter()
                        .rposition(|&unit| unit == NL)
                        .map_or(0, |at| at + 1);
                    let table = self.add(parent_of_para(&self.stack), kind, header_line, start);
                    self.nodes[table as usize].lines = (n - 1, n);
                    table
                };
                self.nodes[table as usize].end = end;
                self.para = None;
                self.table = Some(table);
                return Ok(());
            }
        }
        if !opened
            && let Some(para) = self.para
            && let Some(level) = setext(rest)
        {
            // The heading ends with its underline.
            self.nodes[para as usize].kind = Kind::Heading(level);
            self.nodes[para as usize].end = end;
            self.para = None;
            return Ok(());
        }
        // A list ends where a block other than its items starts.
        self.close_lists_above(line_start);
        let parent = self.deepest();
        match ascii(rest[0]) {
            b'#' => {
                if let Some((level, text)) = atx(rest) {
                    self.para = None;
                    let heading = self.add(parent, Kind::Heading(level), line_start, content);
                    // Empty content may not point into the source.
                    let at = if text.is_empty() {
                        content
                    } else {
                        (text.as_ptr() as usize - self.source.as_ptr() as usize) / 2
                    };
                    self.lines.push((at, at + text.len()));
                    let n = self.lines.len() as u32;
                    self.nodes[heading as usize].lines = (n - 1, n);
                    self.nodes[heading as usize].end = end;
                    return Ok(());
                }
            }
            b'*' | b'-' | b'_' if thematic(rest) => {
                self.para = None;
                let rule = self.add(parent, Kind::Rule, line_start, content);
                self.nodes[rule as usize].end = end;
                return Ok(());
            }
            b'`' | b'~' if let Some((fence, length)) = fence_open(rest) => {
                // The info string: the language up to a space, the meta after.
                let info = &rest[length..];
                if info.iter().any(|&unit| matches!(ascii(unit), b'\\' | b'&')) {
                    return Err(Fallback); // ponytail: escapes and references in the info
                }
                let at = content + length;
                let lead = run(info, SP);
                let word = info[lead..]
                    .iter()
                    .position(|&unit| unit == SP)
                    .map_or(info.len(), |end| lead + end);
                let trail = info
                    .iter()
                    .rposition(|&unit| unit != SP)
                    .map_or(word, |last| (last + 1).max(word));
                let meta_lead = word + run(&info[word..trail], SP);
                let range = |from: usize, to: usize| ((at + from) as u32, (at + to) as u32);
                self.para = None;
                // micromark's meta keeps its trailing spaces.
                let meta_end = if meta_lead < trail { info.len() } else { trail };
                let kind = Kind::Code {
                    lang: range(lead, word),
                    meta: range(meta_lead, meta_end),
                };
                let code = self.add(parent, kind, line_start, content);
                let n = self.lines.len() as u32;
                self.nodes[code as usize].lines = (n, n);
                self.nodes[code as usize].end = end;
                self.fence = Some((code, fence, length, spaces));
                return Ok(());
            }
            b'<' => return Err(Fallback), // HTML
            b'[' if self.para.is_none() && may_define(rest) => {
                // A link definition (one line, in a text kept whole): recorded, not shown.
                let at = line_start + pos + spaces;
                // Not one (yet): a paragraph.
                if let Some((label, destination, title)) =
                    crate::marked::definition(&self.source[at..])?
                {
                    if !self.whole {
                        return Err(Fallback); // a definition marked did not register
                    }
                    let label = &self.source[at + label.0..at + label.1];
                    let destination = &self.source[at + destination.0..at + destination.1];
                    if destination.iter().any(|&unit| matches!(unit, 0x5C | 0x26)) {
                        return Err(Fallback); // ponytail: escapes and references
                    }
                    let label = crate::inline::normalize_label(label).ok_or(Fallback)?;
                    if !self.links.iter().any(|(name, ..)| *name == label) {
                        let title = title.map(|(start, end)| {
                            crate::inline::unescape(&self.source[at + start..at + end])
                        });
                        self.links
                            .push((label, String::from_utf16_lossy(destination), title));
                    }
                    self.lazy = false;
                    self.def_open = true;
                    return Ok(());
                }
            }
            _ => {}
        }
        // A continuation line keeps its own indent (code spans show it).
        match self.para {
            Some(para) => self.para_line(para, line_start + pos, end),
            None => {
                let para = self.add(parent, Kind::Para, line_start, content);
                self.para = Some(para);
                self.para_line(para, content, end);
            }
        }
        self.lazy = false;
        Ok(())
    }

    /// Whether the item's marker line is the line just before `line_start` (and this
    /// one is blank): an item starting blank ends at a second blank line.
    fn blank_after_marker(&self, item: usize, line_start: usize) -> bool {
        let marker_line = self.nodes[item].line;
        newline(self.source, marker_line) + 1 == line_start
    }

    /// A list open at the deepest level with no item open in it (the item closed) ends
    /// where a quote or a leaf starts, on the line at `line_start`.
    fn close_lists_above(&mut self, line_start: usize) {
        while let Some(&top) = self.stack.last()
            && let Kind::List { .. } = self.nodes[top as usize].kind
        {
            let id = self.stack.pop().expect("open") as usize;
            self.nodes[id].exit = line_start;
            self.extend(id);
        }
    }

    /// Whether `rest` (a line past the containers it continues) starts a block that
    /// ends a paragraph.
    fn interrupts(&self, rest: &[u16]) -> Result<bool> {
        let spaces = run(rest, SP);
        if spaces >= 4 {
            return Ok(false);
        }
        let rest = &rest[spaces..];
        Ok(match ascii(rest[0]) {
            b'>' => true,
            b'#' => atx(rest).is_some(),
            b'`' | b'~' => fence_open(rest).is_some(),
            b'<' => return Err(Fallback),
            b'*' | b'-' | b'_' if thematic(rest) => true,
            _ => list_marker(rest)?.is_some_and(|marker| {
                marker.content.is_some() && (!marker.ordered || marker.start == 1)
            }),
        })
    }

    fn para_line(&mut self, para: u32, start: usize, end: usize) {
        let node = &mut self.nodes[para as usize];
        if node.lines.1 == node.lines.0 {
            node.lines = (self.lines.len() as u32, self.lines.len() as u32);
        }
        self.lines.push((start, end));
        let node = &mut self.nodes[para as usize];
        node.lines.1 += 1;
        node.end = end;
    }
}

/// Streamdown's test for a table's delimiter row in a block: a trimmed line with a pipe,
/// `\|?[ \t]*:?-+:?[ \t]*(\|[ \t]*:?-+:?[ \t]*)*\|?` throughout.
fn table_rule(block: &[u16]) -> bool {
    block.split(|&unit| unit == NL).any(|line| {
        let start = line
            .iter()
            .position(|&unit| unit != SP)
            .unwrap_or(line.len());
        let end = line
            .iter()
            .rposition(|&unit| unit != SP)
            .map_or(start, |at| at + 1);
        let line = &line[start..end];
        if !line.contains(&u16::from(b'|')) {
            return false;
        }
        let is = |at: usize, byte: u8| line.get(at) == Some(&u16::from(byte));
        let mut at = usize::from(is(0, b'|'));
        let mut first = true;
        loop {
            if !first {
                if at == line.len() {
                    return true;
                }
                if !is(at, b'|') {
                    return false;
                }
                at += 1;
                if at == line.len() {
                    return true; // a trailing pipe
                }
            }
            first = false;
            at += run(&line[at..], SP);
            at += usize::from(is(at, b':'));
            let dashes = run(&line[at..], u16::from(b'-'));
            if dashes == 0 {
                return false;
            }
            at += dashes;
            at += usize::from(is(at, b':'));
            at += run(&line[at..], SP);
        }
    })
}

/// Streamdown's test for an unclosed fence in a block: lines opening (` {0,3}` and three
/// or more backticks or tildes) and closing (the same character, as many or more) fences,
/// at line starts only.
fn unclosed_fence(block: &[u16]) -> bool {
    let mut open: Option<(u16, usize)> = None;
    for line in block.split(|&unit| unit == NL) {
        let spaces = run(line, SP);
        let Some(&fence) = line
            .get(spaces)
            .filter(|&&unit| unit == 0x60 || unit == 0x7E)
        else {
            continue;
        };
        let length = run(&line[spaces..], fence);
        if spaces > 3 || length < 3 {
            continue;
        }
        match open {
            None => open = Some((fence, length)),
            Some((char, at_least)) if char == fence && length >= at_least => open = None,
            Some(_) => {}
        }
    }
    open.is_some()
}

/// A code fence opening `rest`: its character and length (three or more backticks, with
/// none in the info string, or tildes).
pub(crate) fn fence_open(rest: &[u16]) -> Option<(u16, usize)> {
    let &fence = rest.first()?;
    let length = run(rest, fence);
    (matches!(fence, 0x60 | 0x7E)
        && length >= 3
        && (fence == 0x7E || !rest[length..].contains(&0x60)))
    .then_some((fence, length))
}

fn parent_of_para(stack: &[u32]) -> u32 {
    *stack.last().expect("the root")
}

/// Whether a line (`rest`, past its indent) starts a block that ends a table: any list
/// item, a quote, an ATX heading, a fence, a thematic break.
fn table_ends(rest: &[u16]) -> Result<bool> {
    Ok(match ascii(rest[0]) {
        b'>' => true,
        b'#' => atx(rest).is_some(),
        b'`' | b'~' => fence_open(rest).is_some(),
        b'<' => return Err(Fallback),
        b'*' | b'-' | b'_' if thematic(rest) => true,
        _ => list_marker(rest)?.is_some(),
    })
}

/// A table's delimiter row (`rest`, past its indent): its columns' alignments, when it
/// has a pipe or a colon (`-+` alone is a setext underline).
fn delimiter_row(rest: &[u16]) -> Option<Vec<u8>> {
    let is = |at: usize, byte: u8| rest.get(at) == Some(&u16::from(byte));
    let mut at = usize::from(is(0, b'|'));
    let mut marked = at > 0;
    let mut aligns = Vec::new();
    loop {
        at += run(&rest[at..], SP);
        if at == rest.len() {
            break;
        }
        let left = is(at, b':');
        at += usize::from(left);
        let dashes = run(&rest[at..], u16::from(b'-'));
        if dashes == 0 {
            return None;
        }
        at += dashes;
        let right = is(at, b':');
        at += usize::from(right);
        marked |= left || right;
        aligns.push(match (left, right) {
            (false, false) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (true, true) => 3,
        });
        at += run(&rest[at..], SP);
        if at == rest.len() {
            break;
        }
        if !is(at, b'|') {
            return None;
        }
        marked = true;
        at += 1;
    }
    (marked && !aligns.is_empty()).then_some(aligns)
}

/// A table row's cells (ranges of `line`, trimmed): split at pipes, a leading and a
/// trailing pipe dropped.
fn split_cells(line: &[u16]) -> Vec<(usize, usize)> {
    let trim = |(mut start, mut end): (usize, usize)| {
        while start < end && line[start] == SP {
            start += 1;
        }
        while end > start && line[end - 1] == SP {
            end -= 1;
        }
        (start, end)
    };
    let (mut start, mut end) = trim((0, line.len()));
    if start < end && line[start] == u16::from(b'|') {
        start = trim((start + 1, end)).0;
        if start == end {
            return Vec::new(); // a lone pipe: no cell
        }
    }
    if start < end && line[end - 1] == u16::from(b'|') {
        end -= 1;
    }
    let mut cells = Vec::new();
    let mut from = start;
    for at in (start..end)
        .filter(|&at| line[at] == u16::from(b'|'))
        .chain([end])
    {
        cells.push(trim((from, at)));
        from = at + 1;
    }
    cells
}

/// micromark's exit for a container closed at `point`: back over the blank lines before
/// it, but not past `content_end`.
fn container_exit(source: &[u16], point: usize, content_end: usize) -> usize {
    let mut end = point;
    let mut at = point;
    loop {
        let line_start = source[..at]
            .iter()
            .rposition(|&unit| unit == NL)
            .map_or(0, |nl| nl + 1);
        if line_start == 0 || !source[line_start..at].iter().all(|&unit| unit == SP) {
            return end;
        }
        let eol = line_start - 1;
        if eol < content_end {
            return end;
        }
        end = eol;
        at = eol;
    }
}

/// Where a block ends for micromark: a list at its exit (past the blank lines that only
/// container prefixes fill), others at their content's end.
fn end_of(source: &[u16], node: &Node) -> usize {
    match node.kind {
        Kind::List { .. } => node.end.max(container_exit(source, node.exit, node.end)),
        _ => node.end,
    }
}

/// Whether two line endings or more separate `end` from `start`.
fn blank_between(source: &[u16], end: usize, start: usize) -> bool {
    end < start
        && source[end..start]
            .iter()
            .filter(|&&unit| unit == NL)
            .count()
            >= 2
}

/// Renders `source` as blocks into `out`, each of Streamdown's blocks (see
/// [`crate::marked`]) parsed on its own; each block's split, in order.
pub(crate) fn render(
    source: &[u16],
    out: &mut Out,
    inline: &mut Inline,
    stable: usize,
    splits: &mut Vec<Split>,
) -> Result {
    // A text with footnotes or link definitions is one block (Streamdown's), rendered
    // whole on each update: its calls number themselves across it.
    let footnotes = crate::marked::footnotes(source);
    let mut whole = out.options.whole || footnotes;
    let cuts = if whole {
        Vec::new()
    } else if let Some(cuts) = crate::marked::cuts(source)? {
        cuts
    } else {
        whole = true;
        Vec::new()
    };
    if whole && !out.options.whole {
        out.flipped = true;
        return Ok(());
    }
    let stable = if whole { 0 } else { stable };
    inline.refs.clear();
    let mut from = 0;
    // Root DOM nodes written so far.
    let mut index = 0;
    for to in cuts.into_iter().chain([source.len()]) {
        let block = &source[from..to];
        let mut parser = Parser {
            source: block,
            nodes: Vec::new(),
            lines: Vec::new(),
            stack: Vec::new(),
            para: None,
            blank: false,
            fence: None,
            table: None,
            aligns: Vec::new(),
            lazy: false,
            indented: None,
            blanks: Vec::new(),
            whole,
            links: Vec::new(),
            def_open: false,
        };
        parser.nodes.push(Node {
            kind: Kind::Root,
            first: NONE,
            last: NONE,
            next: NONE,
            line: 0,
            start: 0,
            end: 0,
            lines: (0, 0),
            exit: 0,
        });
        parser.stack.push(0);
        let mut start = 0;
        while start <= block.len() {
            let end = newline(block, start);
            parser.line(start, &block[start..end])?;
            start = end + 1;
        }
        parser.close_to(1, block.len());
        if whole && !footnotes && parser.links.is_empty() {
            // No longer one block (a definition on the last line grew into text).
            out.flipped = true;
            return Ok(());
        }
        if whole {
            inline.refs.links = std::mem::take(&mut parser.links);
            for (id, node) in parser.nodes.iter().enumerate() {
                if let Kind::Footnote { label } = node.kind {
                    // ponytail: footnotes of paragraphs only (micromark's indents for code
                    // and lazy lines in them are their own).
                    let mut child = node.first;
                    while child != NONE {
                        if parser.nodes[child as usize].kind != Kind::Para {
                            return Err(Fallback);
                        }
                        child = parser.nodes[child as usize].next;
                    }
                    let label: Vec<u16> = block[label.0 as usize..label.1 as usize]
                        .iter()
                        .map(|&unit| crate::inline::lower(unit))
                        .collect();
                    if !inline.refs.notes.iter().any(|(name, _)| *name == label) {
                        inline.refs.notes.push((label, id as u32));
                    }
                }
            }
        }
        // Streamdown's `isIncomplete`: the last block while animating, with an unclosed
        // fence by its own test.
        let incomplete = out.options.animating && to == source.len() && unclosed_fence(block);
        let mut writer = Writer {
            source: block,
            nodes: &parser.nodes,
            lines: &parser.lines,
            aligns: &parser.aligns,
            out,
            inline,
            stable,
            scratch: Vec::new(),
            incomplete,
            checkbox: None,
        };
        // The root's children (footnote definitions render after them).
        let roots: Vec<u32> = writer
            .children(0)
            .filter(|&id| !matches!(parser.nodes[id as usize].kind, Kind::Footnote { .. }))
            .collect();
        // With `dir`, every block (an empty one too) is one root element.
        let wrap = writer.out.options.dir;
        if !roots.is_empty() || wrap > 0 {
            let list = roots
                .iter()
                .any(|&id| matches!(parser.nodes[id as usize].kind, Kind::List { .. }));
            splits.push(Split {
                source: from,
                words: writer.out.words.len(),
                text: writer.out.text.len(),
                nodes: index,
                list,
                decides: 0,
            });
        }
        // The last root element's word.
        let mut last_element = None;
        let wrapper = writer.out.words.len();
        let mut depth = 0u32;
        if wrap > 0 {
            writer.out.open_html(if wrap == 1 {
                r#"<div dir="ltr" style="display: contents;">"#
            } else {
                r#"<div dir="rtl" style="display: contents;">"#
            });
        }
        for (index, &child) in roots.iter().enumerate() {
            if index > 0 {
                // remark's root puts a line ending between its children.
                writer.out.ascii("\n");
            }
            let at = writer.out.words.len();
            writer.block(child, false)?;
            last_element = writer.out.words[at..]
                .iter()
                .position(|&word| word & 3 == kind::OPEN || word & 3 == kind::VOID)
                .map(|offset| at + offset);
        }
        if whole && let Some(at) = writer.footnotes(!roots.is_empty())? {
            last_element = Some(at);
        }
        if wrap > 0 {
            writer.out.close();
            last_element = Some(wrapper);
        }
        // Root nodes, as the DOM side's `build` counts them (a table's whitespace text is one).
        for &word in &writer.out.words[wrapper..] {
            match word & 3 {
                kind::CLOSE => depth -= 1,
                kind::OPEN => {
                    index += u32::from(depth == 0);
                    depth += 1;
                }
                _ => index += u32::from(depth == 0),
            }
        }
        // Streamdown hides its caret on the root's last element while the last block holds
        // an unclosed fence or a table's delimiter row.
        if to == source.len()
            && out.options.animating
            && (unclosed_fence(block) || table_rule(block))
            && let Some(at) = last_element
        {
            out.add_attribute(at, r#" data-sd-caret-hidden="""#);
        }
        from = to;
    }
    Ok(())
}

/// A code block's controls: Streamdown's download and copy buttons and their icons.
const CODE_ACTIONS: &str = r#"<div class="pointer-events-none absolute top-2 right-2 z-10 flex items-center"><div class="pointer-events-auto flex shrink-0 items-center gap-2 rounded-md border border-sidebar bg-sidebar/80 px-1.5 py-1 supports-[backdrop-filter]:bg-sidebar/70 supports-[backdrop-filter]:backdrop-blur" data-streamdown="code-block-actions"><button aria-label="Download file" class="cursor-pointer p-1 text-muted-foreground transition-all hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50" data-streamdown="code-block-download-button" title="Download file" type="button"><svg aria-hidden="true" color="currentColor" height="16" stroke-linejoin="round" viewBox="0 0 16 16" width="16" size="14"><path clip-rule="evenodd" d="M8.75 1V1.75V8.68934L10.7197 6.71967L11.25 6.18934L12.3107 7.25L11.7803 7.78033L8.70711 10.8536C8.31658 11.2441 7.68342 11.2441 7.29289 10.8536L4.21967 7.78033L3.68934 7.25L4.75 6.18934L5.28033 6.71967L7.25 8.68934V1.75V1H8.75ZM13.5 9.25V13.5H2.5V9.25V8.5H1V9.25V14C1 14.5523 1.44771 15 2 15H14C14.5523 15 15 14.5523 15 14V9.25V8.5H13.5V9.25Z" fill="currentColor" fill-rule="evenodd"></path></svg></button><button aria-label="Copy Code" class="cursor-pointer p-1 text-muted-foreground transition-all hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50" data-streamdown="code-block-copy-button" title="Copy Code" type="button"><svg aria-hidden="true" color="currentColor" height="16" stroke-linejoin="round" viewBox="0 0 16 16" width="16" size="14"><path clip-rule="evenodd" d="M2.75 0.5C1.7835 0.5 1 1.2835 1 2.25V9.75C1 10.7165 1.7835 11.5 2.75 11.5H3.75H4.5V10H3.75H2.75C2.61193 10 2.5 9.88807 2.5 9.75V2.25C2.5 2.11193 2.61193 2 2.75 2H8.25C8.38807 2 8.5 2.11193 8.5 2.25V3H10V2.25C10 1.2835 9.2165 0.5 8.25 0.5H2.75ZM7.75 4.5C6.7835 4.5 6 5.2835 6 6.25V13.75C6 14.7165 6.7835 15.5 7.75 15.5H13.25C14.2165 15.5 15 14.7165 15 13.75V6.25C15 5.2835 14.2165 4.5 13.25 4.5H7.75ZM7.5 6.25C7.5 6.11193 7.61193 6 7.75 6H13.25C13.3881 6 13.5 6.11193 13.5 6.25V13.75C13.5 13.8881 13.3881 14 13.25 14H7.75C7.61193 14 7.5 13.8881 7.5 13.75V6.25Z" fill="currentColor" fill-rule="evenodd"></path></svg></button></div></div>"#;

/// A table's controls: Streamdown's copy, download and fullscreen buttons.
const TABLE_CONTROLS: &str = r#"<div class="flex items-center justify-end gap-1"><div class="relative"><button class="cursor-pointer p-1 text-muted-foreground transition-all hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50" title="Copy table" type="button"><svg aria-hidden="true" color="currentColor" height="14" stroke-linejoin="round" viewBox="0 0 16 16" width="14"><path clip-rule="evenodd" d="M2.75 0.5C1.7835 0.5 1 1.2835 1 2.25V9.75C1 10.7165 1.7835 11.5 2.75 11.5H3.75H4.5V10H3.75H2.75C2.61193 10 2.5 9.88807 2.5 9.75V2.25C2.5 2.11193 2.61193 2 2.75 2H8.25C8.38807 2 8.5 2.11193 8.5 2.25V3H10V2.25C10 1.2835 9.2165 0.5 8.25 0.5H2.75ZM7.75 4.5C6.7835 4.5 6 5.2835 6 6.25V13.75C6 14.7165 6.7835 15.5 7.75 15.5H13.25C14.2165 15.5 15 14.7165 15 13.75V6.25C15 5.2835 14.2165 4.5 13.25 4.5H7.75ZM7.5 6.25C7.5 6.11193 7.61193 6 7.75 6H13.25C13.3881 6 13.5 6.11193 13.5 6.25V13.75C13.5 13.8881 13.3881 14 13.25 14H7.75C7.61193 14 7.5 13.8881 7.5 13.75V6.25Z" fill="currentColor" fill-rule="evenodd"></path></svg></button></div><div class="relative"><button class="cursor-pointer p-1 text-muted-foreground transition-all hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50" title="Download table" type="button"><svg aria-hidden="true" color="currentColor" height="16" stroke-linejoin="round" viewBox="0 0 16 16" width="16" size="14"><path clip-rule="evenodd" d="M8.75 1V1.75V8.68934L10.7197 6.71967L11.25 6.18934L12.3107 7.25L11.7803 7.78033L8.70711 10.8536C8.31658 11.2441 7.68342 11.2441 7.29289 10.8536L4.21967 7.78033L3.68934 7.25L4.75 6.18934L5.28033 6.71967L7.25 8.68934V1.75V1H8.75ZM13.5 9.25V13.5H2.5V9.25V8.5H1V9.25V14C1 14.5523 1.44771 15 2 15H14C14.5523 15 15 14.5523 15 14V9.25V8.5H13.5V9.25Z" fill="currentColor" fill-rule="evenodd"></path></svg></button></div><button class="cursor-pointer p-1 text-muted-foreground transition-all hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50" title="View fullscreen" type="button"><svg aria-hidden="true" color="currentColor" height="16" stroke-linejoin="round" viewBox="0 0 16 16" width="16" size="14"><path clip-rule="evenodd" d="M1 5.25V6H2.5V5.25V2.5H5.25H6V1H5.25H2C1.44772 1 1 1.44772 1 2V5.25ZM5.25 14.9994H6V13.4994H5.25H2.5V10.7494V9.99939H1V10.7494V13.9994C1 14.5517 1.44772 14.9994 2 14.9994H5.25ZM15 10V10.75V14C15 14.5523 14.5523 15 14 15H10.75H10V13.5H10.75H13.5V10.75V10H15ZM10.75 1H10V2.5H10.75H13.5V5.25V6H15V5.25V2C15 1.44772 14.5523 1 14 1H10.75Z" fill="currentColor" fill-rule="evenodd"></path></svg></button></div>"#;

impl Writer<'_, '_> {
    /// mdast-util-to-hast's footnote section through Streamdown's components (its list
    /// item shows a lone paragraph's content; its section drops an item with nothing
    /// but the back reference): where it opens, if any footnote was called.
    fn footnotes(&mut self, after: bool) -> Result<Option<usize>> {
        if self.inline.refs.calls.is_empty() {
            return Ok(None);
        }
        if after {
            self.out.ascii("\n");
        }
        let at = self.out.words.len();
        self.out
            .open_html(r#"<section class="footnotes" data-footnotes="">"#);
        let p = crate::inline::clobber(&self.out);
        self.out.open_html(&format!(r#"<h2 class="mt-6 mb-2 font-semibold text-2xl sr-only" data-streamdown="heading-2" id="{p}footnote-label">"#));
        self.out.ascii("Footnotes");
        self.out.close();
        self.out.ascii("\n");
        self.out.open(tag::OL);
        self.out.ascii("\n");
        for index in 0..self.inline.refs.calls.len() {
            let calls = self.inline.refs.calls.clone();
            let (note, count) = calls[index];
            let (label, node) = &self.inline.refs.notes[note as usize];
            let node = *node;
            let children: Vec<u32> = self.children(node).collect();
            if let Some(&last) = children.last() {
                let id = String::from_utf16_lossy(label);
                self.out.open_html(&format!(r#"<li class="py-1 [&amp;&gt;p]:inline" data-streamdown="list-item" id="{p}user-content-fn-{id}">"#));
                let back = (id.clone(), index + 1);
                let para_last = self.nodes[last as usize].kind == Kind::Para;
                if children.len() == 1 && para_last {
                    self.inline_content(last, true)?;
                    self.out.ascii(" ");
                    self.backrefs(count, " ", &back)?;
                } else {
                    self.out.ascii("\n");
                    for &child in &children {
                        if child == last && para_last {
                            self.out.open(tag::P);
                            self.inline_content(child, true)?;
                            self.out.ascii(" ");
                            self.backrefs(count, " ", &back)?;
                            self.out.close();
                        } else {
                            self.block(child, false)?;
                        }
                        self.out.ascii("\n");
                    }
                    if !para_last {
                        // (Each one a child of the item, line endings around.)
                        self.backrefs(count, "\n \n", &back)?;
                        self.out.ascii("\n");
                    }
                }
                self.out.close();
                if self.inline.refs.calls != calls {
                    return Err(Fallback); // ponytail: a call inside a footnote
                }
            }
            self.out.ascii("\n");
        }
        self.out.close();
        self.out.ascii("\n");
        self.out.close();
        Ok(Some(at))
    }

    /// A footnote's back references, one per call, `between` them; `back` is the note's
    /// label and number.
    fn backrefs(&mut self, count: u32, between: &str, back: &(String, usize)) -> Result<()> {
        let island = self.out.options.overrides & crate::tokens::overrides::A != 0;
        let id = if island { crate::inline::note_id(&back.0.encode_utf16().collect::<Vec<_>>())? } else { String::new() };
        for call in 1..=count {
            if call > 1 {
                self.out.ascii(between);
            }
            if island {
                // The page's own `a`, as mdast-util-to-hast gives it.
                let suffix = if call > 1 { format!("-{call}") } else { String::new() };
                let number = back.1;
                self.out.open_html(&format!(
                    r##"{}<a href="#user-content-fnref-{id}{suffix}" data-footnote-backref="" aria-label="Back to reference {number}{suffix}" class="data-footnote-backref" target="_blank" rel="noopener noreferrer">"##,
                    crate::tokens::ISLAND
                ));
            } else {
                self.out.open_html(r#"<button class="wrap-anywhere appearance-none text-left font-medium text-primary underline data-footnote-backref" data-incomplete="false" data-streamdown="link" type="button">"#);
            }
            self.out.text(&[0x21A9]);
            if call > 1 {
                self.out.open_html(crate::inline::SUP);
                self.out.ascii(&call.to_string());
                self.out.close();
            }
            self.out.close();
        }
        Ok(())
    }

    /// Controls (Streamdown disables every one of them while the text animates).
    fn controls(&mut self, html: &str) {
        if self.out.options.animating {
            // ponytail: rewritten on each render; cache it if it shows in profiles.
            self.out
                .subtree(&html.replace(" title=", r#" disabled="" title="#));
        } else {
            self.out.subtree(html);
        }
    }
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

struct Writer<'a, 'o> {
    source: &'a [u16],
    nodes: &'a [Node],
    lines: &'a [(usize, usize)],
    aligns: &'a [u8],
    out: &'o mut Out,
    inline: &'o mut Inline,
    stable: usize,
    scratch: Vec<u16>,
    /// The block's code blocks are marked incomplete.
    incomplete: bool,
    /// A task item's checkbox, for its first paragraph.
    checkbox: Option<u32>,
}

impl Writer<'_, '_> {
    fn children(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        let mut child = self.nodes[id as usize].first;
        std::iter::from_fn(move || {
            (child != NONE).then(|| {
                let current = child;
                child = self.nodes[child as usize].next;
                current
            })
        })
    }

    /// Writes block `id`; `inline_para`: a paragraph writes only its content (a tight
    /// item's).
    fn block(&mut self, id: u32, inline_para: bool) -> Result {
        let node = &self.nodes[id as usize];
        match node.kind {
            Kind::Root | Kind::Item { .. } | Kind::Footnote { .. } => {
                unreachable!("written by their parents")
            }
            Kind::Quote => {
                self.out.open(tag::BLOCKQUOTE);
                self.wrap(id)?;
                self.out.close();
            }
            Kind::List { ordered, start, .. } => {
                let tasks = self.children(id).any(|item| {
                    matches!(self.nodes[item as usize].kind, Kind::Item { task: 1.., .. })
                });
                let (ul, ol) = if tasks {
                    (tag::UL_TASKS, tag::OL_TASKS)
                } else {
                    (tag::UL, tag::OL)
                };
                match (ordered, start) {
                    (false, _) => self.out.open(ul),
                    (true, 1) => self.out.open(ol),
                    (true, start) => {
                        let ol = TAGS[ol as usize];
                        self.out
                            .open_html(&format!("{} start=\"{start}\">", &ol[..ol.len() - 1]));
                    }
                }
                let loose = self.loose(id);
                self.out.ascii("\n");
                let items: Vec<u32> = self.children(id).collect();
                for item in items {
                    self.item(item, loose)?;
                    self.out.ascii("\n");
                }
                self.out.close();
            }
            Kind::Para => {
                if !inline_para {
                    self.out.open(tag::P);
                }
                if let Some(checkbox) = self.checkbox.take() {
                    self.out.void(checkbox);
                }
                self.inline_content(id, true)?;
                if !inline_para {
                    self.out.close();
                }
            }
            Kind::Heading(level) => {
                self.out.open(tag::H1 + u32::from(level) - 1);
                self.inline_content(id, true)?;
                self.out.close();
            }
            Kind::Rule => self.out.void(tag::HR),
            Kind::Code { lang, meta } => self.code(id, lang, meta),
            Kind::Table { aligns } => self.table(id, aligns)?,
        }
        Ok(())
    }

    /// Streamdown's table: HAST's whitespace inside the table (React puts it before the
    /// wrapper), the wrapper and its controls, then the table, short rows padded.
    fn table(&mut self, id: u32, aligns: (u32, u32)) -> Result {
        let (first, last) = self.nodes[id as usize].lines;
        let lines = &self.lines[first as usize..last as usize];
        let aligns = &self.aligns[aligns.0 as usize..aligns.1 as usize];
        let (columns, rows) = (aligns.len(), lines.len() - 1);
        // mdast-util-to-hast wraps the sections, each section's rows and each row's cells.
        let whitespace = 2
            + usize::from(rows > 0)
            + 2
            + if rows > 0 { rows + 1 } else { 0 }
            + (rows + 1) * (columns + 1);
        self.out.repeat(b'\n', whitespace as u32);
        // A page's own `table` gets the table alone (Streamdown's wrapper is its own).
        let island = self.out.options.overrides & crate::tokens::overrides::TABLE != 0;
        if island {
            self.out.open_html("\u{1}<table>");
        } else {
            self.out.open(tag::TABLE_WRAPPER);
            if self.out.options.controls {
                self.controls(TABLE_CONTROLS);
            }
            self.out.open(tag::TABLE_SCROLL);
            self.out.open(tag::TABLE);
        }
        self.out.open(tag::THEAD);
        self.row(lines[0], aligns, tag::TH)?;
        self.out.close();
        if rows > 0 {
            self.out.open(tag::TBODY);
            for &line in &lines[1..] {
                self.row(line, aligns, tag::TD)?;
            }
            self.out.close();
        }
        for _ in 0..if island { 1 } else { 3 } {
            self.out.close();
        }
        Ok(())
    }

    fn row(&mut self, (start, end): (usize, usize), aligns: &[u8], cell: u32) -> Result {
        let line = &self.source[start..end];
        let cells = split_cells(line);
        self.out.open(tag::TR);
        for (column, &align) in aligns.iter().enumerate() {
            self.out.open(cell + u32::from(align));
            if let Some(&(from, to)) = cells.get(column)
                && from < to
            {
                self.inline
                    .render(&line[from..to], 0, self.out, self.stable)?;
            }
            self.out.close();
        }
        self.out.close();
        Ok(())
    }

    /// Streamdown's code block before highlighting: its container, header and controls,
    /// then a span per line (the code without its trailing line endings), each holding
    /// one token span, or a bare line ending for an empty line.
    fn code(&mut self, id: u32, lang: (u32, u32), meta: (u32, u32)) {
        let source = self.source;
        let text = |range: (u32, u32)| &source[range.0 as usize..range.1 as usize];
        let attribute =
            |range: (u32, u32)| escape_attribute(&String::from_utf16_lossy(text(range)));
        let language = attribute(lang);
        let prefix = if language.is_empty() {
            String::new()
        } else {
            format!("language-{language} ")
        };
        let incomplete = if self.incomplete {
            r#" data-incomplete="true""#
        } else {
            ""
        };
        if self.out.options.overrides & crate::tokens::overrides::CODE != 0 {
            // mdast-util-to-hast's `code`: the value and a line ending, if any.
            let class = if language.is_empty() { String::new() } else { format!(r#" class="language-{language}""#) };
            let meta = if meta.0 == meta.1 || self.out.options.setup & crate::tokens::setup::NO_CODE_META != 0 { String::new() } else { format!(r#" metastring="{}""#, attribute(meta)) };
            self.out.open_html(&format!("\u{1}<code{class}{meta}{incomplete}>"));
            let (first, last) = self.nodes[id as usize].lines;
            let lines = &self.lines[first as usize..last as usize];
            for (index, &(start, end)) in lines.iter().enumerate() {
                if index > 0 {
                    self.out.ascii("\n");
                }
                self.out.text(&source[start..end]);
            }
            if lines.len() > 1 || lines.first().is_some_and(|&(start, end)| start < end) {
                self.out.ascii("\n");
            }
            self.out.close();
            return;
        }
        self.out.open_html(&format!(
            r#"<div class="relative my-4 flex w-full flex-col gap-2 rounded-xl border border-border bg-sidebar p-2"{incomplete} data-language="{language}" data-streamdown="code-block" style="content-visibility: auto; contain-intrinsic-size: auto 200px;" dir="ltr">"#
        ));
        self.out.open_html(&format!(r#"<div class="flex h-8 items-center text-muted-foreground text-xs" data-language="{language}" data-streamdown="code-block-header">"#));
        self.out
            .open_html(r#"<span class="ml-1 font-mono lowercase">"#);
        self.out.text(text(lang));
        self.out.close();
        self.out.close();
        if self.out.options.controls {
            self.controls(CODE_ACTIONS);
        }
        let meta = if meta.0 == meta.1 || self.out.options.setup & crate::tokens::setup::NO_CODE_META != 0 {
            String::new()
        } else {
            format!(r#" metastring="{}""#, attribute(meta))
        };
        let (scroll, height) = match self.out.options.code_max_height {
            0 => ("", String::new()),
            pixels => (
                "overflow-y-auto ",
                format!(r#" style="max-height: {pixels}px;""#),
            ),
        };
        self.out.open_html(&format!(
            r#"<div class="{prefix}{scroll}overflow-x-auto rounded-md border border-border bg-background p-4 text-sm" data-language="{language}" data-streamdown="code-block-body"{height}{meta}>"#
        ));
        self.out.open_html(&format!(
            r#"<pre class="{prefix}bg-[var(--sdm-bg,inherit)] dark:bg-[var(--shiki-dark-bg,var(--sdm-bg,inherit))]" style="--sdm-bg: transparent; --sdm-fg: inherit;">"#
        ));
        let numbered = self.out.options.line_numbers;
        if numbered {
            self.out.open(tag::CODE_BLOCK);
        } else {
            self.out.open_html("<code>");
        }
        let (first, last) = self.nodes[id as usize].lines;
        let lines = &self.lines[first as usize..last as usize];
        // Trailing empty lines are trailing line endings; the code has one line at least.
        let count = lines
            .iter()
            .rposition(|&(start, end)| start < end)
            .map_or(1, |last| last + 1);
        for index in 0..count {
            let line = lines
                .get(index)
                .map_or(&[][..], |&(start, end)| &source[start..end]);
            if numbered {
                self.out.open(tag::CODE_LINE);
            } else {
                self.out.open_html(r#"<span class="block">"#);
            }
            if line.is_empty() {
                self.out.ascii("\n");
            } else {
                self.out.open(tag::CODE_TOKEN);
                self.out.text(line);
                self.out.close();
            }
            self.out.close();
        }
        for _ in 0..4 {
            self.out.close();
        }
    }

    /// mdast-util-to-hast's `wrap` (loose): line endings before, between and after.
    fn wrap(&mut self, id: u32) -> Result {
        self.out.ascii("\n");
        let children: Vec<u32> = self.children(id).collect();
        for (index, &child) in children.iter().enumerate() {
            if index > 0 {
                self.out.ascii("\n");
            }
            self.block(child, false)?;
        }
        if !children.is_empty() {
            self.out.ascii("\n");
        }
        Ok(())
    }

    /// An item: mdast-util-to-hast's `listItem`, then Streamdown's `MemoLi` (an item
    /// holding only a paragraph renders the paragraph's content).
    fn item(&mut self, id: u32, loose: bool) -> Result {
        let Kind::Item { task, .. } = self.nodes[id as usize].kind else {
            unreachable!("an item")
        };
        // ponytail: a task whose paragraph became a heading or table (no checkbox): the
        // exact path.
        if task > 0 && self.nodes[self.nodes[id as usize].first as usize].kind != Kind::Para {
            return Err(Fallback);
        }
        self.out.open(if task > 0 { tag::LI_TASK } else { tag::LI });
        // The checkbox starts the first paragraph.
        self.checkbox = (task > 0).then(|| tag::CHECKBOX + u32::from(task - 1));
        let children: Vec<u32> = self.children(id).collect();
        let para = |child: u32| self.nodes[child as usize].kind == Kind::Para;
        if children.len() == 1 && para(children[0]) {
            self.block(children[0], true)?;
        } else {
            for (index, &child) in children.iter().enumerate() {
                let is_para = para(child);
                if loose || index != 0 || !is_para {
                    self.out.ascii("\n");
                }
                self.block(child, is_para && !loose)?;
            }
            if let Some(&last) = children.last()
                && (loose || !para(last))
            {
                self.out.ascii("\n");
            }
        }
        self.out.close();
        Ok(())
    }

    /// mdast-util-to-hast's `listLoose`: the list is spread, or any of its items is.
    fn loose(&self, list: u32) -> bool {
        let node = &self.nodes[list as usize];
        // Line endings between the content and the list's exit count toward its spread.
        if blank_between(
            self.source,
            node.end,
            container_exit(self.source, node.exit, node.end),
        ) {
            return true;
        }
        let items: Vec<u32> = self.children(list).collect();
        let mut previous: Option<usize> = None;
        for &item in &items {
            let node = &self.nodes[item as usize];
            if previous.is_some_and(|end| blank_between(self.source, end, node.start)) {
                return true;
            }
            previous = Some(node.end);
            // An item is spread when a blank line separates two of its children.
            let mut before: Option<usize> = None;
            for child in self.children(item) {
                let child = &self.nodes[child as usize];
                if before.is_some_and(|end| blank_between(self.source, end, child.start)) {
                    return true;
                }
                before = Some(end_of(self.source, child));
            }
        }
        false
    }

    /// A paragraph's or heading's inline content.
    fn inline_content(&mut self, id: u32, trim_end: bool) -> Result {
        let (first, last) = self.nodes[id as usize].lines;
        let lines = &self.lines[first as usize..last as usize];
        let Some(&(start, _)) = lines.first() else {
            return self.inline.render(&[], 0, self.out, self.stable);
        };
        let end = lines.last().expect("a line").1;
        // Lines apart by line endings and one width of container indent (an item's) are
        // one stretch of the source, the indent stripped as the inline scan meets it;
        // others (quoted or lazy lines) are joined without their prefixes.
        let mut indent = None;
        let contiguous = lines.windows(2).all(|pair| {
            let gap = &self.source[pair[0].1..pair[1].0];
            gap.first() == Some(&NL)
                && gap[1..].iter().all(|&unit| unit == SP)
                && *indent.get_or_insert(gap.len() - 1) == gap.len() - 1
        });
        let trim = |text: &[u16]| -> usize {
            if trim_end {
                text.iter()
                    .rposition(|&unit| unit != SP)
                    .map_or(0, |at| at + 1)
            } else {
                text.len()
            }
        };
        if contiguous {
            let text = &self.source[start..end];
            return self.inline.render(
                &text[..trim(text)],
                indent.unwrap_or(0),
                self.out,
                self.stable,
            );
        }
        self.scratch.clear();
        for (index, &(start, end)) in lines.iter().enumerate() {
            if index > 0 {
                self.scratch.push(NL);
            }
            self.scratch.extend_from_slice(&self.source[start..end]);
        }
        let length = trim(&self.scratch);
        let scratch = std::mem::take(&mut self.scratch);
        let result = self
            .inline
            .render(&scratch[..length], 0, self.out, self.stable);
        self.scratch = scratch;
        result
    }
}
