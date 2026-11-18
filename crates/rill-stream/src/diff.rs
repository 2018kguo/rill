//! The DOM edit from one rendering of the open tail to the next, as a small program
//! for js/src/stream-dom.ts.
//!
//! Both renderings are this crate's own HTML, so the DOM holds exactly the old one's
//! nodes. Walking the two token by token finds the first difference, with its node's
//! path (child indices, the first relative to the tail's first node). The usual update
//! only lengthens one text node: that is a single `append` of the new characters. Any
//! other difference removes the old nodes from there on and inserts the new HTML from
//! there on, one fragment per level, deepest first.

/// Opcodes.
pub mod op {
    /// `depth, path..., text`: append `text` to the text node at the path.
    pub const APPEND: u32 = 0;
    /// `depth, path...`: remove the node at the path, the siblings after it, and the
    /// siblings after each of its ancestors (up to the tail).
    pub const REMOVE: u32 = 1;
    /// `depth, path..., html`: insert `html` at the end of the element at the path
    /// (depth 0: the root).
    pub const INSERT: u32 = 2;
    /// `depth, path..., text`: append a text node holding `text` to the element at the path.
    pub const INSERT_TEXT: u32 = 3;
}

/// The program: opcodes with their operands, strings as `[start, end)` UTF-16 ranges in
/// `text`.
#[derive(Default)]
pub struct Program {
    pub ops: Vec<u32>,
    pub text: String,
    utf16: u32,
}

impl Program {
    pub fn clear(&mut self) {
        self.ops.clear();
        self.text.clear();
        self.utf16 = 0;
    }

    fn string(&mut self, value: &str) {
        let units = value.encode_utf16().count() as u32;
        self.text.push_str(value);
        self.ops.push(self.utf16);
        self.ops.push(self.utf16 + units);
        self.utf16 += units;
    }

    /// Inserts `html` at the end of the element at `path`: as a text node when it is text.
    fn insert(&mut self, path: &[u32], html: &str) {
        if html.contains('<') {
            self.path(op::INSERT, path);
            self.string(html);
        } else {
            self.path(op::INSERT_TEXT, path);
            self.string(&decode(html));
        }
    }

    fn path(&mut self, opcode: u32, path: &[u32]) {
        self.ops.push(opcode);
        self.ops.push(path.len() as u32);
        self.ops.extend_from_slice(path);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Open(&'a str),
    Close,
    Void(&'a str),
    Text(&'a str),
}

struct Tokens<'a> {
    html: &'a str,
    at: usize,
}

fn void(tag: &str) -> bool {
    let name = tag[1..].split([' ', '>']).next().unwrap_or("");
    matches!(name, "br" | "hr" | "img" | "input")
}

impl<'a> Iterator for Tokens<'a> {
    type Item = (usize, Token<'a>);
    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.html.as_bytes();
        let start = self.at;
        if start >= bytes.len() {
            return None;
        }
        if bytes[start] == b'<' {
            // Attribute values are quoted, and may hold `>`.
            let mut at = start + 1;
            let mut quoted = false;
            while at < bytes.len() && (quoted || bytes[at] != b'>') {
                quoted ^= bytes[at] == b'"';
                at += 1;
            }
            self.at = at + 1;
            let tag = &self.html[start..self.at];
            let token = if bytes[start + 1] == b'/' {
                Token::Close
            } else if void(tag) {
                Token::Void(tag)
            } else {
                Token::Open(tag)
            };
            return Some((start, token));
        }
        let end = memchr::memchr(b'<', &bytes[start..]).map_or(bytes.len(), |at| start + at);
        self.at = end;
        Some((start, Token::Text(&self.html[start..end])))
    }
}

fn decode(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// How many nodes `html` puts at its top level.
pub fn top_level_nodes(html: &str) -> u32 {
    let mut depth = 0u32;
    let mut count = 0;
    for (_, token) in (Tokens { html, at: 0 }) {
        match token {
            Token::Open(_) => {
                count += u32::from(depth == 0);
                depth += 1;
            }
            Token::Close => depth -= 1,
            Token::Void(_) | Token::Text(_) => count += u32::from(depth == 0),
        }
    }
    count
}

/// The program that turns the DOM of `old` into that of `new`.
pub fn diff(old: &str, new: &str, program: &mut Program) {
    let mut olds = Tokens { html: old, at: 0 };
    let mut news = Tokens { html: new, at: 0 };
    // The index of the next child at each level.
    let mut path: Vec<u32> = vec![0];
    loop {
        let (a, b) = (olds.next(), news.next());
        match (a, b) {
            (None, None) => return,
            (Some((_, x)), Some((_, y))) if x == y => match x {
                Token::Open(_) => path.push(0),
                Token::Close => {
                    path.pop();
                    *path.last_mut().expect("balanced") += 1;
                }
                Token::Void(_) | Token::Text(_) => *path.last_mut().expect("balanced") += 1,
            },
            // A text node that grew: append to it, and go on comparing what follows.
            (Some((_, Token::Text(x))), Some((_, Token::Text(y)))) if y.starts_with(x) => {
                program.path(op::APPEND, &path);
                program.string(&decode(&y[x.len()..]));
                *path.last_mut().expect("balanced") += 1;
            }
            (a, b) => {
                if a.is_some() {
                    program.path(op::REMOVE, &path);
                }
                let Some((from, _)) = b else { return };
                // The new HTML from the difference on, cut where each open level closes.
                let rest = &new[from..];
                let mut depth = 0usize;
                let mut level_start = 0;
                let mut tokens = Tokens { html: rest, at: 0 };
                let mut levels = path.len();
                while let Some((at, token)) = tokens.next() {
                    match token {
                        Token::Open(_) => depth += 1,
                        Token::Close if depth > 0 => depth -= 1,
                        Token::Close => {
                            // Closes the element at `path[..levels - 1]`.
                            levels -= 1;
                            if at > level_start {
                                program.insert(&path[..levels], &rest[level_start..at]);
                            }
                            level_start = tokens.at;
                        }
                        _ => {}
                    }
                }
                if level_start < rest.len() {
                    program.insert(&[], &rest[level_start..]);
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(old: &str, new: &str) -> (Vec<u32>, String) {
        let mut program = Program::default();
        diff(old, new, &mut program);
        (program.ops, program.text)
    }

    #[test]
    fn growing_text_is_one_append() {
        let (ops, text) = run("<p>ab <em>c</em></p>", "<p>ab <em>cd</em></p>");
        assert_eq!(ops, [op::APPEND, 3, 0, 1, 0, 0, 1]);
        assert_eq!(text, "d");
    }

    #[test]
    fn a_changed_element_is_replaced_from_there() {
        let (ops, text) = run("<p>a *b</p>", "<p>a <em>b</em></p>");
        assert_eq!(ops, [op::REMOVE, 2, 0, 0, op::INSERT, 1, 0, 0, 12]);
        assert_eq!(text, "a <em>b</em>");
    }

    #[test]
    fn growing_text_then_more_after_it() {
        let (ops, text) = run("<p><em>a</em></p>", "<p><em>ab</em> c</p>");
        assert_eq!(ops, [op::APPEND, 3, 0, 0, 0, 0, 1, op::REMOVE, 2, 0, 1, op::INSERT_TEXT, 1, 0, 1, 3]);
        assert_eq!(text, "b c");
    }

    #[test]
    fn new_blocks_are_inserted_at_the_root() {
        let (ops, text) = run("<p>a</p>", "<p>a</p><p>b</p>");
        assert_eq!(ops, [op::INSERT, 0, 0, 8]);
        assert_eq!(text, "<p>b</p>");
    }
}
