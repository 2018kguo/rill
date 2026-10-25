//! Streamdown 2.7.1's `animated` (lib/animate.ts): while an answer streams, each word
//! (or character) of the text is a `<span data-sd-animate>` whose style names the
//! animation, its duration, easing and delay, with the words already shown at duration 0
//! and the new ones staggered on a timeline shared by the message's blocks. List items
//! fade their marker in with their first word, and checkboxes, images and rules are
//! stamped with the same timing.
//!
//! This is the plugin's pass on rill's finished tree, before the markup is written. The
//! timeline's cursor lives in JavaScript (one per message) and crosses each call.

use std::borrow::Cow;

use crate::hast::{self, NodeKind, Tree, Value};
use crate::mdast::NodeId;

/// One block's animation pass: the `animated` options and the timeline's state.
#[derive(Clone, Debug, Default)]
pub struct Animate {
    /// `animation`, as given (`fadeIn` by default): the span's `--sd-animation` is
    /// `sd-<animation>`.
    pub animation: String,
    /// `duration` in milliseconds as JavaScript writes the number (`String(150)`).
    pub duration: String,
    pub easing: String,
    /// `sep: "char"`: a unit per character instead of per word.
    pub by_char: bool,
    /// `stagger` in milliseconds.
    pub stagger: f64,
    /// UTF-16 units of text already shown (the plugin's `prevContentLength`): units that
    /// start before it render at duration 0.
    pub shown: u32,
    /// The timeline's cursor (`passNextStartAt`), the clock (`now()`), and its
    /// `maxBacklogMs`.
    pub cursor: f64,
    pub now: f64,
    pub backlog: f64,
}

/// What a pass leaves for the next one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Animated {
    /// UTF-16 units of text the block holds (`getLastRenderCharCount`).
    pub chars: u32,
    /// The timeline's cursor after this block took its words.
    pub cursor: f64,
}

/// Elements whose subtrees are left alone (code blocks, KaTeX's SVG and MathML).
fn skipped(tag: &str) -> bool {
    matches!(tag, "pre" | "svg" | "math" | "annotation")
}

/// Elements with no text that animate in themselves.
fn void_animated(tag: &str) -> bool {
    matches!(tag, "img" | "hr")
}

/// JavaScript's `\s` (and what `trim()` removes).
fn space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// Han, Hiragana and Katakana (by Script), CJK punctuation and fullwidth forms: the
/// plugin's `CJK_RE`, generated from V8 (Unicode 17).
const CJK: &[(u32, u32)] = &[
    (0x2e80, 0x2e99),
    (0x2e9b, 0x2ef3),
    (0x2f00, 0x2fd5),
    (0x3000, 0x303f),
    (0x3041, 0x3096),
    (0x309d, 0x309f),
    (0x30a1, 0x30fa),
    (0x30fd, 0x30ff),
    (0x31f0, 0x31ff),
    (0x32d0, 0x32fe),
    (0x3300, 0x3357),
    (0x3400, 0x4dbf),
    (0x4e00, 0x9fff),
    (0xf900, 0xfa6d),
    (0xfa70, 0xfad9),
    (0xff00, 0xffef),
    (0x16fe2, 0x16fe3),
    (0x16ff0, 0x16ff6),
    (0x1aff0, 0x1aff3),
    (0x1aff5, 0x1affb),
    (0x1affd, 0x1affe),
    (0x1b000, 0x1b122),
    (0x1b132, 0x1b132),
    (0x1b150, 0x1b152),
    (0x1b155, 0x1b155),
    (0x1b164, 0x1b167),
    (0x1f200, 0x1f200),
    (0x20000, 0x2a6df),
    (0x2a700, 0x2b81d),
    (0x2b820, 0x2cead),
    (0x2ceb0, 0x2ebe0),
    (0x2ebf0, 0x2ee5d),
    (0x2f800, 0x2fa1d),
    (0x30000, 0x3134a),
    (0x31350, 0x33479),
];

