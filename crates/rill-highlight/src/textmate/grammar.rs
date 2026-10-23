//! Grammars and rules (vscode-textmate `rule.ts`, `grammar.ts`), and the scanner
//! (Shiki's `JavaScriptScanner` with its search cache shared by all scanners).

use super::matcher::{Matcher, create_matchers};
use super::raw::{RawGrammar, RawRule, ROOT_NODE, SELF_NODE};
use crate::regex::{EmulationOptions, JsRegExp, Matcher as RegexMatcher};
use crate::fxhash::FxHashMap as HashMap;
use std::rc::Rc;

pub type RuleId = u32;
pub const END_RULE_ID: i32 = -1;
pub const WHILE_RULE_ID: i32 = -2;
/// Capture start/end of a group that did not participate (vscode-oniguruma's MAX).
pub const UNMATCHED: u32 = u32::MAX;

/// An oniguruma-to-es translation: pattern, flags, emulation options.
#[derive(Debug, Clone)]
pub struct Translation {
    pub pattern: Rc<str>,
    pub flags: Rc<str>,
    pub options: EmulationOptions,
}

/// The grammars available to a highlighter (Shiki's registry for one `langs` list).
#[derive(Debug, Default)]
pub struct Registry {
    grammars: HashMap<Rc<str>, Rc<RawGrammar>>,
    /// Oniguruma source -> translation (`None`: skipped by the forgiving engine).
    translations: HashMap<Rc<str>, Option<Translation>>,
    /// Scope names injected into each target scope (Shiki `injectTo`), in load order.
    injections: Vec<(Rc<str>, Rc<str>)>,
}

impl Registry {
    pub fn add_grammar(&mut self, grammar: RawGrammar, translations: impl IntoIterator<Item = (Rc<str>, Option<Translation>)>) {
        for target in &grammar.inject_to {
            if !self.injections.iter().any(|(t, s)| t == target && *s == grammar.scope_name) {
                self.injections.push((target.clone(), grammar.scope_name.clone()));
            }
        }
        self.translations.extend(translations);
        self.grammars.insert(grammar.scope_name.clone(), Rc::new(grammar));
    }

    pub fn grammar(&self, scope_name: &str) -> Option<&Rc<RawGrammar>> {
        self.grammars.get(scope_name)
    }

    /// Shiki `Resolver.getInjections`: grammars injected into `scope_name` or any of
    /// its dotted prefixes.
    fn injections_for(&self, scope_name: &str) -> Vec<Rc<str>> {
        let parts: Vec<&str> = scope_name.split('.').collect();
        let mut result = Vec::new();
        for i in 1..=parts.len() {
            let prefix = parts[..i].join(".");
            for (target, scope) in &self.injections {
                if **target == *prefix {
                    result.push(scope.clone());
                }
            }
        }
        result
    }
}

/// A rule definition: a node of one grammar instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Desc {
    instance: u32,
    node: u32,
}

/// `initGrammar`: a grammar's rules with their own repository and `$base`.
#[derive(Debug)]
struct Instance {
    raw: Rc<RawGrammar>,
    rule_ids: Vec<RuleId>,
    repository: Rc<Repository>,
}

type Repository = HashMap<Rc<str>, Desc>;

/// vscode-textmate `RegExpSource`.
#[derive(Debug, Clone)]
pub struct RegexSource {
    /// `None` when the grammar gave a non-string.
    pub source: Option<Rc<str>>,
    pub has_anchor: bool,
    pub has_back_references: bool,
    pub rule_id: i32,
}

impl RegexSource {
    fn new(source: Option<&str>, rule_id: i32) -> Self {
        let Some(raw) = source else {
            return RegexSource { source: None, has_anchor: false, has_back_references: false, rule_id };
        };
        let bytes = raw.as_bytes();
        let mut output = String::new();
        let mut last = 0;
        let mut has_anchor = false;
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'\\' && pos + 1 < bytes.len() {
                match bytes[pos + 1] {
                    b'z' => {
                        output.push_str(&raw[last..pos]);
                        output.push_str("$(?!\\n)(?<!\\n)");
                        last = pos + 2;
                    }
                    b'A' | b'G' => has_anchor = true,
                    _ => {}
                }
                // Skip the escaped character (one UTF-16 unit in JS; a char here).
                pos += 1;
                pos += raw[pos..].chars().next().map_or(1, char::len_utf8);
                continue;
            }
            pos += raw[pos..].chars().next().map_or(1, char::len_utf8);
        }
        let source: Rc<str> = if last == 0 { Rc::from(raw) } else { output.push_str(&raw[last..]); Rc::from(output) };
        let has_back_references = has_back_reference(&source);
        RegexSource { source: Some(source), has_anchor, has_back_references, rule_id }
    }
}

