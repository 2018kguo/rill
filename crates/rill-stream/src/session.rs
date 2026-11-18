//! A streaming answer: the text arrives a piece at a time and each update renders only
//! what can still change.
//!
//! The text is repaired as Streamdown repairs it (remend, kept incremental). Blocks
//! before the last blank line are final: their HTML is handed over once, as the
//! update's `addition`, and never rendered again. The rest is rendered on every update
//! as `pending`, to replace the previous `pending`.

use rill_remend::Stream as Remend;

use crate::diff::{Program, diff, top_level_nodes};
use crate::{Fallback, render_split};

#[derive(Default)]
pub struct Session {
    remend: Remend,
    /// The repaired text so far, as UTF-16 units (as remend works on them).
    healed: Vec<u16>,
    /// `healed[..stable]` is final and in the DOM already.
    stable: usize,
    pub addition: String,
    pub pending: String,
    /// The open tail's HTML as the DOM shows it.
    shown: String,
    scratch: String,
    /// The DOM edit for a `Patch`, from `shown` to `addition` then `pending`.
    pub program: Program,
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
        let (keep, middle) = self.remend.heal_with(appended, reset, |middle, _| middle.to_vec());
        let mut outcome = if fresh { Outcome::Reset } else { Outcome::Patch };
        if fresh || keep < self.stable {
            // A new text, or a repair reaching back into final blocks: render it all.
            self.stable = 0;
            outcome = Outcome::Reset;
        }
        self.healed.truncate(keep);
        self.healed.extend_from_slice(&middle);
        match self.render_tail() {
            Ok(()) => {
                self.program.clear();
                if outcome == Outcome::Patch {
                    let mut next = std::mem::take(&mut self.scratch);
                    next.clear();
                    next.push_str(&self.addition);
                    next.push_str(&self.pending);
                    diff(&self.shown, &next, &mut self.program);
                    self.scratch = next;
                }
                self.final_nodes = top_level_nodes(&self.addition);
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
        let tail = String::from_utf16(&self.healed[self.stable..]).map_err(|_| Fallback)?;
        // What comes before the last block is final.
        let (source, html) = render_split(&tail, &mut self.pending)?;
        self.addition.push_str(&self.pending[..html]);
        self.pending.drain(..html);
        self.stable += tail[..source].encode_utf16().count();
        Ok(())
    }
}
