//! Themes: Shiki's `normalizeTheme` and vscode-textmate's theme trie.

use serde_json::{Map, Value, json};
use std::cell::RefCell;
use crate::fxhash::FxHashMap as HashMap;
use std::rc::Rc;

pub const FONT_STYLE_NOT_SET: i32 = -1;

/// A theme normalized as Shiki does before handing it to vscode-textmate.
#[derive(Debug, Clone)]
pub struct NormalizedTheme {
    pub name: String,
    pub fg: String,
    pub bg: String,
    pub settings: Vec<Value>,
    /// Lower-case color -> replacement.
    pub color_replacements: HashMap<String, String>,
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Shiki `normalizeTheme`.
pub fn normalize_theme(raw: &Value) -> Result<NormalizedTheme, String> {
    let mut theme: Map<String, Value> = raw.as_object().ok_or("theme is not an object")?.clone();
    if theme.contains_key("tokenColors") && !truthy(theme.get("settings")) {
        let token_colors = theme.remove("tokenColors").unwrap();
        theme.insert("settings".into(), token_colors);
    }
    let theme_type = non_empty(theme.get("type")).unwrap_or_else(|| "dark".into());
    let mut color_replacements: Map<String, Value> = match theme.get("colorReplacements") {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    let mut settings: Vec<Value> = match theme.get("settings") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let mut fg = non_empty(theme.get("fg"));
    let mut bg = non_empty(theme.get("bg"));
    let mut colors: Map<String, Value> = match theme.get("colors") {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    if fg.is_none() || bg.is_none() {
        let global = settings.iter().find(|s| !truthy(s.get("name")) && !truthy(s.get("scope")));
        if let Some(global) = global {
            if let Some(foreground) = non_empty(global.get("settings").and_then(|s| s.get("foreground"))) {
                fg = Some(foreground);
            }
            if let Some(background) = non_empty(global.get("settings").and_then(|s| s.get("background"))) {
                bg = Some(background);
            }
        }
        if fg.is_none() {
            fg = non_empty(colors.get("editor.foreground"));
        }
        if bg.is_none() {
            bg = non_empty(colors.get("editor.background"));
        }
        let light = theme_type == "light";
        if fg.is_none() {
            fg = Some(if light { "#333333" } else { "#bbbbbb" }.into());
        }
        if bg.is_none() {
            bg = Some(if light { "#fffffe" } else { "#1e1e1e" }.into());
        }
    }
    let fg = fg.unwrap();
    let bg = bg.unwrap();
    let first_is_global = settings
        .first()
        .is_some_and(|s| truthy(s.get("settings")) && !truthy(s.get("scope")));
    if !first_is_global {
        settings.insert(0, json!({ "settings": { "foreground": fg, "background": bg } }));
    }
    // Colors that are not hex get placeholder hex colors, replaced back on output.
    let mut replacement_count = 0u32;
    let mut replacement_map: HashMap<String, String> = HashMap::default();
    let mut replacement_for = |value: &str, replacements: &Map<String, Value>| -> String {
        if let Some(hex) = replacement_map.get(value) {
            return hex.clone();
        }
        loop {
            replacement_count += 1;
            let hex = format!("#{:08x}", replacement_count);
            if replacements.contains_key(&format!("#{hex}")) {
                continue;
            }
            replacement_map.insert(value.to_string(), hex.clone());
            return hex;
        }
    };
    for setting in settings.iter_mut() {
        let Some(inner) = setting.get("settings").and_then(Value::as_object) else { continue };
        let replace = |key: &str| inner.get(key).and_then(Value::as_str).is_some_and(|c| !c.is_empty() && !c.starts_with('#'));
        let (replace_fg, replace_bg) = (replace("foreground"), replace("background"));
        if !replace_fg && !replace_bg {
            continue;
        }
        let mut inner = inner.clone();
        for (key, enabled) in [("foreground", replace_fg), ("background", replace_bg)] {
            if enabled {
                let original = inner[key].as_str().unwrap().to_string();
                let hex = replacement_for(&original, &color_replacements);
                color_replacements.insert(hex.clone(), Value::String(original));
                inner.insert(key.into(), Value::String(hex));
            }
        }
        setting.as_object_mut().unwrap().insert("settings".into(), Value::Object(inner));
    }
    let keys: Vec<String> = colors.keys().cloned().collect();
    for key in keys {
        if key == "editor.foreground" || key == "editor.background" || key.starts_with("terminal.ansi") {
            let value = colors[&key].as_str().unwrap_or("").to_string();
            if !value.starts_with('#') {
                let hex = replacement_for(&value, &color_replacements);
                color_replacements.insert(hex.clone(), Value::String(value));
                colors.insert(key, Value::String(hex));
            }
        }
    }
    Ok(NormalizedTheme {
        name: non_empty(theme.get("name")).unwrap_or_default(),
        fg,
        bg,
        settings,
        color_replacements: color_replacements
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|v| (k, v.to_string())))
            .collect(),
    })
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleAttributes {
    pub font_style: i32,
    pub foreground: u32,
    pub background: u32,
}

#[derive(Debug, Clone)]
struct TrieRule {
    scope_depth: u32,
    parent_scopes: Rc<[Rc<str>]>,
    font_style: i32,
    foreground: u32,
    background: u32,
}

impl TrieRule {
    fn accept_overwrite(&mut self, scope_depth: u32, font_style: i32, foreground: u32, background: u32) {
        if self.scope_depth <= scope_depth {
            self.scope_depth = scope_depth;
        }
        if font_style != FONT_STYLE_NOT_SET {
            self.font_style = font_style;
        }
        if foreground != 0 {
            self.foreground = foreground;
        }
        if background != 0 {
            self.background = background;
        }
    }
}

#[derive(Debug, Clone)]
struct TrieElement {
    main_rule: TrieRule,
    rules_with_parent_scopes: Vec<TrieRule>,
    children: HashMap<String, TrieElement>,
}

fn cmp_by_specificity(a: &TrieRule, b: &TrieRule) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a.scope_depth != b.scope_depth {
        return b.scope_depth.cmp(&a.scope_depth);
    }
    let (mut ai, mut bi) = (0, 0);
    loop {
        if a.parent_scopes.get(ai).is_some_and(|s| &**s == ">") {
            ai += 1;
        }
        if b.parent_scopes.get(bi).is_some_and(|s| &**s == ">") {
            bi += 1;
        }
        if ai >= a.parent_scopes.len() || bi >= b.parent_scopes.len() {
            break;
        }
        let diff = b.parent_scopes[bi].encode_utf16().count() as i64 - a.parent_scopes[ai].encode_utf16().count() as i64;
        if diff != 0 {
            return if diff < 0 { Ordering::Less } else { Ordering::Greater };
        }
        ai += 1;
        bi += 1;
    }
    (b.parent_scopes.len() as i64).cmp(&(a.parent_scopes.len() as i64))
}

fn split_head(scope: &str) -> (&str, &str) {
    match scope.find('.') {
        Some(dot) => (&scope[..dot], &scope[dot + 1..]),
        None => (scope, ""),
    }
}

impl TrieElement {
    fn find(&self, scope: &str) -> Vec<TrieRule> {
        if !scope.is_empty() {
            let (head, tail) = split_head(scope);
            if let Some(child) = self.children.get(head) {
                return child.find(tail);
            }
        }
        let mut rules = self.rules_with_parent_scopes.clone();
        rules.push(self.main_rule.clone());
        rules.sort_by(cmp_by_specificity);
        rules
    }

