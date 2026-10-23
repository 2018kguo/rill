//! Shiki's `codeToTokens` with two themes (`themes: { light, dark }`,
//! `defaultColor: "light"`), as `@streamdown/code` calls it.

use crate::textmate::grammar::{Grammar, Registry, Translation};
use crate::textmate::raw::RawGrammar;
use crate::textmate::theme::{NormalizedTheme, Theme, normalize_theme};
use crate::textmate::tokenizer::{State, THEMES, Tokenizer, font_style, foreground};
use crate::regex::EmulationOptions;
use serde_json::Value;
use std::collections::HashMap;
use std::rc::Rc;

/// Oniguruma sources and their translations (`None`: skipped).
pub type Translations = Vec<(Rc<str>, Option<Translation>)>;

/// One grammar file of the data build (`{ grammar, regexes }`).
pub fn parse_grammar_file(text: &str) -> Result<(RawGrammar, Translations), String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let grammar = RawGrammar::from_json(&value["grammar"])?;
    let mut translations = Vec::new();
    if let Value::Object(regexes) = &value["regexes"] {
        for (source, translation) in regexes {
            let translation = match translation {
                Value::Array(parts) => {
                    let options = &parts[2];
                    let numbers = |v: &Value| v.as_array().map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as u32)).collect::<Vec<_>>()).unwrap_or_default();
                    Some(Translation {
                        pattern: Rc::from(parts[0].as_str().unwrap_or("")),
                        flags: Rc::from(parts[1].as_str().unwrap_or("")),
                        options: EmulationOptions {
                            hidden_captures: numbers(&options["hiddenCaptures"]),
                            transfers: options["transfers"]
                                .as_array()
                                .map(|a| a.iter().map(|t| (t[0].as_u64().unwrap_or(0) as u32, numbers(&t[1]))).collect())
                                .unwrap_or_default(),
                            clip_search: options["strategy"].as_str() == Some("clip_search"),
                        },
                    })
                }
                _ => None,
            };
            translations.push((Rc::from(source.as_str()), translation));
        }
    }
    Ok((grammar, translations))
}

/// The style of a token in one theme: color id and font style.
type ThemeStyle = (u32, u32);

/// A token of the result: UTF-16 offset and length in the code, and an index into
/// the highlighter's style table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub offset: u32,
    pub length: u32,
    pub style: u32,
}

pub struct Tokenized {
    pub lines: Vec<Vec<Token>>,
    /// The grammar state after the last line (`None`: still `INITIAL`).
    pub state: Option<State>,
}

pub struct Highlighter {
    grammar: Grammar,
    themes: [Theme; THEMES],
    normalized: [NormalizedTheme; THEMES],
    style_ids: HashMap<[ThemeStyle; THEMES], u32>,
    /// `htmlStyle` of each style id as (property, value) pairs in Shiki's order.
    pub styles: Vec<Vec<(String, String)>>,
}

impl Highlighter {
    /// A highlighter for `language` with the grammar files the language loads (in
    /// Shiki's load order) and the light and dark theme JSON.
    pub fn new(grammar_files: &[&str], root_scope_or_name: &str, themes: [&str; THEMES]) -> Result<Self, String> {
        let mut registry = Registry::default();
        let mut root_scope = None;
        for file in grammar_files {
            let (grammar, translations) = parse_grammar_file(file)?;
            if &*grammar.name == root_scope_or_name || &*grammar.scope_name == root_scope_or_name {
                root_scope = Some(grammar.scope_name.clone());
            }
            registry.add_grammar(grammar, translations);
        }
        let root_scope = root_scope.ok_or_else(|| format!("no grammar {root_scope_or_name}"))?;
        let grammar = Grammar::new(Rc::new(registry), &root_scope)?;
        let mut normalized = Vec::new();
        for theme in themes {
            let value: Value = serde_json::from_str(theme).map_err(|e| e.to_string())?;
            normalized.push(normalize_theme(&value)?);
        }
        let normalized: [NormalizedTheme; THEMES] = normalized.try_into().map_err(|_| "two themes required")?;
        let themes = [Theme::new(&normalized[0].settings), Theme::new(&normalized[1].settings)];
        Ok(Self { grammar, themes, normalized, style_ids: HashMap::new(), styles: Vec::new() })
    }

    pub fn theme(&self, index: usize) -> &NormalizedTheme {
        &self.normalized[index]
    }

    /// Regex executions so far (for benchmarks).
    pub fn regex_execs(&self) -> u64 {
        self.grammar.execs
    }

