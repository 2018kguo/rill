//! Line tokenization (vscode-textmate `tokenizeString.ts`, `StateStackImpl`,
//! `AttributedScopeStack`, `LineTokens`), for several themes at once: the rules
//! matched do not depend on the theme, so one pass computes every theme's token
//! metadata.

use super::grammar::{EndRule, Grammar, OnigString, RuleId, RuleKind, ScanMatch, END_RULE_ID, UNMATCHED, WHILE_RULE_ID};
use super::theme::{FONT_STYLE_NOT_SET, ScopeStack, StyleAttributes, Theme};
use std::cell::Cell;
use std::rc::Rc;

/// Themes tokenized together (Streamdown's light and dark).
pub const THEMES: usize = 2;

pub type Metadata = [u32; THEMES];

/// `AttributedScopeStack`.
#[derive(Debug)]
pub struct AttrScope {
    pub parent: Option<Rc<AttrScope>>,
    pub path: Rc<ScopeStack>,
    pub attrs: Metadata,
}

impl AttrScope {
    pub fn scope_names(&self) -> Vec<Rc<str>> {
        let mut names = Vec::new();
        let mut node = Some(&self.path);
        while let Some(path) = node {
            names.push(path.name.clone());
            node = path.parent.as_ref();
        }
        names.reverse();
        names
    }
}

/// `StateStackImpl`. `None` stands for `INITIAL`.
#[derive(Debug)]
pub struct StateNode {
    pub parent: Option<Rc<StateNode>>,
    pub rule_id: RuleId,
    enter_pos: Cell<i64>,
    anchor_pos: Cell<i64>,
    pub begin_rule_captured_eol: bool,
    pub end_rule: Option<Rc<EndRule>>,
    pub name_scopes: Rc<AttrScope>,
    pub content_name_scopes: Rc<AttrScope>,
}

pub type State = Rc<StateNode>;

#[allow(clippy::too_many_arguments)]
fn push_state(
    parent: Option<&State>,
    rule_id: RuleId,
    enter_pos: i64,
    anchor_pos: i64,
    begin_rule_captured_eol: bool,
    end_rule: Option<Rc<EndRule>>,
    name_scopes: Rc<AttrScope>,
    content_name_scopes: Rc<AttrScope>,
) -> State {
    Rc::new(StateNode {
        parent: parent.cloned(),
        rule_id,
        enter_pos: Cell::new(enter_pos),
        anchor_pos: Cell::new(anchor_pos),
        begin_rule_captured_eol,
        end_rule,
        name_scopes,
        content_name_scopes,
    })
}

fn with_content_name_scopes(state: &State, content: Rc<AttrScope>) -> State {
    if Rc::ptr_eq(&state.content_name_scopes, &content) {
        return state.clone();
    }
    push_state(
        state.parent.as_ref(),
        state.rule_id,
        state.enter_pos.get(),
        state.anchor_pos.get(),
        state.begin_rule_captured_eol,
        state.end_rule.clone(),
        state.name_scopes.clone(),
        content,
    )
}

fn with_end_rule(state: &State, end_rule: Rc<EndRule>) -> State {
    if state.end_rule.as_deref() == Some(&*end_rule) {
        return state.clone();
    }
    push_state(
        state.parent.as_ref(),
        state.rule_id,
        state.enter_pos.get(),
        state.anchor_pos.get(),
        state.begin_rule_captured_eol,
        Some(end_rule),
        state.name_scopes.clone(),
        state.content_name_scopes.clone(),
    )
}

fn has_same_rule_as(this: &State, other: &State) -> bool {
    let mut node = Some(this);
    while let Some(el) = node {
        if el.enter_pos.get() != other.enter_pos.get() {
            break;
        }
        if el.rule_id == other.rule_id {
            return true;
        }
        node = el.parent.as_ref();
    }
    false
}

