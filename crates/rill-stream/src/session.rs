//! A streaming answer: the text arrives a piece at a time and each update renders only
//! what can still change.
//!
//! The text is repaired as Streamdown repairs it (remend, kept incremental). Blocks
//! before the last blank line are final: their HTML is handed over once, as the
//! update's `addition`, and never rendered again. The rest is rendered on every update
//! as `pending`, to replace the previous `pending`.

use rill_remend::Stream as Remend;

use crate::diff::{Mark, Program, diff};
use crate::{Fallback, render_split};

#[derive(Default)]
pub struct Session {
    remend: Remend,
    /// The repaired text (remend's, in UTF-16 units) is final and in the DOM up to
    /// `stable`; `tail` is the rest, as UTF-8.
    stable: usize,
    tail: String,
    /// The repaired text's length after the last update.
    healed: usize,
    pub addition: String,
    pub pending: String,
    /// The open tail's HTML as the DOM shows it.
    shown: String,
    scratch: String,
    /// The DOM edit for a `Patch`, from `shown` to `addition` then `pending`.
    pub program: Program,
    /// Where the next diff may start walking `shown`.
    mark: Mark,
    /// How many root nodes of `addition` became final.
    pub final_nodes: u32,
    /// The last update fell back: the next one starts over.
    failed: bool,
}

/// What an update did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Append `addition`, then replace the pending nodes with `pending`.
    Patch = 0,
    /// Replace everything with `addition` then `pending`.
    Reset = 1,
    /// The text needs the exact path.
    Fallback = 2,
}

impl Session {
    /// Renders the text so far plus `appended` (with `reset`, `appended` is the whole text).
    pub fn update(&mut self, appended: &str, reset: bool) -> Outcome {
        // After a fallback, remend's state still holds: only the rendering starts over.
        let fresh = reset || self.failed;
        self.failed = false;
        let (keep, ()) = self.remend.heal_with(appended, reset, |_, _| ());
        let mut outcome = if fresh { Outcome::Reset } else { Outcome::Patch };
        let from = if fresh || keep < self.stable {
            // A new text, or a repair reaching back into final blocks: render it all.
            self.stable = 0;
            self.tail.clear();
            outcome = Outcome::Reset;
            0
        } else {
            // Drop what the repair took back (units `keep..` of the last text) from
            // the end of the tail: a few characters.
            let mut units = self.healed - keep;
            let mut cut = self.tail.len();
            for c in self.tail.chars().rev() {
                if units == 0 {
                    break;
                }
                units -= c.len_utf16();
                cut -= c.len_utf8();
            }
            self.tail.truncate(cut);
            keep
        };
        let healed = self.remend.healed();
        self.healed = healed.len();
        for c in char::decode_utf16(healed[from..].iter().copied()) {
            let Ok(c) = c else {
                self.failed = true;
                return Outcome::Fallback;
            };
            self.tail.push(c);
        }
        match self.render_tail() {
            Ok(()) => {
                self.program.clear();
                if outcome == Outcome::Patch {
                    let mut next = std::mem::take(&mut self.scratch);
                    next.clear();
                    next.push_str(&self.addition);
                    next.push_str(&self.pending);
                    diff(&self.shown, &next, &mut self.program, &mut self.mark);
                    self.scratch = next;
                }
                // The mark is in `addition + pending`; `shown` becomes `pending`.
                if outcome != Outcome::Patch || !self.addition.is_empty() {
                    self.mark.clear();
                }
                self.shown.clear();
                self.shown.push_str(&self.pending);
                outcome
            }
            Err(Fallback) => {
                self.failed = true;
                Outcome::Fallback
            }
        }
    }

    fn render_tail(&mut self) -> crate::Result {
        self.addition.clear();
        self.pending.clear();
        // What comes before the last block is final.
        let (source, html, nodes) = render_split(&self.tail, &mut self.pending)?;
        self.final_nodes = nodes;
        self.addition.push_str(&self.pending[..html]);
        self.pending.drain(..html);
        self.stable += self.tail[..source].encode_utf16().count();
        self.tail.drain(..source);
        Ok(())
    }
}
