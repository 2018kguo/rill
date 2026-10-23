//! Scope selectors (vscode-textmate `createMatchers` with `nameMatcher`), used for
//! injections.

use std::rc::Rc;

#[derive(Debug, Clone)]
pub enum Matcher {
    /// Identifiers that must appear in order among the scopes.
    Names(Vec<Rc<str>>),
    /// `-x`; `None` when nothing followed the `-` (never matches).
    Not(Option<Box<Matcher>>),
    All(Vec<Matcher>),
    Any(Vec<Matcher>),
}

#[derive(Debug, Clone)]
pub struct PrioritizedMatcher {
    pub matcher: Matcher,
    /// 1 for `R:`, -1 for `L:`, else 0.
    pub priority: i32,
}

impl Matcher {
    pub fn matches(&self, scopes: &[&str]) -> bool {
        match self {
            Matcher::Names(identifiers) => names_match(identifiers, scopes),
            Matcher::Not(operand) => operand.as_ref().is_some_and(|m| !m.matches(scopes)),
            Matcher::All(matchers) => matchers.iter().all(|m| m.matches(scopes)),
            Matcher::Any(matchers) => matchers.iter().any(|m| m.matches(scopes)),
        }
    }
}

fn names_match(identifiers: &[Rc<str>], scopes: &[&str]) -> bool {
    if scopes.len() < identifiers.len() {
        return false;
    }
    let mut last = 0;
    identifiers.iter().all(|identifier| {
        for (i, scope) in scopes.iter().enumerate().skip(last) {
            if scopes_are_matching(scope, identifier) {
                last = i + 1;
                return true;
            }
        }
        false
    })
}

pub fn scopes_are_matching(this_scope: &str, scope: &str) -> bool {
    if this_scope.is_empty() {
        return false;
    }
    this_scope == scope
        || (this_scope.len() > scope.len() && this_scope.starts_with(scope) && this_scope.as_bytes()[scope.len()] == b'.')
}

/// Tokens of /([LR]:|[\w\.:][\w\.:\-]*|[\,\|\-\(\)])/g.
fn tokenize(input: &str) -> Vec<&str> {
    let bytes = input.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':';
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if (b == b'L' || b == b'R') && bytes.get(i + 1) == Some(&b':') {
            tokens.push(&input[i..i + 2]);
            i += 2;
        } else if is_word(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && (is_word(bytes[i]) || bytes[i] == b'-') {
                i += 1;
            }
            tokens.push(&input[start..i]);
        } else if matches!(b, b',' | b'|' | b'-' | b'(' | b')') {
            tokens.push(&input[i..i + 1]);
            i += 1;
        } else {
            i += 1;
        }
    }
    tokens
}

fn is_identifier(token: Option<&str>) -> bool {
    token.is_some_and(|t| t.bytes().any(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':'))
}

struct Parser<'a> {
    tokens: Vec<&'a str>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn token(&self) -> Option<&'a str> {
        self.tokens.get(self.pos).copied()
    }

    fn next(&mut self) {
        self.pos += 1;
    }

    fn parse_operand(&mut self) -> Option<Matcher> {
        match self.token() {
            Some("-") => {
                self.next();
                let operand = self.parse_operand();
                Some(Matcher::Not(operand.map(Box::new)))
            }
            Some("(") => {
                self.next();
                let inner = self.parse_inner_expression();
                if self.token() == Some(")") {
                    self.next();
                }
                Some(inner)
            }
            token if is_identifier(token) => {
                let mut identifiers = Vec::new();
                while is_identifier(self.token()) {
                    identifiers.push(Rc::from(self.token().unwrap()));
                    self.next();
                }
                Some(Matcher::Names(identifiers))
            }
            _ => None,
        }
    }

    fn parse_conjunction(&mut self) -> Matcher {
        let mut matchers = Vec::new();
        while let Some(matcher) = self.parse_operand() {
            matchers.push(matcher);
        }
        Matcher::All(matchers)
    }

    fn parse_inner_expression(&mut self) -> Matcher {
        let mut matchers = vec![self.parse_conjunction()];
        loop {
            if matches!(self.token(), Some("|") | Some(",")) {
                while matches!(self.token(), Some("|") | Some(",")) {
                    self.next();
                }
            } else {
                break;
            }
            matchers.push(self.parse_conjunction());
        }
        Matcher::Any(matchers)
    }
}

pub fn create_matchers(selector: &str) -> Vec<PrioritizedMatcher> {
    let mut parser = Parser { tokens: tokenize(selector), pos: 0 };
    let mut results = Vec::new();
    while parser.token().is_some() {
        let mut priority = 0;
        if let Some(token) = parser.token()
            && token.len() == 2 && token.as_bytes()[1] == b':' {
                priority = match token.as_bytes()[0] {
                    b'R' => 1,
                    b'L' => -1,
                    _ => 0,
                };
                parser.next();
            }
        let matcher = parser.parse_conjunction();
        results.push(PrioritizedMatcher { matcher, priority });
        if parser.token() != Some(",") {
            break;
        }
        parser.next();
    }
    results
}
