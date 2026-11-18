//! A streaming answer: the text arrives a piece at a time and each update renders only
//! what can still change.
//!
//! The text is repaired as Streamdown repairs it (remend, kept incremental). Blocks
//! before the last blank line are final: their HTML is handed over once, as the
//! update's `addition`, and never rendered again. The rest is rendered on every update
//! as `pending`, to replace the previous `pending`.

use rill_remend::Stream as Remend;

use crate::diff::{Mark, Program, diff};
use crate::inline::Inline;
use crate::{Fallback, render_split_with};

#[derive(Default)]
pub struct Session {
    remend: Remend,
    /// The repaired text (remend's, in UTF-16 units) is final and in the DOM up to
    /// `stable`; `tail` is the rest, as UTF-8.
    stable: usize,
    tail: String,
    /// The repaired text's length after the last update.
    healed: usize,
    /// This update's HTML: the blocks that became final (`..split`), then the open ones.
    html: String,
    split: usize,
    /// The last update's HTML: `shown[shown_from..]` is the open tail as the DOM shows it.
    shown: String,
    shown_from: usize,
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
                    diff(&self.shown[self.shown_from..], &self.html, &mut self.program, &mut self.mark);
                }
                // The mark is in `html`; the next diff starts from `html[split..]`.
                if outcome != Outcome::Patch || self.split != 0 {
                    self.mark.clear();
                }
                std::mem::swap(&mut self.shown, &mut self.html);
                self.shown_from = self.split;
                outcome
            }
            Err(Fallback) => {
                self.failed = true;
                Outcome::Fallback
            }
        }
    }

    /// The HTML of the last update: the blocks that became final, then the open ones.
    pub fn html(&self) -> (&str, &str) {
        self.shown.split_at(self.shown_from)
    }

    fn render_tail(&mut self) -> crate::Result {
        self.html.clear();
        // What comes before the last block is final.
        let (source, html, nodes) = render_split_with(&self.tail, &mut self.html, &mut self.inline)?;
        self.final_nodes = nodes;
        self.split = html;
        self.stable += self.tail[..source].encode_utf16().count();
        self.tail.drain(..source);
        Ok(())
    }
}