fn cjk(c: char) -> bool {
    let c = c as u32;
    if c < 0x2e80 {
        return false;
    }
    match CJK.binary_search_by(|&(start, end)| {
        if end < c {
            std::cmp::Ordering::Less
        } else if start > c {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        Ok(_) => true,
        Err(_) => false,
    }
}

fn units(text: &str) -> u32 {
    text.chars().map(|c| c.len_utf16() as u32).sum()
}

fn blank(text: &str) -> bool {
    text.chars().all(space)
}

/// The plugin's `splitByWord`: a unit per run of non-space with the spaces after it,
/// every CJK character its own unit. Byte ranges into `text`.
fn split_by_word(text: &str, out: &mut Vec<(usize, usize)>) {
    let mut start = 0;
    let mut empty = true;
    let mut in_space = false;
    let mut previous_cjk = false;
    for (index, c) in text.char_indices() {
        let is_space = space(c);
        let is_cjk = !is_space && cjk(c);
        let starts_unit = !is_space && !empty && (is_cjk || previous_cjk);
        if starts_unit {
            out.push((start, index));
            start = index;
        } else if is_space != in_space && !empty {
            if is_space {
                in_space = true;
                continue;
            }
            out.push((start, index));
            start = index;
        }
        empty = false;
        in_space = is_space;
        previous_cjk = is_cjk;
    }
    if !empty {
        out.push((start, text.len()));
    }
}

/// The plugin's `splitByChar`: a unit per character, spaces joining the one before.
fn split_by_char(text: &str, out: &mut Vec<(usize, usize)>) {
    // A run of spaces before any other character, not yet a unit.
    let mut spaces: Option<(usize, usize)> = None;
    for (index, c) in text.char_indices() {
        let end = index + c.len_utf8();
        if space(c) {
            match out.last_mut() {
                Some(last) if !blank(&text[last.0..last.1]) => last.1 = end,
                _ => {
                    spaces = Some(spaces.map_or((index, end), |(start, _)| (start, end)));
                }
            }
        } else {
            if let Some(run) = spaces.take() {
                out.push(run);
            }
            out.push((index, end));
        }
    }
    if let Some((start, end)) = spaces {
        match out.last_mut() {
            Some(last) => last.1 = end,
            None => out.push((start, end)),
        }
    }
}

/// Words or characters, as byte ranges into `text`.
fn parts(text: &str, by_char: bool, out: &mut Vec<(usize, usize)>) {
    out.clear();
    if by_char {
        split_by_char(text, out);
    } else {
        split_by_word(text, out);
    }
}

/// The timeline's `take`: the first new word's delay and the stagger between words.
fn take(animate: &Animate, cursor: &mut f64, words: u32) -> (f64, f64) {
    let ideal = animate.stagger.max(0.0);
    if words == 0 {
        return (0.0, ideal);
    }
    let floor = if ideal == 0.0 { 0.0 } else { ideal.min(4.0) };
    let now = animate.now;
    let start = cursor.max(now);
    let budget_end = now + animate.backlog;
    let ideal_last = start + f64::from(words.saturating_sub(1)) * ideal;
    let mut step = ideal;
    if ideal_last > budget_end && words > 1 {
        step = if start < budget_end {
            floor.max((budget_end - start) / f64::from(words - 1))
        } else {
            floor
        };
    }
    let base = (start - now).round().max(0.0);
    *cursor = start + f64::from(words) * step;
    (base, step)
}

/// JavaScript's `Math.round` for the non-negative delays here, as an integer string.
fn milliseconds(value: f64) -> String {
    format!("{}ms", value.round() as i64)
}

struct Pass<'a, 'b, 's> {
    tree: &'b mut Tree<'s>,
    animate: &'a Animate,
    /// UTF-16 units so far (`charCounter.count`).
    count: u32,
    /// New units so far (`charCounter.newIndex`).
    fresh: u32,
    base: f64,
    step: f64,
    /// Element ancestors of the node being visited.
    ancestors: Vec<NodeId>,
    ranges: Vec<(usize, usize)>,
}

impl<'s> Pass<'_, '_, 's> {
    fn shown(&self, start: u32) -> bool {
        self.animate.shown > 0 && start < self.animate.shown
    }

    /// The units that will animate (`countNewWords`).
    fn count_new(&mut self, ix: NodeId, new: &mut u32) {
        let children = self.tree.child_list(ix);
        for child in children {
            match &self.tree.nodes[child as usize].kind {
                NodeKind::Text(text) => {
                    let text: &str = text;
                    if blank(text) {
                        self.count += units(text);
                        continue;
                    }
                    let mut ranges = std::mem::take(&mut self.ranges);
                    parts(text, self.animate.by_char, &mut ranges);
                    for &(start, end) in &ranges {
                        let part = &text[start..end];
                        let at = self.count;
                        self.count += units(part);
                        if !blank(part) && !self.shown(at) {
                            *new += 1;
                        }
                    }
                    self.ranges = ranges;
                }
                NodeKind::Element { tag, .. } if skipped(tag) => {}
                NodeKind::Element { tag, .. } => {
                    if void_animated(tag) {
                        if !self.shown(self.count) {
                            *new += 1;
                        }
                        self.count += 1;
                    }
                    self.count_new(child, new);
                }
                _ => {}
            }
        }
    }

    fn style(&self, delay: f64, shown: bool, marker: bool) -> String {
        let duration = if shown { "0" } else { &self.animate.duration };
        let easing = &self.animate.easing;
        let delay = milliseconds(delay);
        if marker {
            format!(
                "--sd-marker-duration:{duration}ms;--sd-marker-delay:{delay};--sd-marker-easing:{easing}"
            )
        } else {
            format!(
                "--sd-animation:sd-{};--sd-duration:{duration}ms;--sd-easing:{easing};--sd-delay:{delay}",
                self.animate.animation
            )
        }
    }

    /// `stampMarker` / `stampAnimation`: data attribute and timing, after a style the
    /// element already has.
    fn stamp(&mut self, ix: NodeId, key: &'static str, style: String) {
        let props = self.tree.properties_mut(ix).expect("an element");
        let existing = match hast::get(props, "style") {
            Some(Value::Str(value)) => Some(format!("{value};")),
            _ => None,
        };
        hast::set(props, key, Value::Bool(true));
        let style = match existing {
            Some(existing) => existing + &style,
            None => style,
        };
        hast::set(props, "style", Value::Str(Cow::Owned(style)));
    }

    fn process(&mut self, ix: NodeId) {
        let children = self.tree.child_list(ix);
        let mut out = Vec::with_capacity(children.len());
        let mut changed = false;
        for child in children {
            let tag = self.tree.tag_name(child);
            if let NodeKind::Text(text) = &self.tree.nodes[child as usize].kind {
                let text = text.clone();
                if blank(&text) {
                    self.count += units(&text);
                    out.push(child);
                    continue;
                }
                changed = true;
                self.text(&text, &mut out);
                continue;
            }
            out.push(child);
            let Some(tag) = tag else { continue };
            if skipped(tag) {
                continue;
            }
            if void_animated(tag) {
                let at = self.count;
                self.count += 1;
                let shown = self.shown(at);
                let delay = if shown {
                    0.0
                } else {
                    let delay = self.base + f64::from(self.fresh) * self.step;
                    self.fresh += 1;
                    delay
                };
                let style = self.style(delay, shown, false);
                self.stamp(child, "data-sd-animate", style);
            }
            self.ancestors.push(child);
            self.process(child);
            self.ancestors.pop();
        }
        if changed {
            self.tree.replace_children(ix, &out);
        }
    }

    /// `processTextNode`: the text's units as spans (spaces alone stay text).
    fn text(&mut self, text: &Cow<'s, str>, out: &mut Vec<NodeId>) {
        let mut ranges = std::mem::take(&mut self.ranges);
        parts(text, self.animate.by_char, &mut ranges);
        let item = self
            .ancestors
            .iter()
            .rev()
            .copied()
            .find(|&ancestor| self.tree.tag_name(ancestor) == Some("li"));
        let mut marker = item.filter(|&item| match &self.tree.nodes[item as usize].kind {
            NodeKind::Element { props, .. } => hast::get(props, "data-sd-animate-marker").is_none(),
            _ => false,
        });
        for &(start, end) in &ranges {
            let part = hast::slice_cow(text, start..end);
            let at = self.count;
            self.count += units(&part);
            if blank(&part) {
                out.push(self.tree.add(NodeKind::Text(part)));
                continue;
            }
            let shown = self.shown(at);
            let delay = if shown {
                0.0
            } else {
                let delay = self.base + f64::from(self.fresh) * self.step;
                self.fresh += 1;
                delay
            };
            if let Some(item) = marker.take() {
                let style = self.style(delay, shown, true);
                self.stamp(item, "data-sd-animate-marker", style);
                if let Some(input) = self.checkbox(item) {
                    let style = self.style(delay, shown, false);
                    self.stamp(input, "data-sd-animate", style);
                }
            }
            let duration = if shown { "0" } else { &self.animate.duration };
            let mut style = format!(
                "--sd-animation:sd-{};--sd-duration:{duration}ms;--sd-easing:{}",
                self.animate.animation, self.animate.easing
            );
            if delay != 0.0 {
                style.push_str(";--sd-delay:");
                style.push_str(&milliseconds(delay));
            }
            let word = self.tree.add(NodeKind::Text(part));
            let span = self.tree.add(NodeKind::Element {
                tag: "span",
                props: vec![
                    ("data-sd-animate", Value::Bool(true)),
                    ("style", Value::Str(Cow::Owned(style))),
                ],
                data: None,
            });
            self.tree.replace_children(span, &[word]);
            out.push(span);
        }
        // Every part that is not only spaces animates (the text was not blank).
        self.mark_ancestors();
        self.ranges = ranges;
    }

    /// `markAnimatedAncestors`: `data-sd-animated` up to the first ancestor that has it.
    fn mark_ancestors(&mut self) {
        for index in (0..self.ancestors.len()).rev() {
            let ancestor = self.ancestors[index];
            let props = self.tree.properties_mut(ancestor).expect("an element");
            if matches!(
                hast::get(props, "data-sd-animated"),
                Some(Value::Bool(true))
            ) {
                break;
            }
            hast::set(props, "data-sd-animated", Value::Bool(true));
        }
    }

    /// `findCheckbox`: the first `<input>` in a list item, outside nested lists.
    fn checkbox(&self, ix: NodeId) -> Option<NodeId> {
        for child in self.tree.children(ix) {
            match self.tree.tag_name(child) {
                Some("input") => return Some(child),
                Some("ul" | "ol" | "li") | None => {}
                Some(_) => {
                    if let Some(found) = self.checkbox(child) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
}

/// The animate plugin's pass over `root`'s tree.
pub fn animate(tree: &mut Tree<'_>, root: NodeId, animate: &Animate) -> Animated {
    let mut pass = Pass {
        tree,
        animate,
        count: 0,
        fresh: 0,
        base: 0.0,
        step: 0.0,
        ancestors: Vec::new(),
        ranges: Vec::new(),
    };
    let mut new = 0;
    pass.count_new(root, &mut new);
    let mut cursor = animate.cursor;
    let (base, step) = take(animate, &mut cursor, new);
    pass.count = 0;
    pass.base = base;
    pass.step = step;
    pass.process(root);
    Animated {
        chars: pass.count,
        cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::{split_by_char, split_by_word};

    fn words(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        split_by_word(text, &mut out);
        out.into_iter().map(|(a, b)| &text[a..b]).collect()
    }

    fn chars(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        split_by_char(text, &mut out);
        out.into_iter().map(|(a, b)| &text[a..b]).collect()
    }

    #[test]
    fn splits_as_the_plugin_does() {
        assert_eq!(words(" a bb  c"), [" ", "a ", "bb  ", "c"]);
        assert_eq!(words("中文 ab中"), ["中", "文 ", "ab", "中"]);
        assert_eq!(chars("  ab c "), ["  ", "a", "b ", "c "]);
        assert_eq!(chars("   "), ["   "]);
    }
}
