//! Compiles the parsed tree to a backtracking program.
//!
//! The program follows ECMA-262's pattern semantics: alternatives and quantifiers
//! are tried in priority order, quantified groups reset their captures on every
//! iteration, an iteration past the minimum that matches empty fails, and
//! lookarounds are atomic. Lookbehind bodies are compiled right to left.

use super::charset::CharSet;
use super::parse::{Assertion, Node};

#[derive(Debug, Clone)]
pub enum Inst {
    /// One UTF-16 code unit that is not a surrogate (a BMP character).
    Unit { unit: u16, back: bool },
    /// Either of two BMP code units (an ASCII letter under `i`).
    Unit2 { a: u16, b: u16, back: bool },
    /// One code point from a set.
    Set { set: u32, back: bool },
    /// A run of one-character matches with no captures inside: matched as a loop
    /// that backtracks one character at a time. `follow` (an index into
    /// `Program::follows`, or `NO_FOLLOW`) constrains the code unit right after the
    /// run, so positions where the rest of the pattern cannot match are skipped.
    SetRepeat { set: u32, min: u32, max: u32, greedy: bool, back: bool, possessive: bool, follow: u32 },
    /// Tries the alternatives of `Program::alt_tables[table]` in order.
    AltTable { table: u32 },
    /// Try `next` first; on failure resume at `alt` at the same position.
    Split { alt: u32 },
    Jump { to: u32 },
    Save { slot: u32 },
    /// Sets the captures `from..to` (group numbers) to undefined.
    ClearCaptures { from: u32, to: u32 },
    /// Starts a counted loop: iteration count 0.
    LoopInit { reg: u32 },
    /// Decides whether to run another iteration of a loop whose body starts at
    /// the next instruction and which continues at `exit`.
    LoopTest { reg: u32, min: u32, max: u32, greedy: bool, exit: u32 },
    /// Begins an iteration (taken directly or as the lazy alternative).
    LoopEnter { reg: u32, clear_from: u32, clear_to: u32 },
    /// Ends an iteration: fails if it matched empty past the minimum, else loops.
    LoopEnd { reg: u32, min: u32, test: u32 },
    Assert(Assertion),
    /// Runs the lookaround body at `body` (which ends with `LookEnd`), then goes on
    /// at `next`.
    Look { body: u32, next: u32, negate: bool },
    LookEnd,
    /// Runs the body at `body` (ending with `LookEnd`) once, atomically, and goes on
    /// at `next` from where it ended.
    Atomic { body: u32, next: u32 },
    Backref { groups: u32, ignore_case: bool, back: bool },
    Match,
}

pub const NO_FOLLOW: u32 = u32::MAX;

#[derive(Debug, Clone)]
struct AltEntry {
    pc: u32,
    /// The code unit at the position must be in this set for the alternative to match.
    guard: Option<FirstUnits>,
}

/// Alternatives in priority order with, per code unit class, a bitmask of the
/// alternatives that can match there.
#[derive(Debug, Clone)]
pub struct AltTable {
    pub pcs: Vec<u32>,
    words: usize,
    /// Rows: ASCII units 0-127, then any non-ASCII unit, then end of input.
    masks: Vec<u64>,
}

impl AltTable {
    fn new(entries: &[AltEntry]) -> Self {
        let words = entries.len().div_ceil(64);
        let mut masks = vec![0u64; 130 * words];
        for (i, entry) in entries.iter().enumerate() {
            for row in 0..130usize {
                let viable = match (&entry.guard, row) {
                    (None, _) => true,
                    (Some(_), 129) => false,
                    (Some(guard), 128) => guard.non_ascii,
                    (Some(guard), unit) => guard.may_start(unit as u16),
                };
                if viable {
                    masks[row * words + i / 64] |= 1 << (i % 64);
                }
            }
        }
        Self { pcs: entries.iter().map(|e| e.pc).collect(), words, masks }
    }

