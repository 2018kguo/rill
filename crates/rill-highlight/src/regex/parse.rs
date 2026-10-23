//! Parser for ECMAScript regular expressions in `u` and `v` mode, the forms
//! oniguruma-to-es emits. Case-insensitivity is resolved here: under `i`, every
//! character set is closed under simple case folding, so matching never folds.

use super::charset::{self, CharSet};

#[derive(Debug, Clone)]
pub enum Node {
    Empty,
    /// One character from the set.
    Set(CharSet),
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Capture { index: u32, body: Box<Node> },
    Look { behind: bool, negate: bool, body: Box<Node> },
    /// Matches `body` once without backtracking into it: oniguruma-to-es's
    /// `(?=(body))\1` emulation of atomic groups, recognized after parsing.
    Atomic(Box<Node>),
    Repeat { min: u32, max: u32, greedy: bool, body: Box<Node> },
    /// Matches what the first participating group of `groups` captured.
    Backref { groups: Vec<u32>, ignore_case: bool },
    Assert(Assertion),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assertion {
    InputStart,
    InputEnd,
    LineStart,
    LineEnd,
    /// `\b`; `extended` adds U+017F and U+212A to the word characters (`i` with `u`/`v`).
    WordBoundary { extended: bool },
    NotWordBoundary { extended: bool },
}

pub const UNBOUNDED: u32 = u32::MAX;

#[derive(Debug)]
pub struct Parsed {
    pub node: Node,
    /// Number of capture groups, not counting group 0.
    pub group_count: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Modes {
    ignore_case: bool,
    multiline: bool,
    dot_all: bool,
}

pub fn parse(pattern: &str, flags: &str) -> Result<Parsed, String> {
    let mut modes = Modes::default();
    let mut unicode_sets = false;
    let mut unicode = false;
    for flag in flags.chars() {
        match flag {
            'i' => modes.ignore_case = true,
            'm' => modes.multiline = true,
            's' => modes.dot_all = true,
            'v' => unicode_sets = true,
            'u' => unicode = true,
            'd' | 'g' | 'y' => {}
            other => return Err(format!("unsupported flag {other}")),
        }
    }
    if !unicode && !unicode_sets {
        return Err("only u and v mode patterns are supported".into());
    }
    let chars: Vec<char> = pattern.chars().collect();
    let names = scan_groups(&chars);
    let mut parser = Parser {
        chars,
        pos: 0,
        modes,
        unicode_sets,
        next_group: 1,
        group_names: names,
    };
    let node = parser.parse_disjunction()?;
    if parser.pos != parser.chars.len() {
        return Err(format!("unexpected ')' at {}", parser.pos));
    }
    Ok(Parsed { node, group_count: parser.next_group - 1 })
}

/// Names of every capture group in order (`None` for unnamed groups).
fn scan_groups(chars: &[char]) -> Vec<Option<String>> {
    let mut names = Vec::new();
    let mut class_depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '[' => class_depth += 1,
            ']' if class_depth > 0 => class_depth -= 1,
            '(' if class_depth == 0 => {
                if chars.get(i + 1) != Some(&'?') {
                    names.push(None);
                } else if chars.get(i + 2) == Some(&'<')
                    && !matches!(chars.get(i + 3), Some('=') | Some('!'))
                {
                    let start = i + 3;
                    let mut end = start;
                    while end < chars.len() && chars[end] != '>' {
                        end += 1;
                    }
                    names.push(Some(chars[start..end].iter().collect()));
                }
            }
            _ => {}
        }
        i += 1;
    }
    names
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    modes: Modes,
    unicode_sets: bool,
    next_group: u32,
    group_names: Vec<Option<String>>,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.eat(c) { Ok(()) } else { Err(format!("expected '{c}' at {}", self.pos)) }
    }

    fn parse_disjunction(&mut self) -> Result<Node, String> {
        let mut alternatives = vec![self.parse_alternative()?];
        while self.eat('|') {
            alternatives.push(self.parse_alternative()?);
        }
        Ok(if alternatives.len() == 1 { alternatives.pop().unwrap() } else { Node::Alt(alternatives) })
    }

    fn parse_alternative(&mut self) -> Result<Node, String> {
        let mut terms = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            terms.push(self.parse_term()?);
        }
        Ok(match terms.len() {
            0 => Node::Empty,
            1 => terms.pop().unwrap(),
            _ => Node::Seq(terms),
        })
    }

    fn parse_term(&mut self) -> Result<Node, String> {
        let c = self.peek().unwrap();
        match c {
            '^' => {
                self.pos += 1;
                return Ok(Node::Assert(if self.modes.multiline {
                    Assertion::LineStart
                } else {
                    Assertion::InputStart
                }));
            }
            '$' => {
                self.pos += 1;
                return Ok(Node::Assert(if self.modes.multiline {
                    Assertion::LineEnd
                } else {
                    Assertion::InputEnd
                }));
            }
            '\\' if matches!(self.peek_at(1), Some('b') | Some('B')) => {
                let negate = self.peek_at(1) == Some('B');
                self.pos += 2;
                let extended = self.modes.ignore_case;
                return Ok(Node::Assert(if negate {
                    Assertion::NotWordBoundary { extended }
                } else {
                    Assertion::WordBoundary { extended }
                }));
            }
            '(' if self.peek_at(1) == Some('?') => {
                let look = match (self.peek_at(2), self.peek_at(3)) {
                    (Some('='), _) => Some((false, false, 3)),
                    (Some('!'), _) => Some((false, true, 3)),
                    (Some('<'), Some('=')) => Some((true, false, 4)),
                    (Some('<'), Some('!')) => Some((true, true, 4)),
                    _ => None,
                };
                if let Some((behind, negate, skip)) = look {
                    self.pos += skip;
                    let body = self.parse_disjunction()?;
                    self.expect(')')?;
                    let node = Node::Look { behind, negate, body: Box::new(body) };
                    // Lookarounds take no quantifier in u/v mode.
                    return Ok(node);
                }
            }
            _ => {}
        }
        let atom = self.parse_atom()?;
        self.parse_quantifier(atom)
    }

    fn parse_quantifier(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, UNBOUNDED)
            }
            Some('+') => {
                self.pos += 1;
                (1, UNBOUNDED)
            }
            Some('?') => {
                self.pos += 1;
                (0, 1)
            }
            Some('{') => match self.try_parse_braces() {
                Some(bounds) => bounds,
                None => return Err(format!("lone '{{' at {}", self.pos)),
            },
            _ => return Ok(atom),
        };
        let greedy = !self.eat('?');
        if max < min {
            return Err("numbers out of order in quantifier".into());
        }
        Ok(Node::Repeat { min, max, greedy, body: Box::new(atom) })
    }

    fn try_parse_braces(&mut self) -> Option<(u32, u32)> {
        let start = self.pos;
        self.pos += 1;
        let min = self.parse_decimal();
        let Some(min) = min else {
            self.pos = start;
            return None;
        };
        let max = if self.eat(',') { self.parse_decimal().unwrap_or(UNBOUNDED) } else { min };
        if !self.eat('}') {
            self.pos = start;
            return None;
        }
        Some((min, max))
    }

    fn parse_decimal(&mut self) -> Option<u32> {
        let start = self.pos;
        let mut value: u64 = 0;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            value = (value * 10 + u64::from(d)).min(u64::from(UNBOUNDED - 1));
            self.pos += 1;
        }
        if self.pos == start { None } else { Some(value as u32) }
    }

    fn parse_atom(&mut self) -> Result<Node, String> {
        let c = self.peek().unwrap();
        match c {
            '.' => {
                self.pos += 1;
                Ok(Node::Set(if self.modes.dot_all {
                    CharSet::all()
                } else {
                    line_terminators().complement()
                }))
            }
            '(' => self.parse_group(),
            '[' => {
                self.pos += 1;
                let set = if self.unicode_sets { self.parse_class_v()? } else { self.parse_class_u()? };
                Ok(Node::Set(set))
            }
            '\\' => {
                self.pos += 1;
                self.parse_atom_escape()
            }
            ')' | '*' | '+' | '?' | '{' | '}' | ']' | '|' => {
                Err(format!("unexpected '{c}' at {}", self.pos))
            }
            _ => {
                self.pos += 1;
                Ok(Node::Set(self.char_set(c as u32)))
            }
        }
    }

    fn parse_group(&mut self) -> Result<Node, String> {
        self.pos += 1; // '('
        if !self.eat('?') {
            let index = self.next_group;
            self.next_group += 1;
            let body = self.parse_disjunction()?;
            self.expect(')')?;
            return Ok(Node::Capture { index, body: Box::new(body) });
        }
        if self.eat(':') {
            let body = self.parse_disjunction()?;
            self.expect(')')?;
            return Ok(body);
        }
        if self.eat('<') {
            while let Some(c) = self.peek() {
                self.pos += 1;
                if c == '>' {
                    break;
                }
            }
            let index = self.next_group;
            self.next_group += 1;
            let body = self.parse_disjunction()?;
            self.expect(')')?;
            return Ok(Node::Capture { index, body: Box::new(body) });
        }
        // Modifiers: (?ims-ims:...)
        let saved = self.modes;
        let mut enable = true;
        loop {
            match self.peek() {
                Some('i') => self.modes.ignore_case = enable,
                Some('m') => self.modes.multiline = enable,
                Some('s') => self.modes.dot_all = enable,
                Some('-') if enable => enable = false,
                Some(':') => break,
                other => return Err(format!("unsupported group syntax {other:?} at {}", self.pos)),
            }
            self.pos += 1;
        }
        self.pos += 1; // ':'
        let body = self.parse_disjunction()?;
        self.expect(')')?;
        self.modes = saved;
        Ok(body)
    }

    fn parse_atom_escape(&mut self) -> Result<Node, String> {
        let Some(c) = self.peek() else { return Err("trailing backslash".into()) };
        match c {
            '1'..='9' => {
                let number = self.parse_decimal().unwrap();
                if number >= self.next_group_total() {
                    return Err(format!("invalid backreference \\{number}"));
                }
                Ok(Node::Backref { groups: vec![number], ignore_case: self.modes.ignore_case })
            }
            'k' => {
                self.pos += 1;
                self.expect('<')?;
                let start = self.pos;
                while self.peek().is_some_and(|c| c != '>') {
                    self.pos += 1;
                }
                let name: String = self.chars[start..self.pos].iter().collect();
                self.expect('>')?;
                let groups: Vec<u32> = self
                    .group_names
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.as_deref() == Some(name.as_str()))
                    .map(|(i, _)| i as u32 + 1)
                    .collect();
                if groups.is_empty() {
                    return Err(format!("unknown group name {name}"));
                }
                Ok(Node::Backref { groups, ignore_case: self.modes.ignore_case })
            }
            _ => {
                if let Some(set) = self.try_class_escape()? {
                    return Ok(Node::Set(set));
                }
                let c = self.parse_character_escape(false)?;
                Ok(Node::Set(self.char_set(c)))
            }
        }
    }

    fn next_group_total(&self) -> u32 {
        self.group_names.len() as u32 + 1
    }

    /// `\d \D \s \S \w \W \p{..} \P{..}` at the current position (after `\`).
    fn try_class_escape(&mut self) -> Result<Option<CharSet>, String> {
        let Some(c) = self.peek() else { return Ok(None) };
        let (base, negate) = match c {
            'd' | 'D' => (CharSet::from_ranges(&[(0x30, 0x39)]), c == 'D'),
            's' | 'S' => (whitespace(), c == 'S'),
            'w' | 'W' => (self.word_characters(), c == 'W'),
            'p' | 'P' => {
                self.pos += 1;
                self.expect('{')?;
                let start = self.pos;
                while self.peek().is_some_and(|c| c != '}') {
                    self.pos += 1;
                }
                let name: String = self.chars[start..self.pos].iter().collect();
                self.expect('}')?;
                let set = charset::property(&name).ok_or_else(|| format!("unsupported property {name}"))?;
                return Ok(Some(self.escape_set(set, c == 'P')));
            }
            _ => return Ok(None),
        };
        self.pos += 1;
        Ok(Some(self.escape_set(base, negate)))
    }

    /// The set of a class escape, negated as the mode requires: in `v` mode the
    /// complement is taken after case folding, in `u` mode before.
    fn escape_set(&self, set: CharSet, negate: bool) -> CharSet {
        if !negate {
            return self.fold_set(set);
        }
        if self.unicode_sets { self.fold_set(set).complement() } else { self.fold_set(set.complement()) }
    }

    fn word_characters(&self) -> CharSet {
        let mut set = CharSet::from_ranges(&[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)]);
        if self.modes.ignore_case {
            set.add(0x017F);
            set.add(0x212A);
        }
        set
    }

    fn fold_set(&self, set: CharSet) -> CharSet {
        if self.modes.ignore_case { set.case_closure() } else { set }
    }

    fn char_set(&self, c: u32) -> CharSet {
        self.fold_set(CharSet::single(c))
    }

    /// A CharacterEscape after `\` (and, inside classes, `\b` and `\-`).
    fn parse_character_escape(&mut self, in_class: bool) -> Result<u32, String> {
        let c = self.peek().ok_or("trailing backslash")?;
        self.pos += 1;
        Ok(match c {
            'f' => 0x0C,
            'n' => 0x0A,
            'r' => 0x0D,
            't' => 0x09,
            'v' => 0x0B,
            'b' if in_class => 0x08,
            'c' => {
                let letter = self.peek().ok_or("bad \\c")?;
                if !letter.is_ascii_alphabetic() {
                    return Err("bad \\c escape".into());
                }
                self.pos += 1;
                letter as u32 % 32
            }
            '0' if !self.peek().is_some_and(|d| d.is_ascii_digit()) => 0,
            'x' => {
                let value = self.parse_hex_digits(2).ok_or("bad \\x escape")?;
                value
            }
            'u' => self.parse_unicode_escape()?,
            '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '/' => c as u32,
            '-' if in_class => c as u32,
            '&' | '!' | '#' | '%' | ',' | ':' | ';' | '<' | '=' | '>' | '@' | '`' | '~'
                if in_class && self.unicode_sets =>
            {
                c as u32
            }
            _ => return Err(format!("invalid escape \\{c}")),
        })
    }

    fn parse_hex_digits(&mut self, count: usize) -> Option<u32> {
        let mut value = 0u32;
        for i in 0..count {
            let d = self.peek_at(i)?.to_digit(16)?;
            value = value * 16 + d;
        }
        self.pos += count;
        Some(value)
    }

    fn parse_unicode_escape(&mut self) -> Result<u32, String> {
        if self.eat('{') {
            let start = self.pos;
            let mut value: u32 = 0;
            while let Some(d) = self.peek().and_then(|c| c.to_digit(16)) {
                value = value.saturating_mul(16).saturating_add(d);
                self.pos += 1;
            }
            if self.pos == start || !self.eat('}') || value > charset::MAX_CODE_POINT {
                return Err("bad \\u{...} escape".into());
            }
            return Ok(value);
        }
        let value = self.parse_hex_digits(4).ok_or("bad \\u escape")?;
        // A surrogate pair written as two escapes is one code point.
        if (0xD800..=0xDBFF).contains(&value) && self.peek() == Some('\\') && self.peek_at(1) == Some('u') {
            let saved = self.pos;
            self.pos += 2;
            match self.parse_hex_digits(4) {
                Some(low) if (0xDC00..=0xDFFF).contains(&low) => {
                    return Ok(0x10000 + ((value - 0xD800) << 10) + (low - 0xDC00));
                }
                _ => self.pos = saved,
            }
        }
        Ok(value)
    }

    /// A `u`-mode class after `[`.
    fn parse_class_u(&mut self) -> Result<CharSet, String> {
        let negate = self.eat('^');
        let mut set = CharSet::new();
        loop {
            let Some(c) = self.peek() else { return Err("unterminated class".into()) };
            if c == ']' {
                self.pos += 1;
                break;
            }
            let start = self.parse_class_atom_u()?;
            if self.peek() == Some('-') && self.peek_at(1).is_some_and(|c| c != ']') {
                if let ClassAtom::Char(lo) = start {
                    self.pos += 1;
                    match self.parse_class_atom_u()? {
                        ClassAtom::Char(hi) if hi >= lo => {
                            set.add_range(lo, hi);
                            continue;
                        }
                        _ => return Err("bad class range".into()),
                    }
                }
            }
            match start {
                ClassAtom::Char(c) => set.add(c),
                ClassAtom::Set(s) => set.union(&s),
            }
        }
        // In u mode the class escapes are already resolved; the class is folded and
        // then inverted by the matcher.
        let set = if negate { self.fold_set(set).complement() } else { self.fold_set(set) };
        Ok(set)
    }

    fn parse_class_atom_u(&mut self) -> Result<ClassAtom, String> {
        let c = self.peek().unwrap();
        self.pos += 1;
        if c != '\\' {
            return Ok(ClassAtom::Char(c as u32));
        }
        // Class escapes keep their own folding; fold_set of a union is the same.
        let saved_case = self.modes.ignore_case;
        self.modes.ignore_case = false;
        let set = self.try_class_escape();
        self.modes.ignore_case = saved_case;
        if let Some(set) = set? {
            return Ok(ClassAtom::Set(set));
        }
        Ok(ClassAtom::Char(self.parse_character_escape(true)?))
    }

    /// A `v`-mode class after `[`: ClassUnion, ClassIntersection or ClassSubtraction.
    fn parse_class_v(&mut self) -> Result<CharSet, String> {
        let negate = self.eat('^');
        let mut set;
        if self.eat(']') {
            set = CharSet::new();
        } else {
            let first = self.parse_class_operand_v(true)?;
            if self.at_double('&') {
                set = first;
                while self.at_double('&') {
                    self.pos += 2;
                    let next = self.parse_class_operand_v(false)?;
                    set = set.intersect(&next);
                }
                self.expect(']')?;
            } else if self.at_double('-') {
                set = first;
                while self.at_double('-') {
                    self.pos += 2;
                    let next = self.parse_class_operand_v(false)?;
                    set = set.subtract(&next);
                }
                self.expect(']')?;
            } else {
                set = first;
                loop {
                    if self.eat(']') {
                        break;
                    }
                    if self.peek().is_none() {
                        return Err("unterminated class".into());
                    }
                    let next = self.parse_class_operand_v(true)?;
                    set.union(&next);
                }
            }
        }
        Ok(if negate { set.complement() } else { set })
    }

    fn at_double(&self, c: char) -> bool {
        self.peek() == Some(c) && self.peek_at(1) == Some(c)
    }

    /// One union item (a range when `allow_range`), a nested class or a class escape;
    /// the result is already case-folded when `i` applies.
    fn parse_class_operand_v(&mut self, allow_range: bool) -> Result<CharSet, String> {
        let c = self.peek().ok_or("unterminated class")?;
        if c == '[' {
            self.pos += 1;
            return self.parse_class_v();
        }
        let lo = if c == '\\' {
            self.pos += 1;
            if self.peek() == Some('q') {
                return Err("\\q{...} is not supported".into());
            }
            if let Some(set) = self.try_class_escape()? {
                return Ok(set);
            }
            self.parse_character_escape(true)?
        } else {
            self.pos += 1;
            c as u32
        };
        if allow_range && self.peek() == Some('-') && self.peek_at(1) != Some('-') {
            self.pos += 1;
            let hi = match self.peek() {
                Some('\\') => {
                    self.pos += 1;
                    self.parse_character_escape(true)?
                }
                Some(c) => {
                    self.pos += 1;
                    c as u32
                }
                None => return Err("unterminated range".into()),
            };
            if hi < lo {
                return Err("range out of order".into());
            }
            return Ok(self.fold_set(CharSet::from_ranges(&[(lo, hi)])));
        }
        Ok(self.char_set(lo))
    }
}

enum ClassAtom {
    Char(u32),
    Set(CharSet),
}

fn line_terminators() -> CharSet {
    CharSet::from_ranges(&[(0x0A, 0x0A), (0x0D, 0x0D), (0x2028, 0x2029)])
}

/// `\s`: WhiteSpace and LineTerminator of ECMA-262.
fn whitespace() -> CharSet {
    CharSet::from_ranges(&[
        (0x09, 0x0D),
        (0x20, 0x20),
        (0xA0, 0xA0),
        (0x1680, 0x1680),
        (0x2000, 0x200A),
        (0x2028, 0x2029),
        (0x202F, 0x202F),
        (0x205F, 0x205F),
        (0x3000, 0x3000),
        (0xFEFF, 0xFEFF),
    ])
}