/// /\\(\d+)/
fn has_back_reference(source: &str) -> bool {
    let bytes = source.as_bytes();
    bytes.windows(2).any(|w| w[0] == b'\\' && w[1].is_ascii_digit())
}

/// `_buildAnchorCache` variant for (allowA, allowG).
fn resolve_anchors(source: &str, allow_a: bool, allow_g: bool) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(match next {
                    'A' if !allow_a => '\u{FFFF}',
                    'G' if !allow_g => '\u{FFFF}',
                    other => other,
                });
            }
        }
    }
    out
}

/// The `\N` → placeholder template of a back-referencing end/while source.
fn back_reference_template(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let n: u64 = source[i + 1..j].parse().unwrap_or(u64::MAX);
            out.push(char::from_u32(0xE000 + n.min(0x8FF) as u32).unwrap());
            i = j;
        } else {
            let c = source[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// The resolved end/while pattern of a rule pushed with back-references: the
/// captured strings of the begin match, by group number.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndRule {
    pub values: Rc<[Vec<u16>]>,
}

#[derive(Debug)]
pub enum RuleKind {
    Capture {
        retokenize: RuleId,
    },
    Match {
        source: RegexSource,
        captures: Vec<Option<RuleId>>,
    },
    IncludeOnly {
        patterns: Vec<RuleId>,
        has_missing_patterns: bool,
    },
    BeginEnd {
        begin: RegexSource,
        begin_captures: Vec<Option<RuleId>>,
        end: RegexSource,
        end_captures: Vec<Option<RuleId>>,
        apply_end_pattern_last: bool,
        patterns: Vec<RuleId>,
        has_missing_patterns: bool,
    },
    BeginWhile {
        begin: RegexSource,
        begin_captures: Vec<Option<RuleId>>,
        while_source: RegexSource,
        while_captures: Vec<Option<RuleId>>,
        patterns: Vec<RuleId>,
        has_missing_patterns: bool,
    },
}

#[derive(Debug)]
pub struct Rule {
    pub name: Option<Rc<str>>,
    pub name_is_capturing: bool,
    pub content_name: Option<Rc<str>>,
    pub content_name_is_capturing: bool,
    pub kind: RuleKind,
}

/// CAPTURING_REGEX_SOURCE: /\$(\d+)|\${(\d+):\/(downcase|upcase)}/
fn has_captures(name: &str) -> bool {
    let bytes = name.as_bytes();
    (0..bytes.len()).any(|i| capture_reference_at(name, i).is_some())
}

enum CaseCommand {
    None,
    Downcase,
    Upcase,
}

/// A `$N` or `${N:/downcase|upcase}` at byte `i`: (group, command, length).
fn capture_reference_at(name: &str, i: usize) -> Option<(usize, CaseCommand, usize)> {
    let bytes = name.as_bytes();
    if bytes.get(i) != Some(&b'$') {
        return None;
    }
    let digits = |from: usize| {
        let mut j = from;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        j
    };
    let end = digits(i + 1);
    if end > i + 1 {
        return Some((name[i + 1..end].parse().unwrap_or(usize::MAX), CaseCommand::None, end - i));
    }
    if bytes.get(i + 1) == Some(&b'{') {
        let end = digits(i + 2);
        if end > i + 2 {
            let rest = &name[end..];
            for (suffix, command) in [(":/downcase}", CaseCommand::Downcase), (":/upcase}", CaseCommand::Upcase)] {
                if rest.starts_with(suffix) {
                    let group = name[i + 2..end].parse().unwrap_or(usize::MAX);
                    return Some((group, command, end + suffix.len() - i));
                }
            }
        }
    }
    None
}

/// `RegexSource.replaceCaptures`.
pub fn replace_captures(name: &str, line: &[u16], captures: &[(u32, u32)]) -> String {
    let mut out = String::with_capacity(name.len());
    let mut i = 0;
    let mut last = 0;
    while i < name.len() {
        if let Some((group, command, len)) = capture_reference_at(name, i) {
            if let Some(&(start, end)) = captures.get(group) {
                out.push_str(&name[last..i]);
                let start = (start as usize).min(line.len());
                let end = (end as usize).min(line.len()).max(start);
                let text = String::from_utf16_lossy(&line[start..end]);
                let text = text.trim_start_matches('.');
                match command {
                    CaseCommand::None => out.push_str(text),
                    CaseCommand::Downcase => out.push_str(&text.to_lowercase()),
                    CaseCommand::Upcase => out.push_str(&text.to_uppercase()),
                }
                i += len;
                last = i;
                continue;
            }
        }
        i += name[i..].chars().next().map_or(1, char::len_utf8);
    }
    out.push_str(&name[last..]);
    out
}

impl Rule {
    pub fn name(&self, line: Option<&[u16]>, captures: Option<&[(u32, u32)]>) -> Option<Rc<str>> {
        match (&self.name, line, captures) {
            (Some(name), Some(line), Some(captures)) if self.name_is_capturing => Some(Rc::from(replace_captures(name, line, captures))),
            (name, _, _) => name.clone(),
        }
    }

    pub fn content_name(&self, line: &[u16], captures: &[(u32, u32)]) -> Option<Rc<str>> {
        match &self.content_name {
            Some(name) if self.content_name_is_capturing => Some(Rc::from(replace_captures(name, line, captures))),
            name => name.clone(),
        }
    }
}

/// One injection (`Grammar._collectInjections`).
#[derive(Debug)]
pub struct Injection {
    pub matcher: Matcher,
    pub rule_id: RuleId,
    pub priority: i32,
}

/// A compiled pattern list: the scanner of one rule for one anchor variant.
#[derive(Debug)]
pub struct CompiledRule {
    pub regexes: Vec<Option<u32>>,
    pub rules: Vec<i32>,
}

#[derive(Debug, Default)]
struct SearchCacheEntry {
    string_id: u64,
    searched_from: usize,
    match_index: i64,
    captures: Vec<i32>,
}

/// A line (or a captured prefix of one) as the regex engine sees it.
pub struct OnigString {
    pub id: u64,
    pub content: Vec<u16>,
}

/// A scanner match: the matched rule and capture ranges (UNMATCHED for groups
/// that did not participate).
#[derive(Debug, Clone)]
pub struct ScanMatch {
    pub rule_id: i32,
    pub captures: Vec<(u32, u32)>,
}

pub struct Grammar {
    pub root_scope_name: Rc<str>,
    registry: Rc<Registry>,
    instances: Vec<Instance>,
    included: HashMap<Rc<str>, u32>,
    pub rules: Vec<Rule>,
    pub root_id: RuleId,
    pub injections: Vec<Injection>,
    /// Compiled regexes by id (`None`: skipped, as the forgiving JS engine does).
    regexes: Vec<RegexSlot>,
    regex_ids: HashMap<Rc<str>, u32>,
    dynamic_regex_ids: HashMap<(Rc<str>, Rc<[Vec<u16>]>), u32>,
    compiled: HashMap<(RuleId, u8, Option<Rc<[Vec<u16>]>>), Rc<CompiledRule>>,
    /// Scanners of rules without back-referencing end patterns, by rule and
    /// (allowA, allowG, while) slot.
    static_scanners: Vec<[Option<Rc<CompiledRule>>; 8]>,
    collected: HashMap<(RuleId, bool), Rc<Vec<RegexSource>>>,
    search_cache: Vec<SearchCacheEntry>,
    matcher: RegexMatcher,
    raw_captures: Vec<i32>,
    exec_result: Vec<i32>,
    next_string_id: u64,
    token_types: HashMap<Rc<str>, u32>,
    pub execs: u64,
}

impl Grammar {
    /// Creates the grammar for `scope_name` and compiles its rules
    /// (`getCompiledRuleId` of `$self`, then `getInjections`).
    pub fn new(registry: Rc<Registry>, scope_name: &str) -> Result<Grammar, String> {
        let raw = registry.grammar(scope_name).ok_or_else(|| format!("no grammar for {scope_name}"))?.clone();
        let mut grammar = Grammar {
            root_scope_name: raw.scope_name.clone(),
            registry,
            instances: Vec::new(),
            included: HashMap::default(),
            rules: Vec::new(),
            root_id: 0,
            injections: Vec::new(),
            regexes: Vec::new(),
            regex_ids: HashMap::default(),
            dynamic_regex_ids: HashMap::default(),
            compiled: HashMap::default(),
            static_scanners: Vec::new(),
            collected: HashMap::default(),
            search_cache: Vec::new(),
            matcher: RegexMatcher::default(),
            raw_captures: Vec::new(),
            exec_result: Vec::new(),
            next_string_id: 0,
            token_types: HashMap::default(),
            execs: 0,
        };
        let root = grammar.init_instance(raw, None);
        let repository = grammar.instances[root as usize].repository.clone();
        let self_desc = repository["$self"];
        grammar.root_id = grammar.compiled_rule_id(self_desc, &repository)?;
        grammar.collect_injections(root)?;
        Ok(grammar)
    }

    /// `initGrammar(grammar, base)`.
    fn init_instance(&mut self, raw: Rc<RawGrammar>, base: Option<Desc>) -> u32 {
        let index = self.instances.len() as u32;
        let mut repository: Repository = HashMap::default();
        if let Some(entries) = &raw.nodes[ROOT_NODE as usize].repository {
            for (name, node) in entries {
                repository.insert(name.clone(), Desc { instance: index, node: *node });
            }
        }
        let self_desc = Desc { instance: index, node: SELF_NODE };
        repository.insert(Rc::from("$self"), self_desc);
        repository.insert(Rc::from("$base"), base.unwrap_or(self_desc));
        let node_count = raw.nodes.len();
        self.instances.push(Instance { raw, rule_ids: vec![0; node_count], repository: Rc::new(repository) });
        index
    }

    /// `getExternalGrammar(scopeName, repository)`.
    fn external_grammar(&mut self, scope_name: &str, repository: Option<&Repository>) -> Option<u32> {
        if let Some(&index) = self.included.get(scope_name) {
            return Some(index);
        }
        let raw = self.registry.grammar(scope_name)?.clone();
        let base = repository.and_then(|r| r.get("$base").copied());
        let index = self.init_instance(raw, base);
        self.included.insert(Rc::from(scope_name), index);
        Some(index)
    }

    fn raw_rule(&self, desc: Desc) -> &RawRule {
        &self.instances[desc.instance as usize].raw.nodes[desc.node as usize]
    }

    fn register_rule(&mut self) -> RuleId {
        self.rules.push(Rule {
            name: None,
            name_is_capturing: false,
            content_name: None,
            content_name_is_capturing: false,
            kind: RuleKind::IncludeOnly { patterns: Vec::new(), has_missing_patterns: false },
        });
        self.rules.len() as RuleId
    }

    pub fn rule(&self, id: RuleId) -> &Rule {
        &self.rules[id as usize - 1]
    }

    fn make_rule(name: Option<Rc<str>>, content_name: Option<Rc<str>>, kind: RuleKind) -> Rule {
        let name_is_capturing = name.as_deref().is_some_and(has_captures);
        let content_name_is_capturing = content_name.as_deref().is_some_and(has_captures);
        Rule { name, name_is_capturing, content_name, content_name_is_capturing, kind }
    }

    /// `RuleFactory.getCompiledRuleId`.
    fn compiled_rule_id(&mut self, desc: Desc, repository: &Rc<Repository>) -> Result<RuleId, String> {
        let existing = self.instances[desc.instance as usize].rule_ids[desc.node as usize];
        if existing != 0 {
            return Ok(existing);
        }
        let id = self.register_rule();
        self.instances[desc.instance as usize].rule_ids[desc.node as usize] = id;
        let raw = self.instances[desc.instance as usize].raw.clone();
        let rule = &raw.nodes[desc.node as usize];
        let (name, content_name) = (rule.name.clone(), rule.content_name.clone());
        let built = if let Some(source) = &rule.match_source {
            let captures = self.compile_captures(desc.instance, rule.captures.as_deref(), repository)?;
            Self::make_rule(name, None, RuleKind::Match { source: RegexSource::new(Some(source), id as i32), captures })
        } else if rule.begin.is_none() {
            let mut repository = repository.clone();
            if let Some(entries) = &rule.repository {
                let mut merged = (*repository).clone();
                for (key, node) in entries {
                    merged.insert(key.clone(), Desc { instance: desc.instance, node: *node });
                }
                repository = Rc::new(merged);
            }
            let patterns = match (&rule.patterns, &rule.include) {
                (Some(patterns), _) => patterns.iter().map(|&node| PatternRef::Node(node)).collect(),
                (None, Some(include)) => vec![PatternRef::Include(include.clone())],
                (None, None) => Vec::new(),
            };
            let (patterns, has_missing_patterns) = self.compile_patterns(desc.instance, &patterns, &repository)?;
            Self::make_rule(name, content_name, RuleKind::IncludeOnly { patterns, has_missing_patterns })
        } else if let Some(while_source) = &rule.while_source {
            let begin_captures = self.compile_captures(desc.instance, rule.begin_captures.as_deref().or(rule.captures.as_deref()), repository)?;
            let while_captures = self.compile_captures(desc.instance, rule.while_captures.as_deref().or(rule.captures.as_deref()), repository)?;
            let patterns: Vec<PatternRef> = rule.patterns.iter().flatten().map(|&node| PatternRef::Node(node)).collect();
            let (patterns, has_missing_patterns) = self.compile_patterns(desc.instance, &patterns, repository)?;
            Self::make_rule(
                name,
                content_name,
                RuleKind::BeginWhile {
                    begin: RegexSource::new(rule.begin.clone().flatten().as_deref(), id as i32),
                    begin_captures,
                    while_source: RegexSource::new(Some(while_source), WHILE_RULE_ID),
                    while_captures,
                    patterns,
                    has_missing_patterns,
                },
            )
        } else {
            let begin_captures = self.compile_captures(desc.instance, rule.begin_captures.as_deref().or(rule.captures.as_deref()), repository)?;
            let end_captures = self.compile_captures(desc.instance, rule.end_captures.as_deref().or(rule.captures.as_deref()), repository)?;
            let patterns: Vec<PatternRef> = rule.patterns.iter().flatten().map(|&node| PatternRef::Node(node)).collect();
            let (patterns, has_missing_patterns) = self.compile_patterns(desc.instance, &patterns, repository)?;
            let end = match rule.end.as_deref() {
                Some(end) if !end.is_empty() => end,
                _ => "\u{FFFF}",
            };
            Self::make_rule(
                name,
                content_name,
                RuleKind::BeginEnd {
                    begin: RegexSource::new(rule.begin.clone().flatten().as_deref(), id as i32),
                    begin_captures,
                    end: RegexSource::new(Some(end), END_RULE_ID),
                    end_captures,
                    apply_end_pattern_last: rule.apply_end_pattern_last,
                    patterns,
                    has_missing_patterns,
                },
            )
        };
        self.rules[id as usize - 1] = built;
        Ok(id)
    }

    /// `RuleFactory._compileCaptures`.
    fn compile_captures(&mut self, instance: u32, captures: Option<&[(Rc<str>, u32)]>, repository: &Rc<Repository>) -> Result<Vec<Option<RuleId>>, String> {
        let Some(captures) = captures else { return Ok(Vec::new()) };
        let parse = |key: &str| -> Option<usize> {
            // parseInt: leading digits after optional whitespace/sign.
            let digits: String = key.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        };
        let maximum = captures.iter().filter_map(|(key, _)| parse(key)).max().unwrap_or(0);
        let mut result: Vec<Option<RuleId>> = vec![None; maximum + 1];
        for (key, node) in captures {
            let desc = Desc { instance, node: *node };
            let raw = self.instances[instance as usize].raw.clone();
            let capture = &raw.nodes[*node as usize];
            let retokenize = if capture.patterns.is_some() { self.compiled_rule_id(desc, repository)? } else { 0 };
            let id = self.register_rule();
            self.rules[id as usize - 1] = Self::make_rule(capture.name.clone(), capture.content_name.clone(), RuleKind::Capture { retokenize });
            if let Some(index) = parse(key) {
                result[index] = Some(id);
            }
        }
        Ok(result)
    }

    /// `RuleFactory._compilePatterns`.
    fn compile_patterns(&mut self, instance: u32, patterns: &[PatternRef], repository: &Rc<Repository>) -> Result<(Vec<RuleId>, bool), String> {
        let mut result = Vec::new();
        for pattern in patterns {
            let include = match pattern {
                PatternRef::Node(node) => self.instances[instance as usize].raw.nodes[*node as usize].include.clone(),
                PatternRef::Include(include) => Some(include.clone()),
            };
            let rule_id = match include {
                Some(include) => {
                    if &*include == "$base" || &*include == "$self" {
                        match repository.get(&include) {
                            Some(&desc) => Some(self.compiled_rule_id(desc, repository)?),
                            None => None,
                        }
                    } else if let Some(name) = include.strip_prefix('#') {
                        match repository.get(name) {
                            Some(&desc) => Some(self.compiled_rule_id(desc, repository)?),
                            None => None,
                        }
                    } else {
                        let (scope, rule_name) = match include.find('#') {
                            Some(sharp) => (&include[..sharp], Some(&include[sharp + 1..])),
                            None => (&*include, None),
                        };
                        match self.external_grammar(scope, Some(repository)) {
                            Some(external) => {
                                let external_repository = self.instances[external as usize].repository.clone();
                                let target = match rule_name {
                                    Some(rule_name) => external_repository.get(rule_name).copied(),
                                    None => external_repository.get("$self").copied(),
                                };
                                match target {
                                    Some(desc) => Some(self.compiled_rule_id(desc, &external_repository)?),
                                    None => None,
                                }
                            }
                            None => None,
                        }
                    }
                }
                None => match pattern {
                    PatternRef::Node(node) => Some(self.compiled_rule_id(Desc { instance, node: *node }, repository)?),
                    PatternRef::Include(_) => unreachable!(),
                },
            };
            if let Some(rule_id) = rule_id {
                let skip = match &self.rule(rule_id).kind {
                    RuleKind::IncludeOnly { patterns, has_missing_patterns }
                    | RuleKind::BeginEnd { patterns, has_missing_patterns, .. }
                    | RuleKind::BeginWhile { patterns, has_missing_patterns, .. } => *has_missing_patterns && patterns.is_empty(),
                    _ => false,
                };
                if !skip {
                    result.push(rule_id);
                }
            }
        }
        let has_missing = patterns.len() != result.len();
        Ok((result, has_missing))
    }

    /// `Grammar._collectInjections`.
    fn collect_injections(&mut self, root: u32) -> Result<(), String> {
        let raw = self.instances[root as usize].raw.clone();
        let repository = self.instances[root as usize].repository.clone();
        let mut injections = Vec::new();
        for (selector, node) in &raw.injections {
            let rule_id = self.compiled_rule_id(Desc { instance: root, node: *node }, &repository)?;
            for matcher in create_matchers(selector) {
                injections.push(Injection { matcher: matcher.matcher, rule_id, priority: matcher.priority });
            }
        }
        for scope in self.registry.injections_for(&self.root_scope_name) {
            let Some(instance) = self.external_grammar(&scope, None) else { continue };
            let injection_raw = self.instances[instance as usize].raw.clone();
            let Some(selector) = injection_raw.injection_selector.clone() else { continue };
            let injection_repository = self.instances[instance as usize].repository.clone();
            let rule_id = self.compiled_rule_id(Desc { instance, node: ROOT_NODE }, &injection_repository)?;
            for matcher in create_matchers(&selector) {
                injections.push(Injection { matcher: matcher.matcher, rule_id, priority: matcher.priority });
            }
        }
        injections.sort_by_key(|i| i.priority);
        self.injections = injections;
        Ok(())
    }

    /// Standard token type of a scope (`_toStandardTokenType`), 8 when not set.
    pub fn token_type(&mut self, scope: &Rc<str>) -> u32 {
        if let Some(&t) = self.token_types.get(scope) {
            return t;
        }
        let t = standard_token_type(scope);
        self.token_types.insert(scope.clone(), t);
        t
    }

    /// `collectPatterns` of the rule's own pattern list (`while_list` for the while
    /// pattern of a BeginWhile rule).
    fn collected_sources(&mut self, rule_id: RuleId, while_list: bool) -> Result<Rc<Vec<RegexSource>>, String> {
        if let Some(list) = self.collected.get(&(rule_id, while_list)) {
            return Ok(list.clone());
        }
        let mut out = Vec::new();
        match &self.rule(rule_id).kind {
            RuleKind::Match { source, .. } => out.push(source.clone()),
            RuleKind::IncludeOnly { patterns, .. } => {
                for &pattern in patterns.clone().iter() {
                    self.collect_patterns(pattern, &mut out, 0)?;
                }
            }
            RuleKind::BeginEnd { patterns, end, apply_end_pattern_last, .. } => {
                let (patterns, end, last) = (patterns.clone(), end.clone(), *apply_end_pattern_last);
                for pattern in patterns {
                    self.collect_patterns(pattern, &mut out, 0)?;
                }
                if last {
                    out.push(end);
                } else {
                    out.insert(0, end);
                }
            }
            RuleKind::BeginWhile { patterns, while_source, .. } => {
                if while_list {
                    out.push(while_source.clone());
                } else {
                    for pattern in patterns.clone() {
                        self.collect_patterns(pattern, &mut out, 0)?;
                    }
                }
            }
            RuleKind::Capture { .. } => return Err("capture rule has no patterns".into()),
        }
        let list = Rc::new(out);
        self.collected.insert((rule_id, while_list), list.clone());
        Ok(list)
    }

    /// `Rule.collectPatterns`.
    fn collect_patterns(&self, rule_id: RuleId, out: &mut Vec<RegexSource>, depth: u32) -> Result<(), String> {
        if depth > 1000 {
            return Err("include cycle".into());
        }
        match &self.rule(rule_id).kind {
            RuleKind::Match { source, .. } => out.push(source.clone()),
            RuleKind::IncludeOnly { patterns, .. } => {
                for &pattern in patterns {
                    self.collect_patterns(pattern, out, depth + 1)?;
                }
            }
            RuleKind::BeginEnd { begin, .. } | RuleKind::BeginWhile { begin, .. } => out.push(begin.clone()),
            RuleKind::Capture { .. } => return Err("capture rule in patterns".into()),
        }
        Ok(())
    }

    /// `compileAG` (or `compileWhileAG`): the scanner for `rule_id` with the end or
    /// while pattern resolved from `end_rule`.
    pub fn compile_ag(&mut self, rule_id: RuleId, end_rule: Option<&EndRule>, allow_a: bool, allow_g: bool, while_list: bool) -> Result<Rc<CompiledRule>, String> {
        let slot = usize::from(allow_a) * 4 + usize::from(allow_g) * 2 + usize::from(while_list);
        let index = rule_id as usize - 1;
        if let Some(Some(compiled)) = self.static_scanners.get(index).map(|s| &s[slot]) {
            return Ok(compiled.clone());
        }
        let compiled = self.compile_ag_uncached(rule_id, end_rule, allow_a, allow_g, while_list)?;
        Ok(compiled)
    }

    fn compile_ag_uncached(&mut self, rule_id: RuleId, end_rule: Option<&EndRule>, allow_a: bool, allow_g: bool, while_list: bool) -> Result<Rc<CompiledRule>, String> {
        let slot = usize::from(allow_a) * 4 + usize::from(allow_g) * 2 + usize::from(while_list);
        let sources = self.collected_sources(rule_id, while_list)?;
        let has_anchors = sources.iter().any(|s| s.has_anchor);
        let variant = if has_anchors { 1 + u8::from(allow_a) * 2 + u8::from(allow_g) } else { 0 } + if while_list { 8 } else { 0 };
        let dynamic = sources.iter().any(|s| s.has_back_references && (s.rule_id == END_RULE_ID || s.rule_id == WHILE_RULE_ID));
        let values = if dynamic { end_rule.map(|e| e.values.clone()) } else { None };
        let key = (rule_id, variant, values.clone());
        if let Some(compiled) = self.compiled.get(&key) {
            return Ok(compiled.clone());
        }
        let mut regexes = Vec::with_capacity(sources.len());
        let mut rules = Vec::with_capacity(sources.len());
        for source in sources.iter() {
            let is_end = source.rule_id == END_RULE_ID || source.rule_id == WHILE_RULE_ID;
            let regex = match &source.source {
                None => None,
                Some(text) if is_end && source.has_back_references => match (&values, source.rule_id) {
                    (Some(values), _) => self.dynamic_regex(text, source.has_anchor, allow_a, allow_g, values)?,
                    // A while rule without a resolved pattern uses "\uFFFF".
                    (None, WHILE_RULE_ID) => self.static_regex("\u{FFFF}")?,
                    (None, _) => None,
                },
                Some(text) => {
                    if source.has_anchor {
                        let resolved = resolve_anchors(text, allow_a, allow_g);
                        self.static_regex(&resolved)?
                    } else {
                        self.static_regex(text)?
                    }
                }
            };
            regexes.push(regex);
            rules.push(source.rule_id);
        }
        let compiled = Rc::new(CompiledRule { regexes, rules });
        self.compiled.insert(key, compiled.clone());
        if !dynamic {
            let index = rule_id as usize - 1;
            if self.static_scanners.len() <= index {
                self.static_scanners.resize_with(index + 1, Default::default);
            }
            self.static_scanners[index][slot] = Some(compiled.clone());
        }
        Ok(compiled)
    }

    fn static_regex(&mut self, source: &str) -> Result<Option<u32>, String> {
        if let Some(&id) = self.regex_ids.get(source) {
            return Ok(Some(id));
        }
        let translation = match self.registry.translations.get(source) {
            Some(translation) => translation.clone(),
            None => return Err(format!("no translation for {source:?}")),
        };
        let id = self.regexes.len() as u32;
        self.regexes.push(match translation {
            Some(t) => RegexSlot::Pending(t, None),
            None => RegexSlot::Skipped,
        });
        self.search_cache.push(SearchCacheEntry::default());
        self.regex_ids.insert(Rc::from(source), id);
        Ok(Some(id))
    }

    fn dynamic_regex(&mut self, source: &str, has_anchor: bool, allow_a: bool, allow_g: bool, values: &Rc<[Vec<u16>]>) -> Result<Option<u32>, String> {
        let mut template = back_reference_template(source);
        if has_anchor {
            template = resolve_anchors(&template, allow_a, allow_g);
        }
        let template: Rc<str> = Rc::from(template);
        if let Some(&id) = self.dynamic_regex_ids.get(&(template.clone(), values.clone())) {
            return Ok(Some(id));
        }
        let translation = match self.registry.translations.get(&template) {
            Some(translation) => translation.clone(),
            None => return Err(format!("no translation for template {template:?}")),
        };
        let id = self.regexes.len() as u32;
        self.regexes.push(match translation {
            Some(t) => RegexSlot::Pending(t, Some(values.clone())),
            None => RegexSlot::Skipped,
        });
        self.search_cache.push(SearchCacheEntry::default());
        self.dynamic_regex_ids.insert((template, values.clone()), id);
        Ok(Some(id))
    }

    pub fn new_string(&mut self, content: Vec<u16>) -> OnigString {
        self.next_string_id += 1;
        OnigString { id: self.next_string_id, content }
    }

    /// `JavaScriptScanner.findNextMatchSync`, with the search cache kept per regex
    /// instead of per scanner, so scanners sharing a pattern share its searches.
    /// The compiled regex of a slot, compiling it on first use (`None`: skipped).
    fn regex(&mut self, id: usize) -> Result<Option<Rc<JsRegExp>>, String> {
        match &self.regexes[id] {
            RegexSlot::Ready(regex) => return Ok(Some(regex.clone())),
            RegexSlot::Skipped => return Ok(None),
            RegexSlot::Pending(..) => {}
        }
        let RegexSlot::Pending(t, values) = std::mem::replace(&mut self.regexes[id], RegexSlot::Skipped) else { unreachable!() };
        let regex = match values {
            None => JsRegExp::new(&t.pattern, &t.flags, &t.options),
            Some(values) => JsRegExp::with_placeholders(&t.pattern, &t.flags, &t.options, &values),
        }
        .map_err(|e| format!("{e} in {}", t.pattern))?;
        let regex = Rc::new(regex);
        self.regexes[id] = RegexSlot::Ready(regex.clone());
        Ok(Some(regex))
    }

    pub fn find_next_match(&mut self, compiled: &CompiledRule, string: &OnigString, start: usize) -> Result<Option<ScanMatch>, String> {
        let mut best: Option<(usize, i64)> = None;
        for (index, regex_id) in compiled.regexes.iter().enumerate() {
            let Some(regex_id) = *regex_id else { continue };
            let regex_index = regex_id as usize;
            let Some(regex) = self.regex(regex_index)? else { continue };
            let cacheable = regex.is_search_cacheable();
            let entry = &self.search_cache[regex_index];
            let reuse = cacheable
                && entry.string_id == string.id
                && entry.searched_from <= start
                && (entry.match_index == -1 || entry.match_index >= start as i64);
            let match_index = if reuse {
                entry.match_index
            } else {
                self.execs += 1;
                let found = regex.exec(&mut self.matcher, &string.content, start, &mut self.raw_captures, &mut self.exec_result);
                let entry = &mut self.search_cache[regex_index];
                entry.string_id = string.id;
                entry.searched_from = start;
                entry.captures.clear();
                if found {
                    entry.match_index = i64::from(self.exec_result[0]);
                    entry.captures.extend_from_slice(&self.exec_result);
                } else {
                    entry.match_index = -1;
                }
                if !cacheable {
                    // Not reusable later; mark so the next lookup re-executes.
                    entry.string_id = 0;
                }
                entry.match_index
            };
            if match_index == -1 {
                continue;
            }
            if match_index == start as i64 {
                return Ok(Some(self.scan_match(compiled.rules[index], regex_index)));
            }
            if best.is_none_or(|(_, at)| match_index < at) {
                best = Some((index, match_index));
            }
        }
        // The winner's captures are still in its cache entry: every regex is executed
        // at most once per call, and only the winner can be the best.
        let Some((index, _)) = best else { return Ok(None) };
        let regex_index = compiled.regexes[index].unwrap() as usize;
        Ok(Some(self.scan_match(compiled.rules[index], regex_index)))
    }

    fn scan_match(&self, rule_id: i32, regex_index: usize) -> ScanMatch {
        let raw = &self.search_cache[regex_index].captures;
        let captures = raw
            .chunks(2)
            .map(|pair| if pair[0] < 0 { (UNMATCHED, UNMATCHED) } else { (pair[0] as u32, pair[1] as u32) })
            .collect();
        ScanMatch { rule_id, captures }
    }
}

#[derive(Debug)]
enum RegexSlot {
    /// Not compiled yet: the translation and, for back-reference templates, the
    /// captured values.
    Pending(Translation, Option<Rc<[Vec<u16>]>>),
    Ready(Rc<JsRegExp>),
    Skipped,
}

enum PatternRef {
    Node(u32),
    Include(Rc<str>),
}

/// /\b(comment|string|regex|meta\.embedded)\b/ -> Comment 1, String 2, RegEx 3,
/// Other 0; 8 (NotSet) when absent.
fn standard_token_type(scope: &str) -> u32 {
    let bytes = scope.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    for start in 0..bytes.len() {
        if start > 0 && is_word(bytes[start - 1]) {
            continue;
        }
        for (word, kind) in [("comment", 1), ("string", 2), ("regex", 3), ("meta.embedded", 0)] {
            if bytes[start..].starts_with(word.as_bytes()) && !bytes.get(start + word.len()).is_some_and(|&b| is_word(b)) {
                return kind;
            }
        }
    }
    8
}
