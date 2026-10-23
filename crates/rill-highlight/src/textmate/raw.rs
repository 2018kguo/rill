//! Raw TextMate grammars: the JSON rule objects, flattened into an arena so rules
//! can be referred to (and their compiled ids cached) by index.

use serde_json::{Map, Value};
use std::rc::Rc;

/// The root object of a grammar.
pub const ROOT_NODE: u32 = 0;
/// The `$self` rule vscode-textmate's `initGrammar` adds: `{ patterns, name: scopeName }`.
pub const SELF_NODE: u32 = 1;

#[derive(Debug, Default)]
pub struct RawRule {
    pub name: Option<Rc<str>>,
    pub content_name: Option<Rc<str>>,
    /// `match`, when it is a non-empty string (vscode-textmate tests `desc.match`).
    pub match_source: Option<Rc<str>>,
    /// `begin`, when present at all (`typeof desc.begin !== "undefined"`); the
    /// inner value is the source when it is a string.
    pub begin: Option<Option<Rc<str>>>,
    pub end: Option<Rc<str>>,
    /// `while`, when it is a non-empty string.
    pub while_source: Option<Rc<str>>,
    pub include: Option<Rc<str>>,
    pub captures: Option<Vec<(Rc<str>, u32)>>,
    pub begin_captures: Option<Vec<(Rc<str>, u32)>>,
    pub end_captures: Option<Vec<(Rc<str>, u32)>>,
    pub while_captures: Option<Vec<(Rc<str>, u32)>>,
    pub patterns: Option<Vec<u32>>,
    pub repository: Option<Vec<(Rc<str>, u32)>>,
    pub apply_end_pattern_last: bool,
}

#[derive(Debug)]
pub struct RawGrammar {
    pub name: Rc<str>,
    pub scope_name: Rc<str>,
    pub nodes: Vec<RawRule>,
    /// `injections`: selector and rule node, in object order.
    pub injections: Vec<(Rc<str>, u32)>,
    pub injection_selector: Option<Rc<str>>,
    /// Shiki's `injectTo`: scope names this grammar injects into.
    pub inject_to: Vec<Rc<str>>,
}

fn string(value: Option<&Value>) -> Option<Rc<str>> {
    match value {
        Some(Value::String(s)) => Some(Rc::from(s.as_str())),
        _ => None,
    }
}

/// JavaScript truthiness of a JSON value.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// Object keys in JavaScript property order: integer-like keys ascending, then the
/// others in insertion order.
fn js_ordered(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let is_index = |k: &str| !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()) && (k == "0" || !k.starts_with('0')) && k.len() < 10;
    let mut indices: Vec<(&String, &Value)> = map.iter().filter(|(k, _)| is_index(k)).collect();
    indices.sort_by_key(|(k, _)| k.parse::<u64>().unwrap());
    indices.extend(map.iter().filter(|(k, _)| !is_index(k)));
    indices
}

impl RawGrammar {
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let object = value.as_object().ok_or("grammar is not an object")?;
        let scope_name = string(object.get("scopeName")).ok_or("grammar without scopeName")?;
        let mut grammar = RawGrammar {
            name: string(object.get("name")).unwrap_or_else(|| scope_name.clone()),
            scope_name: scope_name.clone(),
            nodes: vec![RawRule::default(), RawRule::default()],
            injections: Vec::new(),
            injection_selector: string(object.get("injectionSelector")),
            inject_to: match object.get("injectTo") {
                Some(Value::Array(items)) => items.iter().filter_map(|v| string(Some(v))).collect(),
                _ => Vec::new(),
            },
        };
        let root = grammar.read_rule(object);
        if let Some(Value::Object(injections)) = object.get("injections") {
            for (selector, rule) in js_ordered(injections) {
                if let Value::Object(rule) = rule {
                    let node = grammar.add_rule(rule);
                    grammar.injections.push((Rc::from(selector.as_str()), node));
                }
            }
        }
        grammar.nodes[SELF_NODE as usize] = RawRule {
            name: Some(scope_name),
            patterns: root.patterns.clone(),
            ..RawRule::default()
        };
        grammar.nodes[ROOT_NODE as usize] = root;
        Ok(grammar)
    }

    fn add_rule(&mut self, object: &Map<String, Value>) -> u32 {
        let index = self.nodes.len() as u32;
        self.nodes.push(RawRule::default());
        let rule = self.read_rule(object);
        self.nodes[index as usize] = rule;
        index
    }

    fn read_rule(&mut self, object: &Map<String, Value>) -> RawRule {
        let mut rule = RawRule {
            name: string(object.get("name")),
            content_name: string(object.get("contentName")),
            match_source: if truthy(object.get("match")) { string(object.get("match")) } else { None },
            begin: object.get("begin").map(|v| string(Some(v))),
            end: string(object.get("end")),
            while_source: if truthy(object.get("while")) { string(object.get("while")) } else { None },
            include: if truthy(object.get("include")) { string(object.get("include")) } else { None },
            apply_end_pattern_last: truthy(object.get("applyEndPatternLast")),
            ..RawRule::default()
        };
        // vscode-textmate tests `desc.match` and `desc.while` for truthiness; a
        // truthy non-string value still selects the rule kind.
        if truthy(object.get("match")) && rule.match_source.is_none() {
            rule.match_source = Some(Rc::from(""));
        }
        if truthy(object.get("while")) && rule.while_source.is_none() {
            rule.while_source = Some(Rc::from(""));
        }
        rule.captures = self.read_captures(object.get("captures"));
        rule.begin_captures = self.read_captures(object.get("beginCaptures"));
        rule.end_captures = self.read_captures(object.get("endCaptures"));
        rule.while_captures = self.read_captures(object.get("whileCaptures"));
        if let Some(Value::Array(patterns)) = object.get("patterns") {
            let mut nodes = Vec::with_capacity(patterns.len());
            for pattern in patterns {
                match pattern {
                    Value::Object(pattern) => nodes.push(self.add_rule(pattern)),
                    // Non-object patterns become empty rules.
                    _ => nodes.push(self.add_rule(&Map::new())),
                }
            }
            rule.patterns = Some(nodes);
        }
        if let Some(Value::Object(repository)) = object.get("repository") {
            let mut entries = Vec::with_capacity(repository.len());
            for (name, value) in js_ordered(repository) {
                if let Value::Object(value) = value {
                    entries.push((Rc::from(name.as_str()), self.add_rule(value)));
                }
            }
            rule.repository = Some(entries);
        }
        rule
    }

    fn read_captures(&mut self, value: Option<&Value>) -> Option<Vec<(Rc<str>, u32)>> {
        // `desc.beginCaptures || desc.captures`: any object (even empty) is truthy.
        let Some(Value::Object(captures)) = value else { return None };
        let mut entries = Vec::with_capacity(captures.len());
        for (key, value) in js_ordered(captures) {
            let node = match value {
                Value::Object(value) => self.add_rule(value),
                _ => self.add_rule(&Map::new()),
            };
            entries.push((Rc::from(key.as_str()), node));
        }
        Some(entries)
    }
}