    fn insert(&mut self, scope_depth: u32, scope: &str, parent_scopes: Option<Rc<[Rc<str>]>>, font_style: i32, foreground: u32, background: u32) {
        if scope.is_empty() {
            self.insert_here(scope_depth, parent_scopes, font_style, foreground, background);
            return;
        }
        let (head, tail) = split_head(scope);
        if !self.children.contains_key(head) {
            let child = TrieElement {
                main_rule: self.main_rule.clone(),
                rules_with_parent_scopes: self.rules_with_parent_scopes.clone(),
                children: HashMap::default(),
            };
            self.children.insert(head.to_string(), child);
        }
        self.children.get_mut(head).unwrap().insert(scope_depth + 1, tail, parent_scopes, font_style, foreground, background);
    }

    fn insert_here(&mut self, scope_depth: u32, parent_scopes: Option<Rc<[Rc<str>]>>, mut font_style: i32, mut foreground: u32, mut background: u32) {
        let Some(parent_scopes) = parent_scopes else {
            self.main_rule.accept_overwrite(scope_depth, font_style, foreground, background);
            return;
        };
        for rule in &mut self.rules_with_parent_scopes {
            if rule.parent_scopes[..] == parent_scopes[..] {
                rule.accept_overwrite(scope_depth, font_style, foreground, background);
                return;
            }
        }
        if font_style == FONT_STYLE_NOT_SET {
            font_style = self.main_rule.font_style;
        }
        if foreground == 0 {
            foreground = self.main_rule.foreground;
        }
        if background == 0 {
            background = self.main_rule.background;
        }
        self.rules_with_parent_scopes.push(TrieRule { scope_depth, parent_scopes, font_style, foreground, background });
    }
}

/// vscode-textmate's `Theme`: scope paths to style attributes.
#[derive(Debug)]
pub struct Theme {
    /// Color id -> color (upper case); id 0 is unused.
    pub color_map: Vec<Option<String>>,
    pub defaults: StyleAttributes,
    root: TrieElement,
    cache: RefCell<HashMap<Rc<str>, Rc<Vec<TrieRule>>>>,
}

fn is_valid_hex_color(hex: &str) -> bool {
    let Some(digits) = hex.strip_prefix('#') else { return false };
    matches!(digits.len(), 3 | 4 | 6 | 8) && digits.bytes().all(|b| b.is_ascii_hexdigit())
}

struct ParsedRule {
    scope: String,
    parent_scopes: Option<Vec<Rc<str>>>,
    index: usize,
    font_style: i32,
    foreground: Option<String>,
    background: Option<String>,
}

/// JavaScript `<` on strings: UTF-16 code unit order.
fn js_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

impl Theme {
    pub fn new(settings: &[Value]) -> Theme {
        let mut parsed = parse_theme(settings);
        parsed.sort_by(|a, b| {
            js_cmp(&a.scope, &b.scope)
                .then_with(|| match (&a.parent_scopes, &b.parent_scopes) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (Some(x), Some(y)) => {
                        if x.len() == y.len() {
                            x.iter().zip(y.iter()).map(|(p, q)| js_cmp(p, q)).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal)
                        } else {
                            x.len().cmp(&y.len())
                        }
                    }
                })
                .then(a.index.cmp(&b.index))
        });
        let mut default_font_style = 0;
        let mut default_foreground = "#000000".to_string();
        let mut default_background = "#ffffff".to_string();
        let mut rest = parsed.as_slice();
        while let Some((first, tail)) = rest.split_first() {
            if !first.scope.is_empty() {
                break;
            }
            if first.font_style != FONT_STYLE_NOT_SET {
                default_font_style = first.font_style;
            }
            if let Some(fg) = &first.foreground {
                default_foreground = fg.clone();
            }
            if let Some(bg) = &first.background {
                default_background = bg.clone();
            }
            rest = tail;
        }
        let mut color_map = ColorMap::default();
        let defaults = StyleAttributes {
            font_style: default_font_style,
            foreground: color_map.id(Some(&default_foreground)),
            background: color_map.id(Some(&default_background)),
        };
        let mut root = TrieElement {
            main_rule: TrieRule { scope_depth: 0, parent_scopes: Rc::from(Vec::new()), font_style: FONT_STYLE_NOT_SET, foreground: 0, background: 0 },
            rules_with_parent_scopes: Vec::new(),
            children: HashMap::default(),
        };
        for rule in rest {
            let foreground = color_map.id(rule.foreground.as_deref());
            let background = color_map.id(rule.background.as_deref());
            root.insert(0, &rule.scope, rule.parent_scopes.clone().map(Rc::from), rule.font_style, foreground, background);
        }
        Theme { color_map: color_map.colors, defaults, root, cache: RefCell::new(HashMap::default()) }
    }

    /// `Theme.match`.
    pub fn match_path(&self, path: &ScopeStack) -> Option<StyleAttributes> {
        let rules = {
            let mut cache = self.cache.borrow_mut();
            cache.entry(path.name.clone()).or_insert_with(|| Rc::new(self.root.find(&path.name))).clone()
        };
        let rule = rules.iter().find(|rule| scope_path_matches_parent_scopes(path.parent.as_deref(), &rule.parent_scopes))?;
        Some(StyleAttributes { font_style: rule.font_style, foreground: rule.foreground, background: rule.background })
    }
}