    /// The first alternative at or after `from` viable before `unit` (None: end of
    /// input), and whether another one follows it.
    #[inline]
    pub fn next(&self, unit: Option<u16>, from: usize) -> Option<(usize, bool)> {
        let row = match unit {
            Some(u) if u < 128 => u as usize,
            Some(_) => 128,
            None => 129,
        };
        let masks = &self.masks[row * self.words..(row + 1) * self.words];
        let mut word = from / 64;
        let mut found = None;
        while word < self.words {
            let mut bits = masks[word];
            if word == from / 64 {
                bits &= u64::MAX.checked_shl((from % 64) as u32).unwrap_or(0);
            }
            if bits != 0 {
                let index = word * 64 + bits.trailing_zeros() as usize;
                match found {
                    None => {
                        found = Some(index);
                        // Look for a second viable alternative after this one.
                        let rest = bits & !(1u64 << (index % 64));
                        let rest = rest & u64::MAX.checked_shl((index % 64) as u32 + 1).unwrap_or(0);
                        if rest != 0 {
                            return Some((index, true));
                        }
                    }
                    Some(index) => return Some((index, true)),
                }
            }
            word += 1;
        }
        found.map(|index| (index, false))
    }
}

#[derive(Debug, Clone)]
pub struct Program {
    pub insts: Vec<Inst>,
    pub alt_tables: Vec<AltTable>,
    pub follows: Vec<FirstUnits>,
    pub sets: Vec<RuntimeSet>,
    /// Backreference group lists, indexed by `Inst::Backref::groups`.
    pub backref_groups: Vec<Vec<u32>>,
    pub loop_count: u32,
    pub group_count: u32,
    /// Code units that can begin a match, when the pattern cannot match empty.
    pub first_units: Option<FirstUnits>,
    /// Code units every match must contain at or after its start (each element
    /// one of two units), checked before searching.
    pub required_literal: Option<Vec<(u16, u16)>>,
    /// Every match starts at input start.
    pub anchored_start: bool,
}

/// A set with an ASCII bitmap for the common case.
#[derive(Debug, Clone)]
pub struct RuntimeSet {
    ascii: [u64; 2],
    set: CharSet,
    /// Smallest member above ASCII, to reject quickly.
    non_ascii: bool,
}

impl RuntimeSet {
    fn new(set: CharSet) -> Self {
        let mut ascii = [0u64; 2];
        for &(lo, hi) in set.ranges() {
            if lo >= 0x80 {
                break;
            }
            for c in lo..=hi.min(0x7F) {
                ascii[(c >> 6) as usize] |= 1 << (c & 63);
            }
        }
        let non_ascii = set.ranges().last().is_some_and(|&(_, hi)| hi >= 0x80);
        Self { ascii, set, non_ascii }
    }

    #[inline]
    pub fn contains(&self, c: u32) -> bool {
        if c < 0x80 {
            self.ascii[(c >> 6) as usize] & (1 << (c & 63)) != 0
        } else {
            self.non_ascii && self.set.contains(c)
        }
    }
}

/// Over-approximates the first code unit of any match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstUnits {
    pub ascii: [u64; 2],
    pub non_ascii: bool,
}

impl FirstUnits {
    #[inline]
    pub fn may_start(&self, unit: u16) -> bool {
        if unit < 0x80 {
            self.ascii[(unit >> 6) as usize] & (1 << (unit & 63)) != 0
        } else {
            self.non_ascii
        }
    }

    fn none() -> Self {
        Self { ascii: [0, 0], non_ascii: false }
    }

    fn union(&mut self, other: &FirstUnits) {
        self.ascii[0] |= other.ascii[0];
        self.ascii[1] |= other.ascii[1];
        self.non_ascii |= other.non_ascii;
    }

    fn from_set(set: &CharSet) -> Self {
        let runtime = RuntimeSet::new(set.clone());
        Self { ascii: runtime.ascii, non_ascii: runtime.non_ascii }
    }
}

pub fn compile(node: Node, group_count: u32) -> Program {
    clear_memo();
    let node = &recognize_atomic(node, false);
    let mut compiler = Compiler { insts: Vec::new(), alt_tables: Vec::new(), sets: Vec::new(), follows: Vec::new(), backref_groups: Vec::new(), loop_count: 0 };
    compiler.emit_node(node, false, None);
    compiler.insts.push(Inst::Match);
    let first_units = first_units(node);
    let program = Program {
        insts: compiler.insts,
        follows: compiler.follows,
        alt_tables: compiler.alt_tables,
        sets: compiler.sets,
        backref_groups: compiler.backref_groups,
        loop_count: compiler.loop_count,
        group_count,
        first_units,
        required_literal: required_literals(node).into_iter().max_by_key(Vec::len),
        anchored_start: anchored_start(node),
    };
    clear_memo();
    program
}

struct Compiler {
    insts: Vec<Inst>,
    alt_tables: Vec<AltTable>,
    follows: Vec<FirstUnits>,
    sets: Vec<RuntimeSet>,
    backref_groups: Vec<Vec<u32>>,
    loop_count: u32,
}

