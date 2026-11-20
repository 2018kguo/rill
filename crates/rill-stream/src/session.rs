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
use crate::{Fallback, Split, render_split_at};

#[derive(Default)]
pub struct Session {
    remend: Remend,
    /// The repaired text (remend's, in UTF-16 units) is final and in the DOM up to
    /// `stable`; the rest is rendered on every update.
    stable: usize,
    /// The repaired text equals the text as given up to here.
    pub agree: usize,
    /// The repaired text is as it was for the last update up to here.
    unchanged: usize,
    /// The final blocks ended because of what the open tail's first line is: a repair
    /// that changes the text before this reaches into them.
    decides: usize,
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
        if fresh || keep < self.stable || keep < self.decides {
            // A new text, or a repair reaching back into final blocks: render it all.
            self.stable = 0;
            outcome = Outcome::Reset;
        }
        // A lone surrogate takes the exact path.
        let (healed, input) = (self.remend.healed(), self.remend.input());
        // The repair keeps `..keep` of the last repaired text, and the text only grows.
        let limit = healed.len().min(input.len());
        let mut agree = if outcome == Outcome::Reset { 0 } else { self.agree.min(keep) };
        while agree < limit && healed[agree] == input[agree] {
            agree += 1;
        }
        self.agree = agree;
        self.unchanged = if outcome == Outcome::Reset { 0 } else { keep };
        let checked = &healed[if outcome == Outcome::Reset { 0 } else { keep }..];
        let surrogates = checked.iter().fold(false, |any, &unit| any | (unit & 0xF800 == 0xD800));
        if surrogates && char::decode_utf16(checked.iter().copied()).any(|c| c.is_err()) {
            self.failed = true;
            return Outcome::Fallback;
        }
        match self.render_tail() {
            Ok(()) => {
                self.program.clear();
                if outcome == Outcome::Patch {
                    let old = Tokens {
                        words: &self.shown.words[self.shown_from.words..],
                        text: &self.shown.text[self.shown_from.text..],
                        origins: &self.shown.origins[self.shown_from.words..],
                        source: self.remend.input(),
                    };
                    let new = Tokens {
                        words: &self.next.words,
                        text: &self.next.text,
                        origins: &self.next.origins,
                        source: self.remend.input(),
                    };
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
        let shown = &self.shown;
        let strings = shown.strings.borrow();
        crate::tokens::to_html(&shown.words, &shown.text, &shown.origins, self.remend.input(), &strings, &mut html);
        html
    }

    /// The interned strings.
    pub fn strings(&self) -> std::cell::Ref<'_, crate::tokens::Strings> {
        self.shown.strings.borrow()
    }

    fn render_tail(&mut self) -> crate::Result {
        self.next.clear();
        // Both renderings name the same strings.
        if !std::rc::Rc::ptr_eq(&self.next.strings, &self.shown.strings) {
            self.next.strings = self.shown.strings.clone();
        }
        // What comes before the last block is final.
        let split = render_split_at(
            &self.remend.healed()[self.stable..],
            self.stable,
            self.unchanged,
            self.agree,
            &mut self.next,
            &mut self.inline,
        )?;
        self.final_nodes = split.nodes;
        if split.source > 0 || self.stable == 0 {
            self.decides = if split.decides > 0 { self.stable + split.decides } else { 0 };
        }
        self.split = split;
        self.stable += split.source;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    /// After every chunk, the session's HTML is the whole repaired text rendered from
    /// scratch: what each update keeps (final blocks, the inline resume point) is what
    /// rendering again would make. Random texts dense in what the renderer handles.
    #[test]
    fn streaming_render_is_whole_text_render() {
        const PIECES: [&str; 42] = [
            "a", "b", "word", " ", " ", "  ", "\n", "\n\n", "*", "**", "***", "_", "__", "`", "``", "\\",
            "&", "<", "é", "😀", "# ", "- ", "1. ", "---", "x.", ":", "  \n", "www.", "=", "\\*",
            "[", "]", "(", ")", "](", "[x](/u)", "<a:b>", "&amp;", "&#", ";", "](javascript:x)", "\"t\"",
        ];
        let mut seed = std::env::var("STREAM_SEED").map_or(0x9e37_79b9_7f4a_7c15u64, |s| s.parse().unwrap());
        let mut next = |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound as u64) as usize
        };
        let rounds = std::env::var("STREAM_ROUNDS").map_or(3000, |n| n.parse().unwrap());
        for _ in 0..rounds {
            let mut text = String::new();
            for _ in 0..next(60) {
                text.push_str(PIECES[next(PIECES.len())]);
            }
            let mut session = super::Session::default();
            // The blocks earlier updates made final, as the DOM keeps them.
            let mut kept = String::new();
            let mut start = 0;
            while start < text.len() {
                let mut end = (start + 1 + next(16)).min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                let outcome = session.update(&text[start..end], start == 0);
                if outcome == super::Outcome::Reset {
                    kept.clear();
                }
                if outcome != super::Outcome::Fallback {
                    let (shown, split) = (&session.shown, session.shown_from);
                    let input = session.remend.input();
                    let strings = shown.strings.borrow();
                    let (words, origins) = (&shown.words[..split.words], &shown.origins[..split.words]);
                    crate::tokens::to_html(words, &shown.text[..split.text], origins, input, &strings, &mut kept);
                    let mut now = kept.clone();
                    let (words, origins) = (&shown.words[split.words..], &shown.origins[split.words..]);
                    crate::tokens::to_html(words, &shown.text[split.text..], origins, input, &strings, &mut now);
                    let healed = String::from_utf16(session.remend.healed()).expect("no lone surrogates");
                    let mut whole = String::new();
                    if crate::render(&healed, &mut whole).is_ok() {
                        assert_eq!(now, whole, "{:?}", &text[..end]);
                    }
                }
                start = end;
            }
        }
    }
}
