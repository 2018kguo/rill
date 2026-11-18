//! The DOM edit from one rendering of the open tail to the next, as a small program
//! for js/src/stream-dom.ts.
//!
//! Both renderings are token streams (see `tokens`), so the DOM holds exactly the old
//! one's nodes. Walking the two token by token finds the first difference, with its
//! node's path (child indices, the first relative to the tail's first node). The usual
//! update lengthens one text node: an `APPEND` of the new characters, and the walk goes
//! on. Any other difference removes the old nodes from there on and builds the new
//! tokens from there on, one `BUILD` per level, deepest first. Both refer to the new
//! tokens and text in place: nothing is copied.

use crate::tokens::{NOWHERE, kind};

/// Opcodes.
pub mod op {
    /// `depth, path..., start, end`: append the new text's units `[start, end)` after the
    /// text node at the path (with [`SOURCE`] set in `start`: the text as given's).
    pub const APPEND: u32 = 0;
    /// A range in the text as given, not in the rendering's text.
    pub const SOURCE: u32 = 1 << 31;
    /// `depth, path...`: remove the node at the path, the siblings after it, and the
    /// siblings after each of its ancestors (up to the tail).
    pub const REMOVE: u32 = 1;
    /// `depth, path..., start, end, text`: build the new tokens `[start, end)`, their text
    /// from unit `text`, at the end of the element at the path (depth 0: the root).
    pub const BUILD: u32 = 2;
}

/// The program, and the lowest text unit it refers to.
#[derive(Default)]
pub struct Program {
    pub ops: Vec<u32>,
    pub text_from: u32,
}

impl Program {
    pub fn clear(&mut self) {
        self.ops.clear();
        self.text_from = u32::MAX;
    }

    fn path(&mut self, opcode: u32, path: &[u32]) {
        self.ops.push(opcode);
        self.ops.push(path.len() as u32);
        self.ops.extend_from_slice(path);
    }

    fn text(&mut self, at: usize) {
        self.text_from = self.text_from.min(at as u32);
    }
}

/// A token boundary in the last new rendering (word and text unit), with the path there:
/// the next diff, when its old rendering is that one, starts walking from it.
#[derive(Default)]
pub struct Mark {
    word: usize,
    text: usize,
    path: Vec<u32>,
    /// A buffer for the next diff's walk.
    spare: Vec<u32>,
}

impl Mark {
    pub fn clear(&mut self) {
        self.word = 0;
        self.text = 0;
        self.path.clear();
    }
}

/// A rendering: tokens and their text.
#[derive(Clone, Copy)]
pub struct Tokens<'a> {
    pub words: &'a [u32],
    pub text: &'a [u16],
    /// Per word, the text token's origin (see `tokens::Out`).
    pub origins: &'a [u32],
}

fn step(path: &mut Vec<u32>, word: u32) {
    match word & 3 {
        kind::OPEN => path.push(0),
        kind::CLOSE => {
            path.pop();
            *path.last_mut().expect("balanced") += 1;
        }
        _ => *path.last_mut().expect("balanced") += 1,
    }
}

/// The program that turns the DOM of `old` into that of `new` (positions in `program`
/// are in `new`). `mark` is a boundary in `old` (from the diff that produced it, or
/// cleared), and is left at the last boundary before the first difference in `new`.
/// The rendered text equals the text as given up to `agree`.
pub fn diff(old: Tokens<'_>, new: Tokens<'_>, agree: usize, program: &mut Program, mark: &mut Mark) {
    let mut path = std::mem::take(&mut mark.spare);
    path.clear();
    let (mut word, mut text) = (mark.word, mark.text);
    // The mark holds when everything before it is the same in both.
    let holds = word > 0
        && word <= old.words.len().min(new.words.len())
        && text <= old.text.len().min(new.text.len())
        && old.words[..word] == new.words[..word]
        && old.text[..text] == new.text[..text];
    if holds {
        path.extend_from_slice(&mark.path);
    } else {
        (word, text) = (0, 0);
        path.push(0);
    }
    let (mut old_word, mut old_text) = (word, text);
    let mut first = true;
    loop {
        let (a, b) = (old.words.get(old_word).copied(), new.words.get(word).copied());
        if first {
            mark.word = word;
            mark.text = text;
            mark.path.clear();
            mark.path.extend_from_slice(&path);
        }
        match (a, b) {
            (None, None) => break,
            (Some(x), Some(y)) if x == y => {
                if x & 3 == kind::TEXT {
                    let units = (x >> 2) as usize;
                    if old.text[old_text..old_text + units] != new.text[text..text + units] {
                        replace(true, new, word, text, &mut path, program);
                        break;
                    }
                    old_text += units;
                    text += units;
                }
                step(&mut path, x);
                old_word += 1;
                word += 1;
            }
            // A text node that grew: append to it, and go on comparing what follows.
            (Some(x), Some(y))
                if x & 3 == kind::TEXT
                    && y & 3 == kind::TEXT
                    && y >> 2 > x >> 2
                    && old.text[old_text..old_text + (x >> 2) as usize]
                        == new.text[text..text + (x >> 2) as usize] =>
            {
                first = false;
                let (old_units, units) = ((x >> 2) as usize, (y >> 2) as usize);
                program.path(op::APPEND, &path);
                let origin = new.origins[word];
                if origin != NOWHERE && origin as usize + units <= agree {
                    program.ops.push((origin + old_units as u32) | op::SOURCE);
                    program.ops.push(origin + units as u32);
                } else {
                    program.ops.push((text + old_units) as u32);
                    program.ops.push((text + units) as u32);
                    program.text(text + old_units);
                }
                old_text += old_units;
                text += units;
                step(&mut path, y);
                old_word += 1;
                word += 1;
            }
            (_, _) => {
                // Nothing to remove when the old rendering only closes elements from here.
                let remove = old.words[old_word..].iter().any(|&word| word & 3 != kind::CLOSE);
                replace(remove, new, word, text, &mut path, program);
                break;
            }
        }
    }
    mark.spare = path;
}