impl Compiler {
    fn pc(&self) -> u32 {
        self.insts.len() as u32
    }

    fn add_set(&mut self, set: &CharSet) -> u32 {
        self.sets.push(RuntimeSet::new(set.clone()));
        self.sets.len() as u32 - 1
    }

    fn emit_char(&mut self, set: &CharSet, back: bool) {
        let ranges = set.ranges();
        let is_unit = |c: u32| c < 0xD800 || (0xE000..0x10000).contains(&c);
        if let Some(c) = set.as_single() {
            if is_unit(c) {
                self.insts.push(Inst::Unit { unit: c as u16, back });
                return;
            }
        }
        if set.len() == 2 {
            let members: Vec<u32> = ranges.iter().flat_map(|&(lo, hi)| lo..=hi).collect();
            if members.iter().all(|&c| is_unit(c)) {
                self.insts.push(Inst::Unit2 { a: members[0] as u16, b: members[1] as u16, back });
                return;
            }
        }
        let set = self.add_set(set);
        self.insts.push(Inst::Set { set, back });
    }

    fn emit_node(&mut self, node: &Node, back: bool, follow: Option<&FirstUnits>) {
        match node {
            Node::Empty => {}
            Node::Set(set) => self.emit_char(set, back),
            Node::Seq(items) => {
                if back {
                    for item in items.iter().rev() {
                        self.emit_node(item, back, None);
                    }
                } else {
                    // What may follow each item: the continuation of the items after it.
                    let mut follows = vec![follow.cloned(); items.len()];
                    for i in (0..items.len().saturating_sub(1)).rev() {
                        follows[i] = continuation_step(&items[i + 1], follows[i + 1].clone());
                    }
                    for (item, item_follow) in items.iter().zip(&follows) {
                        self.emit_node(item, back, item_follow.as_ref());
                    }
                }
            }
            Node::Alt(alternatives) if !back => {
                // Alternatives in order; each is skipped when the code unit at the
                // position cannot start it.
                let table = self.alt_tables.len() as u32;
                self.alt_tables.push(AltTable::new(&[]));
                self.insts.push(Inst::AltTable { table });
                let mut entries = Vec::with_capacity(alternatives.len());
                let mut jumps = Vec::new();
                for alternative in alternatives {
                    let guard = continuation(std::slice::from_ref(alternative), follow.cloned()).filter(|g| *g != all_units());
                    entries.push(AltEntry { pc: self.pc(), guard });
                    self.emit_node(alternative, back, follow);
                    jumps.push(self.pc());
                    self.insts.push(Inst::Jump { to: 0 });
                }
                let end = self.pc();
                for jump in jumps {
                    self.insts[jump as usize] = Inst::Jump { to: end };
                }
                self.alt_tables[table as usize] = AltTable::new(&entries);
            }
            Node::Alt(alternatives) => {
                let mut jumps = Vec::new();
                for (i, alternative) in alternatives.iter().enumerate() {
                    if i + 1 < alternatives.len() {
                        let split = self.pc();
                        self.insts.push(Inst::Split { alt: 0 });
                        self.emit_node(alternative, back, follow);
                        jumps.push(self.pc());
                        self.insts.push(Inst::Jump { to: 0 });
                        let next = self.pc();
                        self.insts[split as usize] = Inst::Split { alt: next };
                    } else {
                        self.emit_node(alternative, back, follow);
                    }
                }
                let end = self.pc();
                for jump in jumps {
                    self.insts[jump as usize] = Inst::Jump { to: end };
                }
            }
            Node::Capture { index, body } => {
                let (open, close) = (index * 2, index * 2 + 1);
                // A right-to-left body reaches its end first.
                self.insts.push(Inst::Save { slot: if back { close } else { open } });
                self.emit_node(body, back, follow);
                self.insts.push(Inst::Save { slot: if back { open } else { close } });
            }
            Node::Look { behind, negate, body } => {
                let look = self.pc();
                self.insts.push(Inst::Look { body: look + 1, next: 0, negate: *negate });
                self.emit_node(body, *behind, None);
                self.insts.push(Inst::LookEnd);
                let next = self.pc();
                self.insts[look as usize] = Inst::Look { body: look + 1, next, negate: *negate };
            }
            Node::Repeat { min, max, greedy, body } => self.emit_repeat(*min, *max, *greedy, body, back, follow),
            Node::Atomic(body) => self.emit_atomic(body, back),
            Node::Backref { groups, ignore_case } => {
                self.backref_groups.push(groups.clone());
                let groups = self.backref_groups.len() as u32 - 1;
                self.insts.push(Inst::Backref { groups, ignore_case: *ignore_case, back });
            }
            Node::Assert(assertion) => self.insts.push(Inst::Assert(*assertion)),
        }
    }