/// vscode-textmate's `ScopeStack`: a scope name and its parents.
#[derive(Debug)]
pub struct ScopeStack {
    pub parent: Option<Rc<ScopeStack>>,
    pub name: Rc<str>,
}

/// `_scopePathMatchesParentScopes` (`parent_scopes` nearest first).
fn scope_path_matches_parent_scopes(mut path: Option<&ScopeStack>, parent_scopes: &[Rc<str>]) -> bool {
    let mut index = 0;
    while index < parent_scopes.len() {
        let mut pattern: &str = &parent_scopes[index];
        let mut must_match = false;
        if pattern == ">" {
            if index == parent_scopes.len() - 1 {
                return false;
            }
            index += 1;
            pattern = &parent_scopes[index];
            must_match = true;
        }
        loop {
            let Some(node) = path else { return false };
            if matches_scope(&node.name, pattern) {
                break;
            }
            if must_match {
                return false;
            }
            path = node.parent.as_deref();
        }
        path = path.unwrap().parent.as_deref();
        index += 1;
    }
    true
}

fn matches_scope(scope_name: &str, pattern: &str) -> bool {
    pattern == scope_name || (scope_name.starts_with(pattern) && scope_name.as_bytes().get(pattern.len()) == Some(&b'.'))
}

#[derive(Default)]
struct ColorMap {
    colors: Vec<Option<String>>,
    ids: HashMap<String, u32>,
}