/// Removes the old nodes from `path` on (when there are any) and builds the new tokens
/// from `word` (text from `text`) on, cut where each open level closes.
fn replace(remove: bool, new: Tokens<'_>, word: usize, text: usize, path: &mut [u32], program: &mut Program) {
    if remove {
        program.path(op::REMOVE, path);
    }
    let mut depth = 0usize;
    let mut levels = path.len();
    let (mut start, mut start_text) = (word, text);
    let mut at_text = text;
    for (index, &token) in new.words.iter().enumerate().skip(word) {
        match token & 3 {
            kind::OPEN => depth += 1,
            kind::CLOSE if depth > 0 => depth -= 1,
            kind::CLOSE => {
                // Closes the element at `path[..levels - 1]`.
                levels -= 1;
                if index > start {
                    build(program, &path[..levels], start, index, start_text);
                }
                (start, start_text) = (index + 1, at_text);
            }
            kind::TEXT => at_text += (token >> 2) as usize,
            _ => {}
        }
    }
    if start < new.words.len() {
        build(program, &[], start, new.words.len(), start_text);
    }
}

fn build(program: &mut Program, path: &[u32], start: usize, end: usize, text: usize) {
    program.path(op::BUILD, path);
    program.ops.extend_from_slice(&[start as u32, end as u32, text as u32]);
    program.text(text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::{Out, tag};

    fn run(old: &Out, new: &Out) -> Vec<u32> {
        let mut program = Program::default();
        program.clear();
        fn view(out: &Out) -> Tokens<'_> {
            Tokens { words: &out.words, text: &out.text, origins: &out.origins }
        }
        diff(view(old), view(new), 0, &mut program, &mut Mark::default());
        program.ops
    }

    fn para(parts: &[(&str, bool)]) -> Out {
        let mut out = Out::default();
        out.open(tag::P);
        for &(text, em) in parts {
            if em {
                out.open(tag::EM);
                out.ascii(text);
                out.close();
            } else {
                out.ascii(text);
            }
        }
        out.close();
        out
    }

    #[test]
    fn growing_text_is_one_append() {
        let ops = run(&para(&[("ab ", false), ("c", true)]), &para(&[("ab ", false), ("cd", true)]));
        assert_eq!(ops, [op::APPEND, 3, 0, 1, 0, 4, 5]);
    }

    #[test]
    fn a_changed_element_is_replaced_from_there() {
        let ops = run(&para(&[("a *b", false)]), &para(&[("a ", false), ("b", true)]));
        // The text differs: remove it, build `a `, `<em>b</em>` in the paragraph.
        assert_eq!(ops, [op::REMOVE, 2, 0, 0, op::BUILD, 1, 0, 1, 5, 0]);
    }

    #[test]
    fn growing_text_then_more_after_it() {
        let ops = run(&para(&[("a", true)]), &para(&[("ab", true), (" c", false)]));
        assert_eq!(ops, [op::APPEND, 3, 0, 0, 0, 1, 2, op::BUILD, 1, 0, 4, 5, 2]);
    }

    #[test]
    fn new_blocks_are_built_at_the_root() {
        let mut new = para(&[("a", false)]);
        new.open(tag::P);
        new.ascii("b");
        new.close();
        let ops = run(&para(&[("a", false)]), &new);
        assert_eq!(ops, [op::BUILD, 0, 3, 6, 1]);
    }
}