    /// Shiki `_tokenizeWithTheme` for both themes, aligned and styled as
    /// `codeToTokensWithThemes` + `flatTokenVariants` produce them.
    pub fn tokenize(&mut self, code: &[u16], state: Option<&State>) -> Result<Tokenized, String> {
        let mut state = state.cloned();
        let mut lines = Vec::new();
        for (start, end) in split_lines(code) {
            let line = &code[start..end];
            if line.is_empty() {
                lines.push(Vec::new());
                continue;
            }
            let mut tokenizer = Tokenizer { grammar: &mut self.grammar, themes: [&self.themes[0], &self.themes[1]] };
            let (line_tokens, next_state, line_length) = tokenizer.tokenize_line(line, state.as_ref())?;
            // Per theme: tokens as (start, end, style), dropping empty ones.
            let mut per_theme: [Vec<(usize, usize, ThemeStyle)>; THEMES] = Default::default();
            for theme in 0..THEMES {
                let binary = line_tokens.binary_result(theme, &next_state, line_length);
                for (j, &(token_start, metadata)) in binary.iter().enumerate() {
                    let token_start = token_start as usize;
                    let token_end = binary.get(j + 1).map_or(line.len(), |&(s, _)| s as usize);
                    if token_start == token_end {
                        continue;
                    }
                    per_theme[theme].push((token_start, token_end, (foreground(metadata), font_style(metadata))));
                }
            }
            lines.push(self.align(&per_theme, start as u32));
            state = Some(next_state);
        }
        Ok(Tokenized { lines, state })
    }

    /// `alignThemesTokenization`: splits both themes' tokens at the union of their
    /// boundaries.
    fn align(&mut self, per_theme: &[Vec<(usize, usize, ThemeStyle)>; THEMES], line_offset: u32) -> Vec<Token> {
        let mut out = Vec::with_capacity(per_theme[0].len().max(per_theme[1].len()));
        let (a, b) = (&per_theme[0], &per_theme[1]);
        let (mut i, mut j) = (0, 0);
        let mut position = a.first().map_or(0, |t| t.0);
        while i < a.len() && j < b.len() {
            let end = a[i].1.min(b[j].1);
            let style = self.style_id([a[i].2, b[j].2]);
            out.push(Token { offset: line_offset + position as u32, length: (end - position) as u32, style });
            position = end;
            if a[i].1 == end {
                i += 1;
            }
            if b[j].1 == end {
                j += 1;
            }
        }
        out
    }

    fn style_id(&mut self, styles: [ThemeStyle; THEMES]) -> u32 {
        if let Some(&id) = self.style_ids.get(&styles) {
            return id;
        }
        let id = self.styles.len() as u32;
        let html = self.html_style(styles);
        self.styles.push(html);
        self.style_ids.insert(styles, id);
        id
    }

    /// `flatTokenVariants` with variants ["light", "dark"] and `defaultColor: "light"`.
    fn html_style(&self, styles: [ThemeStyle; THEMES]) -> Vec<(String, String)> {
        let objects: Vec<Vec<(&'static str, String)>> = styles
            .iter()
            .enumerate()
            .map(|(theme, &(color_id, font))| {
                let mut object = Vec::new();
                if let Some(color) = self.color(theme, color_id) {
                    object.push(("color", color));
                }
                if font != 0 {
                    if font & 1 != 0 {
                        object.push(("font-style", "italic".to_string()));
                    }
                    if font & 2 != 0 {
                        object.push(("font-weight", "bold".to_string()));
                    }
                    let mut decorations = Vec::new();
                    if font & 4 != 0 {
                        decorations.push("underline");
                    }
                    if font & 8 != 0 {
                        decorations.push("line-through");
                    }
                    if !decorations.is_empty() {
                        object.push(("text-decoration", decorations.join(" ")));
                    }
                }
                object
            })
            .collect();
        let mut keys: Vec<&'static str> = Vec::new();
        for object in &objects {
            for (key, _) in object {
                if !keys.contains(key) {
                    keys.push(key);
                }
            }
        }
        let variants = ["light", "dark"];
        let mut merged = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            for key in &keys {
                let value = object.iter().find(|(k, _)| k == key).map_or_else(|| "inherit".to_string(), |(_, v)| v.clone());
                if index == 0 && (*key == "color" || *key == "background-color") {
                    merged.push((key.to_string(), value));
                } else {
                    let suffix = match *key {
                        "color" => String::new(),
                        "background-color" => "-bg".to_string(),
                        other => format!("-{other}"),
                    };
                    merged.push((format!("--shiki-{}{suffix}", variants[index]), value));
                }
            }
        }
        merged
    }

    /// `applyColorReplacements(colorMap[id], theme.colorReplacements)`.
    fn color(&self, theme: usize, color_id: u32) -> Option<String> {
        let color = self.themes[theme].color_map.get(color_id as usize).cloned().flatten()?;
        if color.is_empty() {
            return None;
        }
        Some(self.normalized[theme].color_replacements.get(&color.to_lowercase()).cloned().unwrap_or(color))
    }
}

/// Shiki `splitLines`: line ranges split at `\r?\n`.
pub fn split_lines(code: &[u16]) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < code.len() {
        if code[i] == 0x0A {
            let end = if i > start && code[i - 1] == 0x0D { i - 1 } else { i };
            lines.push((start, end));
            start = i + 1;
        }
        i += 1;
    }
    lines.push((start, code.len()));
    lines
}