impl ColorMap {
    fn id(&mut self, color: Option<&str>) -> u32 {
        let Some(color) = color else { return 0 };
        let color = color.to_uppercase();
        if let Some(&id) = self.ids.get(&color) {
            return id;
        }
        if self.colors.is_empty() {
            self.colors.push(None);
        }
        let id = self.colors.len() as u32;
        self.colors.push(Some(color.clone()));
        self.ids.insert(color, id);
        id
    }
}

fn parse_theme(settings: &[Value]) -> Vec<ParsedRule> {
    let mut result = Vec::new();
    for (index, entry) in settings.iter().enumerate() {
        let Some(entry_settings) = entry.get("settings").filter(|s| truthy(Some(s))) else { continue };
        let scopes: Vec<String> = match entry.get("scope") {
            Some(Value::String(scope)) => {
                let trimmed = scope.trim_start_matches(',').trim_end_matches(',');
                trimmed.split(',').map(str::to_string).collect()
            }
            Some(Value::Array(items)) => items.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect(),
            _ => vec![String::new()],
        };
        let mut font_style = FONT_STYLE_NOT_SET;
        if let Some(Value::String(style)) = entry_settings.get("fontStyle") {
            font_style = 0;
            for segment in style.split(' ') {
                match segment {
                    "italic" => font_style |= 1,
                    "bold" => font_style |= 2,
                    "underline" => font_style |= 4,
                    "strikethrough" => font_style |= 8,
                    _ => {}
                }
            }
        }
        let color = |key: &str| match entry_settings.get(key) {
            Some(Value::String(c)) if is_valid_hex_color(c) => Some(c.clone()),
            _ => None,
        };
        let (foreground, background) = (color("foreground"), color("background"));
        for scope in scopes {
            let trimmed = js_trim(&scope);
            let segments: Vec<&str> = trimmed.split(' ').collect();
            let leaf = segments.last().unwrap().to_string();
            let parent_scopes = if segments.len() > 1 {
                Some(segments[..segments.len() - 1].iter().rev().map(|s| Rc::from(*s)).collect())
            } else {
                None
            };
            result.push(ParsedRule {
                scope: leaf,
                parent_scopes,
                index,
                font_style,
                foreground: foreground.clone(),
                background: background.clone(),
            });
        }
    }
    result
}

/// `String.prototype.trim` (ECMAScript whitespace and line terminators).
fn js_trim(s: &str) -> &str {
    let is_space = |c: char| {
        matches!(c, '\u{9}'..='\u{D}' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
    };
    s.trim_matches(is_space)
}