    fn emit_atomic(&mut self, body: &Node, back: bool) {
        // A greedy one-character loop, possibly captured, never needs to give back.
        let (capture, inner) = match body {
            Node::Capture { index, body } => (Some(*index), body.as_ref()),
            other => (None, other),
        };
        if let Node::Repeat { min, max, greedy: true, body: repeated } = inner {
            if let Node::Set(set) = repeated.as_ref() {
                let set = self.add_set(set);
                if let Some(index) = capture {
                    self.insts.push(Inst::Save { slot: index * 2 });
                }
                self.insts.push(Inst::SetRepeat { set, min: *min, max: *max, greedy: true, back, possessive: true, follow: NO_FOLLOW });
                if let Some(index) = capture {
                    self.insts.push(Inst::Save { slot: index * 2 + 1 });
                }
                return;
            }
        }
        let atomic = self.pc();
        self.insts.push(Inst::Atomic { body: atomic + 1, next: 0 });
        self.emit_node(body, back, None);
        self.insts.push(Inst::LookEnd);
        let next = self.pc();
        self.insts[atomic as usize] = Inst::Atomic { body: atomic + 1, next };
    }

    fn add_follow(&mut self, follow: Option<&FirstUnits>, back: bool) -> u32 {
        match follow {
            Some(follow) if !back && *follow != all_units() => {
                self.follows.push(follow.clone());
                self.follows.len() as u32 - 1
            }
            _ => NO_FOLLOW,
        }
    }

    fn emit_repeat(&mut self, min: u32, max: u32, greedy: bool, body: &Node, back: bool, follow: Option<&FirstUnits>) {
        if max == 0 {
            return;
        }
        if let Node::Set(set) = body {
            let set = self.add_set(set);
            let follow = self.add_follow(follow, back);
            self.insts.push(Inst::SetRepeat { set, min, max, greedy, back, possessive: false, follow });
            return;
        }
        let (clear_from, clear_to) = capture_range(body);
        // After an iteration comes another iteration or what follows the loop.
        let body_follow = follow.map(|follow| {
            let mut next = first_set(body);
            next.union(follow);
            next
        });
        let body_follow = body_follow.as_ref();
        // `x?` with no captures and a body that cannot match empty needs no loop state.
        if min == 0 && max == 1 && clear_from == clear_to && !can_match_empty(body) {
            let split = self.pc();
            self.insts.push(Inst::Split { alt: 0 });
            if greedy {
                self.emit_node(body, back, follow);
                let end = self.pc();
                self.insts[split as usize] = Inst::Split { alt: end };
            } else {
                // Skipping the body comes first; the body is the alternative.
                let jump = self.pc();
                self.insts.push(Inst::Jump { to: 0 });
                let body_start = self.pc();
                self.emit_node(body, back, follow);
                let end = self.pc();
                self.insts[split as usize] = Inst::Split { alt: body_start };
                self.insts[jump as usize] = Inst::Jump { to: end };
            }
            return;
        }
        let reg = self.loop_count;
        self.loop_count += 1;
        self.insts.push(Inst::LoopInit { reg });
        let test = self.pc();
        self.insts.push(Inst::LoopTest { reg, min, max, greedy, exit: 0 });
        self.insts.push(Inst::LoopEnter { reg, clear_from, clear_to });
        self.emit_node(body, back, body_follow);
        self.insts.push(Inst::LoopEnd { reg, min, test });
        let exit = self.pc();
        self.insts[test as usize] = Inst::LoopTest { reg, min, max, greedy, exit };
    }
}

/// The capture groups inside `node`, as a half-open range of group numbers.
fn capture_range(node: &Node) -> (u32, u32) {
    let mut lo = u32::MAX;
    let mut hi = 0;
    visit_captures(node, &mut |index| {
        lo = lo.min(index);
        hi = hi.max(index + 1);
    });
    if lo == u32::MAX { (0, 0) } else { (lo, hi) }
}

