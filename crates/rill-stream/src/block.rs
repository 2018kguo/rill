//! Blocks: CommonMark's block structure as micromark parses it (block quotes, lists and
//! their items, paragraphs, headings, thematic breaks), then Streamdown's DOM for it.
//!
//! One pass over the lines builds flat records: each line goes through the open
//! containers, may open new ones, and adds its content to the deepest open block. A
//! second pass writes the tokens, because a list's looseness (whether its items' content
//! sits in `<p>`) is known only once it closes.

use crate::inline::Inline;
use crate::tokens::{Out, TAGS, tag};
use crate::{Fallback, NL, Result, SP, Split, ascii, atx, list_marker, may_define, newline, run, setext, thematic};

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Root,
    Quote,
    List { ordered: bool, marker: u8, start: u32 },
    /// `column`: where its content starts, from where the line's content within the
    /// item's container starts. `empty`: its marker line has no content.
    Item { column: usize, empty: bool },
    Para,
    Heading(u8),
    Rule,
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
}

impl<'s> Parser<'s> {
    fn add(&mut self, parent: u32, kind: Kind, line: usize, start: usize) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node { kind, first: NONE, last: NONE, next: NONE, line, start, end: start, lines: (0, 0), exit: 0 });
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
                Kind::Item { column, empty } => {
                    let rest = &line[pos..];
                    if rest.iter().all(|&unit| unit == SP) {
                        // An item can start with at most one blank line.
                        if empty && self.nodes[id].first == NONE && self.blank_after_marker(id, line_start) {
                            break;
                        }
                    } else if run(rest, SP) >= column {
                        pos += column;
                    } else {
                        break;
                    }
                }
                _ => unreachable!("containers only"),
            }
            matched += 1;
        }
        // 2. New containers. micromark tests list starts against an open paragraph only
        // when every open container continued (then for the whole line, even inside a
        // quote the line opens); a line that leaves a container may start any list.
        let all = matched == self.stack.len();
        let interrupting = all && self.para.is_some();
        let mut closed = all;
        let mut opened = false;
        loop {
            let spaces = run(&line[pos..], SP);
            let rest = &line[pos + spaces..];
            if rest.is_empty() || spaces >= 4 {
                break;
            }
            let quote = rest[0] == u16::from(b'>');
            let marker = if quote { None } else { list_marker(rest)? };
            if !quote {
                let Some(marker) = &marker else { break };
                if matches!(ascii(rest[0]), b'*' | b'-' | b'_') && thematic(rest) {
                    break; // a thematic break wins over an item
                }
                // An empty item, or an ordered one not starting at 1, does not interrupt
                // a paragraph.
                if interrupting && !(marker.content.is_some() && (!marker.ordered || marker.start == 1)) {
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
            let kind = Kind::List { ordered: marker.ordered, marker: marker.marker, start: marker.start };
            // The item joins the open list of its kind, or starts a list.
            let deepest = self.deepest() as usize;
            let list = match self.nodes[deepest].kind {
                Kind::List { ordered, marker: m, .. } if ordered == marker.ordered && m == marker.marker => deepest as u32,
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
            let item = self.add(list, Kind::Item { column: spaces + marker.width, empty }, line_start, line_start + pos + spaces);
            self.stack.push(item);
            opened = true;
            match marker.content {
                Some(at) => {
                    pos += spaces + at;
                    // A task, or a definition, as the item's first content.
                    if line[pos] == u16::from(b'[') {
                        return Err(Fallback);
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
                self.para_line(para, line_start + pos, line_start + line.len());
                self.blank = false;
                return Ok(());
            }
            self.close_to(matched, line_start);
        }
        // 3. The leaf.
        let spaces = run(&line[pos..], SP);
        let rest = &line[pos + spaces..];
        if rest.is_empty() {
            self.para = None;
            self.blank = true;
            return Ok(());
        }
        let content = line_start + pos + spaces;
        let end = line_start + line.len();
        if spaces >= 4 {
            match self.para {
                Some(para) if !opened => self.para_line(para, line_start + pos, end),
                _ => return Err(Fallback), // indented code
            }
            self.blank = false;
            return Ok(());
        }
        self.blank = false;
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
                    let at = if text.is_empty() { content } else { (text.as_ptr() as usize - self.source.as_ptr() as usize) / 2 };
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
            b'`' | b'~' if run(rest, rest[0]) >= 3 => return Err(Fallback), // fences
            b'<' => return Err(Fallback),                                      // HTML
            b'[' if self.para.is_none() && may_define(rest) => return Err(Fallback),
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
            b'`' | b'~' => run(rest, rest[0]) >= 3,
            b'<' => return Err(Fallback),
            b'*' | b'-' | b'_' if thematic(rest) => true,
            _ => list_marker(rest)?.is_some_and(|marker| marker.content.is_some() && (!marker.ordered || marker.start == 1)),
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

/// micromark's exit for a container closed at `point`: back over the blank lines before
/// it, but not past `content_end`.
fn container_exit(source: &[u16], point: usize, content_end: usize) -> usize {
    let mut end = point;
    let mut at = point;
    loop {
        let line_start = source[..at].iter().rposition(|&unit| unit == NL).map_or(0, |nl| nl + 1);
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
    end < start && source[end..start].iter().filter(|&&unit| unit == NL).count() >= 2
}

/// Renders `source` as blocks into `out`, each of Streamdown's blocks (see
/// [`crate::marked`]) parsed on its own; each block's split, in order.
pub(crate) fn render(source: &[u16], out: &mut Out, inline: &mut Inline, stable: usize, splits: &mut Vec<Split>) -> Result {
    let cuts = crate::marked::cuts(source)?;
    let mut from = 0;
    // Root DOM nodes written so far.
    let mut index = 0;
    for to in cuts.into_iter().chain([source.len()]) {
        let block = &source[from..to];
        let mut parser = Parser { source: block, nodes: Vec::new(), lines: Vec::new(), stack: Vec::new(), para: None, blank: false };
        parser.nodes.push(Node { kind: Kind::Root, first: NONE, last: NONE, next: NONE, line: 0, start: 0, end: 0, lines: (0, 0), exit: 0 });
        parser.stack.push(0);
        let mut start = 0;
        while start <= block.len() {
            let end = newline(block, start);
            parser.line(start, &block[start..end])?;
            start = end + 1;
        }
        parser.close_to(1, block.len());
        let mut writer = Writer { source: block, nodes: &parser.nodes, lines: &parser.lines, out, inline, stable, scratch: Vec::new() };
        let mut child = parser.nodes[0].first;
        if child != NONE {
            let mut list = false;
            let mut node = child;
            while node != NONE {
                list |= matches!(parser.nodes[node as usize].kind, Kind::List { .. });
                node = parser.nodes[node as usize].next;
            }
            splits.push(Split { source: from, words: writer.out.words.len(), text: writer.out.text.len(), nodes: index, list, decides: 0 });
        }
        while child != NONE {
            if child != parser.nodes[0].first {
                // remark's root puts a line ending between its children.
                writer.out.ascii("\n");
                index += 1;
            }
            writer.block(child, false)?;
            index += 1;
            child = parser.nodes[child as usize].next;
        }
        from = to;
    }
    Ok(())
}

struct Writer<'a, 'o> {
    source: &'a [u16],
    nodes: &'a [Node],
    lines: &'a [(usize, usize)],
    out: &'o mut Out,
    inline: &'o mut Inline,
    stable: usize,
    scratch: Vec<u16>,
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
            Kind::Root | Kind::Item { .. } => unreachable!("written by their parents"),
            Kind::Quote => {
                self.out.open(tag::BLOCKQUOTE);
                self.wrap(id)?;
                self.out.close();
            }
            Kind::List { ordered, start, .. } => {
                match (ordered, start) {
                    (false, _) => self.out.open(tag::UL),
                    (true, 1) => self.out.open(tag::OL),
                    (true, start) => {
                        let ol = TAGS[tag::OL as usize];
                        self.out.open_html(&format!("{} start=\"{start}\">", &ol[..ol.len() - 1]));
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
        }
        Ok(())
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
        self.out.open(tag::LI);
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
        if blank_between(self.source, node.end, container_exit(self.source, node.exit, node.end)) {
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
            if trim_end { text.iter().rposition(|&unit| unit != SP).map_or(0, |at| at + 1) } else { text.len() }
        };
        if contiguous {
            let text = &self.source[start..end];
            return self.inline.render(&text[..trim(text)], indent.unwrap_or(0), self.out, self.stable);
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
        let result = self.inline.render(&scratch[..length], 0, self.out, self.stable);
        self.scratch = scratch;
        result
    }
}