/// `EncodedTokenMetadata.set` (0, `NotSet` or `None` leaves a field as it is).
fn set_metadata(existing: u32, language_id: u32, token_type: u32, font_style: i32, foreground: u32, background: u32) -> u32 {
    let mut language = existing & 0xFF;
    let mut kind = (existing & 0x300) >> 8;
    let balanced = (existing & 0x400) >> 10;
    let mut style = (existing & 0x7800) >> 11;
    let mut fg = (existing & 0xFF_8000) >> 15;
    let mut bg = (existing & 0xFF00_0000) >> 24;
    if language_id != 0 {
        language = language_id;
    }
    if token_type != 8 {
        kind = token_type;
    }
    if font_style != FONT_STYLE_NOT_SET {
        style = font_style as u32;
    }
    if foreground != 0 {
        fg = foreground;
    }
    if background != 0 {
        bg = background;
    }
    language | kind.wrapping_shl(8) | balanced << 10 | style.wrapping_shl(11) | fg.wrapping_shl(15) | bg.wrapping_shl(24)
}

pub fn foreground(metadata: u32) -> u32 {
    (metadata & 0xFF_8000) >> 15
}

pub fn font_style(metadata: u32) -> u32 {
    (metadata & 0x7800) >> 11
}

fn merge_attributes(existing: u32, token_type: u32, style: Option<StyleAttributes>) -> u32 {
    let (font_style, fg, bg) = match style {
        Some(s) => (s.font_style, s.foreground, s.background),
        None => (FONT_STYLE_NOT_SET, 0, 0),
    };
    set_metadata(existing, 0, token_type, font_style, fg, bg)
}

/// Binary tokens per theme for one line (`LineTokens` with `emitBinaryTokens`).
pub struct LineTokens {
    pub tokens: [Vec<(u32, u32)>; THEMES],
    last_end: i64,
}

impl LineTokens {
    fn new() -> Self {
        Self { tokens: Default::default(), last_end: 0 }
    }

    fn produce(&mut self, state: &State, end: usize) {
        self.produce_from_scopes(&state.content_name_scopes, end);
    }

    fn produce_from_scopes(&mut self, scopes: &AttrScope, end: usize) {
        if self.last_end >= end as i64 {
            return;
        }
        for (theme, tokens) in self.tokens.iter_mut().enumerate() {
            let metadata = scopes.attrs[theme];
            if tokens.last().is_none_or(|&(_, last)| last != metadata) {
                tokens.push((self.last_end as u32, metadata));
            }
        }
        self.last_end = end as i64;
    }

    /// `getBinaryResult` for one theme.
    pub fn binary_result(&self, theme: usize, state: &State, line_length: usize) -> Vec<(u32, u32)> {
        let mut tokens = self.tokens[theme].clone();
        if tokens.last().is_some_and(|&(start, _)| start as usize == line_length - 1) {
            tokens.pop();
        }
        if tokens.is_empty() {
            tokens.push((0, state.content_name_scopes.attrs[theme]));
        }
        tokens
    }
}

pub struct Tokenizer<'a> {
    pub grammar: &'a mut Grammar,
    pub themes: [&'a Theme; THEMES],
}

type Result<T> = std::result::Result<T, String>;