fn visit_captures(node: &Node, f: &mut impl FnMut(u32)) {
    match node {
        Node::Capture { index, body } => {
            f(*index);
            visit_captures(body, f);
        }
        Node::Seq(items) | Node::Alt(items) => items.iter().for_each(|item| visit_captures(item, f)),
        Node::Look { body, .. } | Node::Repeat { body, .. } | Node::Atomic(body) => visit_captures(body, f),
        _ => {}
    }
}

/// Rewrites oniguruma-to-es's atomic group emulation `(?=(X))\n` (outside
/// lookbehinds) to `Atomic(Capture n (X))`: the same match, without the
/// backreference comparison.
fn recognize_atomic(node: Node, back: bool) -> Node {
    match node {
        Node::Seq(items) => {
            let mut out: Vec<Node> = Vec::with_capacity(items.len());
            for item in items {
                let item = recognize_atomic(item, back);
                if !back {
                    if let Node::Backref { groups, .. } = &item {
                        if let [group] = groups.as_slice() {
                            if let Some(Node::Look { behind: false, negate: false, body }) = out.last() {
                                if matches!(body.as_ref(), Node::Capture { index, .. } if index == group) {
                                    let Some(Node::Look { body, .. }) = out.pop() else { unreachable!() };
                                    out.push(Node::Atomic(body));
                                    continue;
                                }
                            }
                        }
                    }
                }
                out.push(item);
            }
            Node::Seq(out)
        }
        Node::Alt(items) => Node::Alt(items.into_iter().map(|item| recognize_atomic(item, back)).collect()),
        Node::Capture { index, body } => Node::Capture { index, body: Box::new(recognize_atomic(*body, back)) },
        Node::Look { behind, negate, body } => Node::Look { behind, negate, body: Box::new(recognize_atomic(*body, behind)) },
        Node::Repeat { min, max, greedy, body } => Node::Repeat { min, max, greedy, body: Box::new(recognize_atomic(*body, back)) },
        other => other,
    }
}

thread_local! {
    /// Per-compile memo of `can_match_empty` and `first_set`, keyed by node address.
    static NULLABLE: std::cell::RefCell<crate::fxhash::FxHashMap<usize, bool>> = Default::default();
    static FIRST: std::cell::RefCell<crate::fxhash::FxHashMap<usize, FirstUnits>> = Default::default();
}

fn clear_memo() {
    NULLABLE.with_borrow_mut(|m| m.clear());
    FIRST.with_borrow_mut(|m| m.clear());
}

pub fn can_match_empty(node: &Node) -> bool {
    // Leaves are cheap; memoize only composite nodes.
    if !matches!(node, Node::Seq(_) | Node::Alt(_) | Node::Capture { .. } | Node::Atomic(_) | Node::Repeat { .. }) {
        return can_match_empty_uncached(node);
    }
    let key = node as *const Node as usize;
    if let Some(value) = NULLABLE.with_borrow(|m| m.get(&key).copied()) {
        return value;
    }
    let value = can_match_empty_uncached(node);
    NULLABLE.with_borrow_mut(|m| m.insert(key, value));
    value
}

fn can_match_empty_uncached(node: &Node) -> bool {
    match node {
        Node::Empty | Node::Look { .. } | Node::Assert(_) | Node::Backref { .. } => true,
        Node::Set(_) => false,
        Node::Seq(items) => items.iter().all(can_match_empty),
        Node::Alt(items) => items.iter().any(can_match_empty),
        Node::Capture { body, .. } | Node::Atomic(body) => can_match_empty(body),
        Node::Repeat { min, body, .. } => *min == 0 || can_match_empty(body),
    }
}

fn all_units() -> FirstUnits {
    FirstUnits { ascii: [u64::MAX, u64::MAX], non_ascii: true }
}

fn intersect(a: &FirstUnits, b: &FirstUnits) -> FirstUnits {
    FirstUnits { ascii: [a.ascii[0] & b.ascii[0], a.ascii[1] & b.ascii[1]], non_ascii: a.non_ascii && b.non_ascii }
}

/// The code units a non-empty match of `node` can start with (forward direction).
fn first_set(node: &Node) -> FirstUnits {
    if !matches!(node, Node::Seq(_) | Node::Alt(_) | Node::Capture { .. } | Node::Atomic(_) | Node::Repeat { .. }) {
        return first_set_uncached(node);
    }
    let key = node as *const Node as usize;
    if let Some(value) = FIRST.with_borrow(|m| m.get(&key).cloned()) {
        return value;
    }
    let value = first_set_uncached(node);
    FIRST.with_borrow_mut(|m| m.insert(key, value.clone()));
    value
}

