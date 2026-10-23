//! An ECMAScript regex engine for the patterns oniguruma-to-es produces (`u`/`v`
//! mode, ES2025 syntax), with `EmulatedRegExp`'s extensions: hidden captures,
//! capture transfers and the `clip_search` strategy for `\G`.

mod charset;
mod compile;
mod parse;
mod unicode_tables;
mod vm;

pub use unicode_tables::UNICODE_VERSION;
pub use vm::Matcher;

/// A compiled regex as `toRegExp` from oniguruma-to-es would construct it.
#[derive(Debug, Clone)]
pub struct JsRegExp {
    program: compile::Program,
    sticky: bool,
    clip_search: bool,
    /// For each group number, whether the group is hidden.
    hidden: Vec<bool>,
    /// For each group number, the group its value is transferred to.
    transfer_to: Vec<u32>,
    has_capture_map: bool,
}

/// Options of an `EmulatedRegExp` (oniguruma-to-es `rawOptions`).
#[derive(Debug, Clone, Default)]
pub struct EmulationOptions {
    pub hidden_captures: Vec<u32>,
    pub transfers: Vec<(u32, Vec<u32>)>,
    pub clip_search: bool,
}

impl JsRegExp {
    pub fn new(pattern: &str, flags: &str, options: &EmulationOptions) -> Result<Self, String> {
        Self::build(parse::parse(pattern, flags)?, flags, options)
    }

    /// Compiles a back-reference template (see `parse::parse_with_placeholders`).
    pub fn with_placeholders(pattern: &str, flags: &str, options: &EmulationOptions, values: &[Vec<u16>]) -> Result<Self, String> {
        Self::build(parse::parse_with_placeholders(pattern, flags, Some(values))?, flags, options)
    }

    fn build(parsed: parse::Parsed, flags: &str, options: &EmulationOptions) -> Result<Self, String> {
        let program = compile::compile(parsed.node, parsed.group_count);
        let groups = parsed.group_count as usize + 1;
        let mut hidden = vec![false; groups];
        let mut transfer_to = vec![0; groups];
        for &group in &options.hidden_captures {
            *hidden.get_mut(group as usize).ok_or("hidden capture out of range")? = true;
        }
        for (to, from) in &options.transfers {
            for &group in from {
                *transfer_to.get_mut(group as usize).ok_or("transfer out of range")? = *to;
            }
        }
        Ok(Self {
            program,
            sticky: flags.contains('y'),
            clip_search: options.clip_search,
            hidden,
            transfer_to,
            has_capture_map: !options.hidden_captures.is_empty() || !options.transfers.is_empty(),
        })
    }

    /// Whether a search result can be reused for later start positions on the same
    /// string (Shiki's `isSearchCacheable`).
    pub fn is_search_cacheable(&self) -> bool {
        !self.sticky && !self.clip_search
    }

    /// `exec` with `lastIndex = start`. On a match, `result` holds start/end pairs
    /// for group 0 and each visible group (-1 for groups that did not participate).
    pub fn exec(&self, matcher: &mut Matcher, input: &[u16], start: usize, raw: &mut Vec<i32>, result: &mut Vec<i32>) -> bool {
        self.exec_bounded(matcher, input, start, usize::MAX, raw, result)
    }

    /// `exec`, but only looking for matches that start at or before `limit`: false
    /// means no match starts in `start..=limit`.
    pub fn exec_bounded(&self, matcher: &mut Matcher, input: &[u16], start: usize, limit: usize, raw: &mut Vec<i32>, result: &mut Vec<i32>) -> bool {
        let offset = if self.clip_search && start > 0 { start.min(input.len()) } else { 0 };
        let found = if offset > 0 {
            matcher.exec(&self.program, &input[offset..], 0, limit.saturating_sub(offset), self.sticky, raw)
        } else {
            matcher.exec(&self.program, input, start, limit, self.sticky, raw)
        };
        if !found {
            return false;
        }
        if offset > 0 {
            for value in raw.iter_mut() {
                if *value >= 0 {
                    *value += offset as i32;
                }
            }
        }
        result.clear();
        if !self.has_capture_map {
            result.extend_from_slice(raw);
            return true;
        }
        // EmulatedRegExp#execCore: drop hidden groups, then apply transfers.
        result.extend_from_slice(&raw[0..2]);
        let mut mapped: Vec<Option<usize>> = vec![Some(0)];
        for group in 1..self.hidden.len() {
            if self.hidden[group] {
                mapped.push(None);
            } else {
                mapped.push(Some(result.len() / 2));
                result.extend_from_slice(&raw[group * 2..group * 2 + 2]);
            }
            let to = self.transfer_to[group] as usize;
            if to != 0 && raw[group * 2] >= 0 {
                match mapped.get(to).copied() {
                    Some(Some(target)) if target != 0 => {
                        result[target * 2] = raw[group * 2];
                        result[target * 2 + 1] = raw[group * 2 + 1];
                    }
                    // EmulatedRegExp throws; the forgiving scanner skips the pattern.
                    _ => return false,
                }
            }
        }
        true
    }
}
