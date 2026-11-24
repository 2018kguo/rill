//! Citation markers ([`crate::tokens::Options::citations`]): each `[n]` in the text
//! becomes `<span data-citation-id="n" data-citation-markers-before="k"
//! class="citation-marker"></span>`, `k` counting the `[n]` before it in the whole text,
//! as a pass over a rendering's text tokens (the HAST's text nodes) after it is written.

use crate::tokens::{ISLAND, NOWHERE, Out, TAGS, kind, overrides, tag};
use crate::{Fallback, Result};

/// `/\[(\d+)\]/g` over `units`: each (start, end).
fn matches(units: &[u16], mut each: impl FnMut(usize, usize)) {
    let mut at = 0;
    while let Some(found) = units[at..].iter().position(|&unit| unit == u16::from(b'[')) {
        let open = at + found;
        let mut end = open + 1;
        while end < units.len() && (0x30..=0x39).contains(&units[end]) {
            end += 1;
        }
        if end > open + 1 && units.get(end) == Some(&u16::from(b']')) {
            each(open, end + 1);
            at = end + 1;
        } else {
            at = open + 1;
        }
    }
}

/// Splits `out`'s text tokens at their markers; `source` is the whole text the tokens'
/// origins count in. Returns how many words it inserted before word `split`.
/// ponytail: counts the markers before each one by scanning the whole text each render
/// (a sorted list kept across renders if long messages show it).
pub(crate) fn split(out: &mut Out, source: &[u16], split: usize) -> Result<usize> {
    // Most renderings have none.
    let mut literal = 0;
    let any = out.words.iter().zip(&out.origins).any(|(&word, &origin)| {
        if word & 3 != kind::TEXT {
            return false;
        }
        let length = (word >> 2) as usize;
        let units = if origin == NOWHERE {
            literal += length;
            &out.text[literal - length..literal]
        } else {
            &source[origin as usize..origin as usize + length]
        };
        units.contains(&u16::from(b'['))
    });
    if !any {
        return Ok(0);
    }
    // Each marker's position in the whole text, by its number.
    let mut positions: std::collections::HashMap<&[u16], Vec<usize>> = Default::default();
    matches(source, |open, close| positions.entry(&source[open + 1..close - 1]).or_default().push(open));
    let mut words = Vec::with_capacity(out.words.len() + 8);
    let mut origins = Vec::with_capacity(out.words.len() + 8);
    let mut inserted = 0;
    // Open elements: whether each is in a code block (its text is code, not text).
    let mut code = Vec::new();
    let mut literal = 0;
    // The text token before ends in what may start a marker (`[`, digits).
    let mut pending = false;
    let digit = |unit: u16| (0x30..=0x39).contains(&unit);
    for (index, (&word, &origin)) in out.words.iter().zip(&out.origins).enumerate() {
        if index == split {
            inserted = words.len() - split;
        }
        if word & 3 != kind::TEXT {
            pending = false;
            match word & 3 {
                kind::OPEN => {
                    let id = word >> 2;
                    let strings = out.strings.borrow();
                    let html = if id >= tag::INTERNED { strings.get(id) } else { TAGS[id as usize] };
                    let island_code = html.strip_prefix(ISLAND).is_some_and(|html| html.starts_with("<code"));
                    code.push(code.last() == Some(&true) || html.starts_with("<pre") || island_code);
                }
                kind::CLOSE => {
                    code.pop();
                }
                _ => {}
            }
            words.push(word);
            origins.push(origin);
            continue;
        }
        let length = (word >> 2) as usize;
        let units = if origin == NOWHERE {
            literal += length;
            &out.text[literal - length..literal]
        } else {
            &source[origin as usize..origin as usize + length]
        };
        // A marker across text tokens (one HAST text node): ponytail, the exact path.
        if pending && units.first().is_some_and(|&unit| digit(unit) || unit == u16::from(b']')) {
            return Err(Fallback);
        }
        let tail = units.iter().rev().take_while(|&&unit| digit(unit)).count();
        pending = units.len() > tail && units[units.len() - tail - 1] == u16::from(b'[');
        let mut found = Vec::new();
        matches(units, |open, close| found.push((open, close)));
        if found.is_empty() {
            words.push(word);
            origins.push(origin);
            continue;
        }
        // ponytail: markers in a code block, or in text that is not the source's.
        if origin == NOWHERE || code.last() == Some(&true) {
            return Err(Fallback);
        }
        let start = origin as usize;
        let mut last = 0;
        for (open, close) in found {
            if open > last {
                words.push(kind::TEXT | ((open - last) as u32) << 2);
                origins.push((start + last) as u32);
            }
            let number = &units[open + 1..close - 1];
            let before = positions[number].partition_point(|&at| at < start + open);
            let island = if out.options.overrides & overrides::SPAN != 0 { "\u{1}" } else { "" };
            let html = format!(
                r#"{island}<span data-citation-id="{}" data-citation-markers-before="{before}" class="citation-marker">"#,
                String::from_utf16_lossy(number)
            );
            let id = out.strings.borrow_mut().id(&html);
            words.push(kind::OPEN | id << 2);
            origins.push(NOWHERE);
            words.push(kind::CLOSE);
            origins.push(NOWHERE);
            last = close;
        }
        if last < units.len() {
            words.push(kind::TEXT | ((units.len() - last) as u32) << 2);
            origins.push((start + last) as u32);
        }
    }
    if split >= out.words.len() {
        inserted = words.len() - out.words.len();
    }
    out.words = words;
    out.origins = origins;
    Ok(inserted)
}