fn first_set_uncached(node: &Node) -> FirstUnits {
    match node {
        Node::Set(set) => FirstUnits::from_set(set),
        Node::Empty | Node::Assert(_) | Node::Look { .. } => FirstUnits::none(),
        Node::Backref { .. } => all_units(),
        Node::Seq(items) => {
            let mut first = FirstUnits::none();
            for item in items {
                first.union(&first_set(item));
                if !can_match_empty(item) {
                    break;
                }
            }
            first
        }
        Node::Alt(items) => {
            let mut first = FirstUnits::none();
            for item in items {
                first.union(&first_set(item));
            }
            first
        }
        Node::Capture { body, .. } | Node::Atomic(body) | Node::Repeat { body, .. } => first_set(body),
    }
}

/// A set the code unit at a match start must be in, or `None` when a match can
/// start anywhere (including empty at the end of the input).
fn first_units(node: &Node) -> Option<FirstUnits> {
    match node {
        Node::Seq(items) => sequence_first_units(items),
        other => sequence_first_units(std::slice::from_ref(other)),
    }
}

fn sequence_first_units(items: &[Node]) -> Option<FirstUnits> {
    continuation(items, None)
}

/// A set the code unit at the start of a match of `items` followed by something
/// constrained by `outer` must be in (`None`: no constraint).
fn continuation(items: &[Node], outer: Option<FirstUnits>) -> Option<FirstUnits> {
    items.iter().rev().fold(outer, |rest, item| continuation_step(item, rest))
}

/// `continuation` of `item` followed by something constrained by `rest`.
fn continuation_step(item: &Node, rest: Option<FirstUnits>) -> Option<FirstUnits> {
    match item {
        Node::Look { behind: false, negate: false, body } => {
            if can_match_empty(body) {
                return rest;
            }
            let own = first_set(body);
            Some(match rest {
                Some(rest) => intersect(&own, &rest),
                None => own,
            })
        }
        Node::Look { .. } | Node::Assert(_) => rest,
        _ if !can_match_empty(item) => Some(first_set(item)),
        _ => {
            let mut first = first_set(item);
            first.union(&rest?);
            Some(first)
        }
    }
}

/// A character node as a literal element: one BMP unit, or two (case variants).
fn literal_element(node: &Node) -> Option<(u16, u16)> {
    let Node::Set(set) = node else { return None };
    let is_unit = |c: u32| c < 0xD800 || (0xE000..0x10000).contains(&c);
    if let Some(c) = set.as_single() {
        return is_unit(c).then_some((c as u16, c as u16));
    }
    if set.len() == 2 {
        let members: Vec<u32> = set.ranges().iter().flat_map(|&(lo, hi)| lo..=hi).collect();
        if members.iter().all(|&c| is_unit(c)) {
            return Some((members[0] as u16, members[1] as u16));
        }
    }
    None
}

/// Literal runs that every match must contain at or after the match start
/// (lookahead text included, lookbehinds and alternations ignored).
fn required_literals(node: &Node) -> Vec<Vec<(u16, u16)>> {
    match node {
        Node::Seq(items) => {
            let mut literals = Vec::new();
            let mut run = Vec::new();
            for item in items {
                if let Some(element) = literal_element(item) {
                    run.push(element);
                    continue;
                }
                if matches!(item, Node::Assert(_)) {
                    continue;
                }
                if !run.is_empty() {
                    literals.push(std::mem::take(&mut run));
                }
                literals.extend(required_literals(item));
            }
            if !run.is_empty() {
                literals.push(run);
            }
            literals
        }
        Node::Set(_) => literal_element(node).map(|e| vec![vec![e]]).unwrap_or_default(),
        Node::Capture { body, .. } | Node::Atomic(body) => required_literals(body),
        Node::Repeat { min, body, .. } if *min > 0 => required_literals(body),
        Node::Look { behind: false, negate: false, body } => required_literals(body),
        Node::Alt(items) if items.len() == 1 => required_literals(&items[0]),
        _ => Vec::new(),
    }
}

/// Whether every match must begin at input start (`^` without `m` first).
fn anchored_start(node: &Node) -> bool {
    match node {
        Node::Assert(Assertion::InputStart) => true,
        Node::Seq(items) => items.first().is_some_and(anchored_start),
        Node::Alt(items) => items.iter().all(anchored_start),
        Node::Capture { body, .. } => anchored_start(body),
        _ => false,
    }
}
