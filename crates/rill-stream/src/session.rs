//! A streaming answer: the text arrives a piece at a time and each update renders only
//! what can still change.
//!
//! The text is repaired as Streamdown repairs it (remend, kept incremental). Blocks
//! before the last open one are final: they are rendered once and never again. The
//! rest is rendered on every update (as tokens, see `tokens`) and diffed against what
//! the DOM shows into a short edit program.

use rill_remend::Stream as Remend;

use crate::diff::{Mark, Program, Tokens, diff};
use crate::inline::Inline;
use crate::tokens::Out;
use crate::{Fallback, Split, render_split};

#[derive(Default)]
pub struct Session {
    remend: Remend,
    /// The repaired text (remend's, in UTF-16 units) is final and in the DOM up to
    /// `stable`; `tail` is the rest, as UTF-8.
    stable: usize,
    tail: String,
    /// The repaired text's length after the last update.
    healed: usize,
    /// This update's rendering: the blocks that became final (before `split`), then the
    /// open ones.
    next: Out,
    split: Split,
    /// The last update's rendering: from `shown_from` on, the open tail as the DOM shows it.
    shown: Out,
    shown_from: Split,
    /// The DOM edit for a `Patch`, from the shown tail to `html`.
    pub program: Program,
    /// Where the next diff may start walking `shown`.
    mark: Mark,
    /// The inline scanner, its buffers kept between updates.
    inline: Inline,
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
        let (keep, ()) = self.remend.heal_with(appended, reset, |_, _| ());
        self.after_heal(keep, reset)
    }

    /// [`Session::update`] for text in UTF-16 units (as JavaScript holds it).
    pub fn update_units(&mut self, appended: &[u16], reset: bool) -> Outcome {
        let (keep, ()) = self.remend.heal_units_with(appended, reset, |_, _| ());
        self.after_heal(keep, reset)
    }

    fn after_heal(&mut self, keep: usize, reset: bool) -> Outcome {
        // After a fallback, remend's state still holds: only the rendering starts over.
        let fresh = reset || self.failed;
        self.failed = false;
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
                    let old = Tokens {
                        words: &self.shown.words[self.shown_from.words..],
                        text: &self.shown.text[self.shown_from.text..],
                    };
                    let new = Tokens { words: &self.next.words, text: &self.next.text };
                    diff(old, new, &mut self.program, &mut self.mark);
                }
                // The mark is in `next`; the next diff starts from `next` after `split`.
                if outcome != Outcome::Patch || self.split.words != 0 {
                    self.mark.clear();
                }
                std::mem::swap(&mut self.shown, &mut self.next);
                self.shown_from = self.split;
                outcome
            }
            Err(Fallback) => {
                self.failed = true;
                Outcome::Fallback
            }
        }
    }

    /// The last update's rendering (the edit program's positions are in it).
    pub fn rendering(&self) -> &Out {
        &self.shown
    }

    /// The last update's HTML (the blocks that became final, then the open ones).
    pub fn html(&self) -> String {
        let mut html = String::new();
        crate::tokens::to_html(&self.shown.words, &self.shown.text, &mut html);
        html
    }

    fn render_tail(&mut self) -> crate::Result {
        self.next.clear();
        // What comes before the last block is final.
        let split = render_split(&self.tail, &mut self.next, &mut self.inline)?;
        self.final_nodes = split.nodes;
        self.split = split;
        self.stable += self.tail[..split.source].encode_utf16().count();
        self.tail.drain(..split.source);
        Ok(())
    }
}