impl Tokenizer<'_> {
    fn push_attributed(&mut self, target: &Rc<AttrScope>, scope_path: Option<&str>) -> Rc<AttrScope> {
        let Some(scope_path) = scope_path else { return target.clone() };
        if !scope_path.contains(' ') {
            return self.push_one(target, scope_path);
        }
        let mut result = target.clone();
        for scope in scope_path.split(' ') {
            result = self.push_one(&result, scope);
        }
        result
    }

    fn push_one(&mut self, target: &Rc<AttrScope>, scope_name: &str) -> Rc<AttrScope> {
        let name: Rc<str> = Rc::from(scope_name);
        let token_type = self.grammar.token_type(&name);
        let path = Rc::new(ScopeStack { parent: Some(target.path.clone()), name });
        let mut attrs = [0; THEMES];
        for (theme, attr) in attrs.iter_mut().enumerate() {
            *attr = merge_attributes(target.attrs[theme], token_type, self.themes[theme].match_path(&path));
        }
        Rc::new(AttrScope { parent: Some(target.clone()), path, attrs })
    }

    fn initial_state(&mut self) -> State {
        let mut defaults = [0; THEMES];
        for (theme, default) in defaults.iter_mut().enumerate() {
            let style = self.themes[theme].defaults;
            *default = set_metadata(0, 1, 8, style.font_style, style.foreground, style.background);
        }
        let root_name = self.grammar.rule(self.grammar.root_id).name(None, None);
        let scopes = match root_name {
            Some(name) => {
                let token_type = self.grammar.token_type(&name);
                let path = Rc::new(ScopeStack { parent: None, name });
                let mut attrs = [0; THEMES];
                for (theme, attr) in attrs.iter_mut().enumerate() {
                    *attr = merge_attributes(defaults[theme], token_type, self.themes[theme].match_path(&path));
                }
                Rc::new(AttrScope { parent: None, path, attrs })
            }
            None => Rc::new(AttrScope { parent: None, path: Rc::new(ScopeStack { parent: None, name: Rc::from("unknown") }), attrs: defaults }),
        };
        push_state(None, self.grammar.root_id, -1, -1, false, None, scopes.clone(), scopes)
    }

    /// `Grammar._tokenize` + `tokenizeLine2`: tokenizes `line` (without its line
    /// break) after `previous` (`None`: the first line).
    pub fn tokenize_line(&mut self, line: &[u16], previous: Option<&State>) -> Result<(LineTokens, State, usize)> {
        let (is_first_line, state) = match previous {
            None => (true, self.initial_state()),
            Some(state) => {
                let mut node = Some(state);
                while let Some(el) = node {
                    el.enter_pos.set(-1);
                    el.anchor_pos.set(-1);
                    node = el.parent.as_ref();
                }
                (false, state.clone())
            }
        };
        let mut content = Vec::with_capacity(line.len() + 1);
        content.extend_from_slice(line);
        content.push(0x0A);
        let string = self.grammar.new_string(content);
        let line_length = string.content.len();
        let mut tokens = LineTokens::new();
        let state = self.tokenize_string(&string, is_first_line, 0, state, &mut tokens, true)?;
        Ok((tokens, state, line_length))
    }

    fn tokenize_string(&mut self, line: &OnigString, mut is_first_line: bool, mut line_pos: usize, mut stack: State, tokens: &mut LineTokens, check_while: bool) -> Result<State> {
        let line_length = line.content.len();
        let mut anchor_position: i64 = -1;
        if check_while {
            let result = self.check_while_conditions(line, is_first_line, line_pos, stack, tokens)?;
            stack = result.0;
            line_pos = result.1;
            is_first_line = result.2;
            anchor_position = result.3;
        }
        loop {
            let Some(found) = self.match_rule_or_injections(line, is_first_line, line_pos, &stack, anchor_position)? else {
                tokens.produce(&stack, line_length);
                return Ok(stack);
            };
            let captures = &found.captures;
            let (match_start, match_end) = (captures[0].0 as usize, captures[0].1 as usize);
            let has_advanced = !captures.is_empty() && match_end > line_pos;
            if found.rule_id == END_RULE_ID {
                let popped_rule = stack.rule_id;
                tokens.produce(&stack, match_start);
                stack = with_content_name_scopes(&stack, stack.name_scopes.clone());
                let end_captures = match &self.grammar.rule(popped_rule).kind {
                    RuleKind::BeginEnd { end_captures, .. } => end_captures.clone(),
                    _ => return Err("end match on a rule without end".into()),
                };
                self.handle_captures(line, is_first_line, &stack, tokens, &end_captures, captures)?;
                tokens.produce(&stack, match_end);
                let popped = stack.clone();
                stack = popped.parent.clone().ok_or("popped the root state")?;
                anchor_position = popped.anchor_pos.get();
                if !has_advanced && popped.enter_pos.get() == line_pos as i64 {
                    tokens.produce(&popped, line_length);
                    return Ok(popped);
                }
            } else {
                let rule_id = found.rule_id as RuleId;
                tokens.produce(&stack, match_start);
                let before_push = stack.clone();
                let scope_name = self.grammar.rule(rule_id).name(Some(&line.content), Some(captures));
                let name_scopes = self.push_attributed(&stack.content_name_scopes, scope_name.as_deref());
                stack = push_state(Some(&stack), rule_id, line_pos as i64, anchor_position, match_end == line_length, None, name_scopes.clone(), name_scopes.clone());
                enum Kind {
                    BeginEnd { captures: Vec<Option<RuleId>>, back_references: bool },
                    BeginWhile { captures: Vec<Option<RuleId>>, back_references: bool },
                    Match { captures: Vec<Option<RuleId>> },
                }
                let kind = match &self.grammar.rule(rule_id).kind {
                    RuleKind::BeginEnd { begin_captures, end, .. } => Kind::BeginEnd { captures: begin_captures.clone(), back_references: end.has_back_references },
                    RuleKind::BeginWhile { begin_captures, while_source, .. } => Kind::BeginWhile { captures: begin_captures.clone(), back_references: while_source.has_back_references },
                    RuleKind::Match { captures, .. } => Kind::Match { captures: captures.clone() },
                    _ => Kind::Match { captures: Vec::new() },
                };
                match kind {
                    Kind::BeginEnd { captures: begin_captures, back_references } | Kind::BeginWhile { captures: begin_captures, back_references } => {
                        self.handle_captures(line, is_first_line, &stack, tokens, &begin_captures, captures)?;
                        tokens.produce(&stack, match_end);
                        anchor_position = match_end as i64;
                        let content_name = self.grammar.rule(rule_id).content_name(&line.content, captures);
                        let content_scopes = self.push_attributed(&name_scopes, content_name.as_deref());
                        stack = with_content_name_scopes(&stack, content_scopes);
                        if back_references {
                            let values: Vec<Vec<u16>> = captures
                                .iter()
                                .map(|&(start, end)| {
                                    if start == UNMATCHED {
                                        Vec::new()
                                    } else {
                                        let (s, e) = ((start as usize).min(line_length), (end as usize).min(line_length));
                                        line.content[s..e.max(s)].to_vec()
                                    }
                                })
                                .collect();
                            stack = with_end_rule(&stack, Rc::new(EndRule { values: Rc::from(values) }));
                        }
                        if !has_advanced && has_same_rule_as(&before_push, &stack) {
                            let parent = stack.parent.clone().unwrap();
                            tokens.produce(&parent, line_length);
                            return Ok(parent);
                        }
                    }
                    Kind::Match { captures: match_captures } => {
                        self.handle_captures(line, is_first_line, &stack, tokens, &match_captures, captures)?;
                        tokens.produce(&stack, match_end);
                        stack = stack.parent.clone().unwrap();
                        if !has_advanced {
                            if let Some(parent) = stack.parent.clone() {
                                stack = parent;
                            }
                            tokens.produce(&stack, line_length);
                            return Ok(stack);
                        }
                    }
                }
            }
            if match_end > line_pos {
                line_pos = match_end;
                is_first_line = false;
            }
        }
    }

    fn check_while_conditions(&mut self, line: &OnigString, mut is_first_line: bool, mut line_pos: usize, mut stack: State, tokens: &mut LineTokens) -> Result<(State, usize, bool, i64)> {
        let mut anchor_position: i64 = if stack.begin_rule_captured_eol { 0 } else { -1 };
        let mut while_rules = Vec::new();
        let mut node = Some(stack.clone());
        while let Some(el) = node {
            if matches!(self.grammar.rule(el.rule_id).kind, RuleKind::BeginWhile { .. }) {
                while_rules.push(el.clone());
            }
            node = el.parent.clone();
        }
        while let Some(while_state) = while_rules.pop() {
            let compiled = self.grammar.compile_ag(while_state.rule_id, while_state.end_rule.as_deref(), is_first_line, line_pos as i64 == anchor_position, true)?;
            match self.grammar.find_next_match(&compiled, line, line_pos)? {
                Some(found) => {
                    if found.rule_id != WHILE_RULE_ID {
                        stack = while_state.parent.clone().unwrap();
                        break;
                    }
                    if !found.captures.is_empty() {
                        let (start, end) = (found.captures[0].0 as usize, found.captures[0].1 as usize);
                        tokens.produce(&while_state, start);
                        let while_captures = match &self.grammar.rule(while_state.rule_id).kind {
                            RuleKind::BeginWhile { while_captures, .. } => while_captures.clone(),
                            _ => unreachable!(),
                        };
                        self.handle_captures(line, is_first_line, &while_state, tokens, &while_captures, &found.captures)?;
                        tokens.produce(&while_state, end);
                        anchor_position = end as i64;
                        if end > line_pos {
                            line_pos = end;
                            is_first_line = false;
                        }
                    }
                }
                None => {
                    stack = while_state.parent.clone().unwrap();
                    break;
                }
            }
        }
        Ok((stack, line_pos, is_first_line, anchor_position))
    }

    fn match_rule_or_injections(&mut self, line: &OnigString, is_first_line: bool, line_pos: usize, stack: &State, anchor_position: i64) -> Result<Option<ScanMatch>> {
        let compiled = self.grammar.compile_ag(stack.rule_id, stack.end_rule.as_deref(), is_first_line, line_pos as i64 == anchor_position, false)?;
        let match_result = self.grammar.find_next_match(&compiled, line, line_pos)?;
        if self.grammar.injections.is_empty() {
            return Ok(match_result);
        }
        let Some((injection, priority_match)) = self.match_injections(line, is_first_line, line_pos, stack, anchor_position)? else {
            return Ok(match_result);
        };
        let Some(match_result) = match_result else { return Ok(Some(injection)) };
        let match_score = match_result.captures[0].0;
        let injection_score = injection.captures[0].0;
        if injection_score < match_score || (priority_match && injection_score == match_score) {
            return Ok(Some(injection));
        }
        Ok(Some(match_result))
    }

    fn match_injections(&mut self, line: &OnigString, is_first_line: bool, line_pos: usize, stack: &State, anchor_position: i64) -> Result<Option<(ScanMatch, bool)>> {
        let mut best_rating = u32::MAX;
        let mut best: Option<ScanMatch> = None;
        let mut best_priority = 0;
        let names = stack.content_name_scopes.scope_names();
        let scopes: Vec<&str> = names.iter().map(|s| &**s).collect();
        for i in 0..self.grammar.injections.len() {
            let injection = &self.grammar.injections[i];
            if !injection.matcher.matches(&scopes) {
                continue;
            }
            let (rule_id, priority) = (injection.rule_id, injection.priority);
            let compiled = self.grammar.compile_ag(rule_id, None, is_first_line, line_pos as i64 == anchor_position, false)?;
            let Some(found) = self.grammar.find_next_match(&compiled, line, line_pos)? else { continue };
            let rating = found.captures[0].0;
            if rating >= best_rating {
                continue;
            }
            best_rating = rating;
            best = Some(found);
            best_priority = priority;
            if best_rating as usize == line_pos {
                break;
            }
        }
        Ok(best.map(|found| (found, best_priority == -1)))
    }

    fn handle_captures(&mut self, line: &OnigString, is_first_line: bool, stack: &State, tokens: &mut LineTokens, captures: &[Option<RuleId>], indices: &[(u32, u32)]) -> Result<()> {
        if captures.is_empty() {
            return Ok(());
        }
        let len = captures.len().min(indices.len());
        let mut local: Vec<(Rc<AttrScope>, usize)> = Vec::new();
        let max_end = indices[0].1;
        for i in 0..len {
            let Some(capture_rule) = captures[i] else { continue };
            let (start, end) = indices[i];
            if end.wrapping_sub(start) == 0 {
                continue;
            }
            if start > max_end {
                break;
            }
            let (start, end) = (start as usize, end as usize);
            while let Some((scopes, local_end)) = local.last() {
                if *local_end > start {
                    break;
                }
                tokens.produce_from_scopes(scopes, *local_end);
                local.pop();
            }
            match local.last() {
                Some((scopes, _)) => tokens.produce_from_scopes(scopes, start),
                None => tokens.produce(stack, start),
            }
            let rule = self.grammar.rule(capture_rule);
            let retokenize = match rule.kind {
                RuleKind::Capture { retokenize } => retokenize,
                _ => 0,
            };
            if retokenize != 0 {
                let scope_name = rule.name(Some(&line.content), Some(indices));
                let content_name = rule.content_name(&line.content, indices);
                let name_scopes = self.push_attributed(&stack.content_name_scopes, scope_name.as_deref());
                let content_scopes = self.push_attributed(&name_scopes, content_name.as_deref());
                let pushed = push_state(Some(stack), retokenize, start as i64, -1, false, None, name_scopes, content_scopes);
                let sub = self.grammar.new_string(line.content[..end.min(line.content.len())].to_vec());
                self.tokenize_string(&sub, is_first_line && start == 0, start, pushed, tokens, false)?;
                continue;
            }
            if let Some(name) = rule.name(Some(&line.content), Some(indices)) {
                let base = local.last().map_or_else(|| stack.content_name_scopes.clone(), |(scopes, _)| scopes.clone());
                let scopes = self.push_attributed(&base, Some(&name));
                local.push((scopes, end));
            }
        }
        while let Some((scopes, end)) = local.pop() {
            tokens.produce_from_scopes(&scopes, end);
        }
        Ok(())
    }
}
