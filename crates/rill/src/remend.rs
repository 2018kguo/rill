//! remend 1.4.0 (Streamdown's healing of incomplete streaming Markdown) with its
//! default options, ported from packages/remend/src (Apache-2.0, Copyright 2023
//! Vercel, Inc.). It works on UTF-16 code units, as the JavaScript does, so every
//! index, slice and comparison is the same; `conformance/remend.mjs` checks the output
//! against the package.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

type Units = Vec<u16>;

const NEWLINE: u16 = b'\n' as u16;
const CR: u16 = b'\r' as u16;
const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;
const STAR: u16 = b'*' as u16;
const UNDERSCORE: u16 = b'_' as u16;
const TILDE: u16 = b'~' as u16;
const BACKTICK: u16 = b'`' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const DOLLAR: u16 = b'$' as u16;
const OPEN_BRACKET: u16 = b'[' as u16;
const CLOSE_BRACKET: u16 = b']' as u16;
const OPEN_PAREN: u16 = b'(' as u16;
const CLOSE_PAREN: u16 = b')' as u16;
const BANG: u16 = b'!' as u16;
const LESS: u16 = b'<' as u16;
const GREATER: u16 = b'>' as u16;
const EQUALS: u16 = b'=' as u16;
const DASH: u16 = b'-' as u16;
const PLUS: u16 = b'+' as u16;
const DOT: u16 = b'.' as u16;
const SLASH: u16 = b'/' as u16;

const INCOMPLETE_LINK: &str = "](streamdown:incomplete-link)";
const INCOMPLETE_IMAGE: &str = "](streamdown:incomplete-image)";

/// remend with its default options: the text Streamdown renders while `children`
/// streams.
pub fn remend(text: &str) -> String {
    let units: Units = text.encode_utf16().collect();
    String::from_utf16_lossy(&remend_units(units))
}

/// A healed text as an edit of its input: the input's first `prefix` units, then
/// `middle`, then its last `suffix` units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Healing {
    pub prefix: usize,
    pub suffix: usize,
    pub middle: String,
}

/// remend over a text that streams: it keeps the text, takes what was appended, and
/// answers with the edit, so only the changes cross a language boundary.
#[derive(Debug, Default)]
pub struct Stream {
    input: Units,
}

impl Stream {
    /// Heals the text so far plus `appended` (or, with `reset`, just `appended`).
    pub fn heal(&mut self, appended: &str, reset: bool) -> Healing {
        if reset {
            self.input.clear();
        }
        self.input.extend(appended.encode_utf16());
        let healed = remend_units(self.input.clone());
        let input = &self.input;
        let prefix = input
            .iter()
            .zip(&healed)
            .take_while(|(left, right)| left == right)
            .count();
        let room = input.len().min(healed.len()) - prefix;
        let suffix = input
            .iter()
            .rev()
            .zip(healed.iter().rev())
            .take(room)
            .take_while(|(left, right)| left == right)
            .count();
        Healing {
            prefix,
            suffix,
            middle: String::from_utf16_lossy(&healed[prefix..healed.len() - suffix]),
        }
    }
}

/// [`remend`] on UTF-16 code units.
pub fn remend_units(text: Units) -> Units {
    if text.is_empty() {
        return text;
    }
    let mut result = strip_single_trailing_space(text);
    let handlers: [fn(Units) -> Units; 13] = [
        single_tilde,
        comparison_operators,
        incomplete_html_tag,
        incomplete_setext_heading,
        incomplete_links_and_images,
        incomplete_bold_italic,
        incomplete_bold,
        incomplete_double_underscore_italic,
        incomplete_single_asterisk_italic,
        incomplete_single_underscore_italic,
        incomplete_inline_code,
        incomplete_strikethrough,
        incomplete_block_katex,
    ];
    for (index, handler) in handlers.into_iter().enumerate() {
        result = handler(result);
        // The links handler (index 4) ends healing once it closed a link or image.
        if index == 4
            && (ends_with(&result, INCOMPLETE_LINK) || ends_with(&result, INCOMPLETE_IMAGE))
        {
            LAST_SCAN.set(None);
            return result;
        }
    }
    // `clearScanCache`: no text stays alive between calls.
    LAST_SCAN.set(None);
    strip_single_trailing_space(result)
}

fn strip_single_trailing_space(mut text: Units) -> Units {
    if text.last() == Some(&SPACE) && !(text.len() >= 2 && text[text.len() - 2] == SPACE) {
        text.pop();
    }
    text
}

// -------------------------------------------------------------------------------------
// String helpers with JavaScript's semantics
// -------------------------------------------------------------------------------------

fn units(value: &str) -> Units {
    value.encode_utf16().collect()
}

/// `text[index]`, `undefined` outside the string.
fn at(text: &[u16], index: isize) -> Option<u16> {
    usize::try_from(index)
        .ok()
        .and_then(|index| text.get(index).copied())
}

fn ends_with(text: &[u16], suffix: &str) -> bool {
    let suffix = units(suffix);
    text.ends_with(&suffix)
}

/// `text.indexOf(pattern, from)`.
fn index_of(text: &[u16], pattern: &[u16], from: usize) -> Option<usize> {
    let (&first, rest) = pattern.split_first()?;
    if pattern.len() > text.len() {
        return None;
    }
    let last_start = text.len() - pattern.len();
    let mut at = from;
    while at <= last_start {
        let found = find_unit(&text[at..=last_start], first)? + at;
        if text[found + 1..found + pattern.len()] == *rest {
            return Some(found);
        }
        at = found + 1;
    }
    None
}

fn index_of_unit(text: &[u16], unit: u16, from: usize) -> Option<usize> {
    find_unit(text.get(from..)?, unit).map(|offset| offset + from)
}

/// `text` as its in-memory bytes, for memchr: an ASCII unit `u` is the byte pair
/// `[u, 0]` at an even offset on little-endian targets.
#[cfg(target_endian = "little")]
fn unit_bytes(text: &[u16]) -> &[u8] {
    // SAFETY: a `u16` slice is `2 * len` initialized bytes, and `u8` has no alignment.
    unsafe { std::slice::from_raw_parts(text.as_ptr().cast::<u8>(), text.len() * 2) }
}

/// The first index of `unit` in `text`.
fn find_unit(text: &[u16], unit: u16) -> Option<usize> {
    #[cfg(target_endian = "little")]
    if unit < 0x80 {
        let bytes = unit_bytes(text);
        let mut at = 0;
        while let Some(offset) = memchr::memchr(unit as u8, &bytes[at..]) {
            let index = at + offset;
            if index % 2 == 0 && bytes[index + 1] == 0 {
                return Some(index / 2);
            }
            at = index + 1;
        }
        return None;
    }
    text.iter().position(|&candidate| candidate == unit)
}

/// The first index of the ASCII unit `first` or `second` in `text`.
fn find_unit2(text: &[u16], first: u16, second: u16) -> Option<usize> {
    #[cfg(target_endian = "little")]
    {
        let bytes = unit_bytes(text);
        memchr::memchr2_iter(first as u8, second as u8, bytes)
            .find(|&index| index % 2 == 0 && bytes[index + 1] == 0)
            .map(|index| index / 2)
    }
    #[cfg(not(target_endian = "little"))]
    text.iter()
        .position(|&unit| unit == first || unit == second)
}

/// Calls `visit` with each index of the ASCII units `first` or `second`, in order.
fn for_each_unit2(text: &[u16], first: u16, second: u16, mut visit: impl FnMut(usize)) {
    #[cfg(target_endian = "little")]
    {
        let bytes = unit_bytes(text);
        for index in memchr::memchr2_iter(first as u8, second as u8, bytes) {
            if index % 2 == 0 && bytes[index + 1] == 0 {
                visit(index / 2);
            }
        }
    }
    #[cfg(not(target_endian = "little"))]
    for (index, &unit) in text.iter().enumerate() {
        if unit == first || unit == second {
            visit(index);
        }
    }
}

/// The last index of `unit` in `text`.
fn rfind_unit(text: &[u16], unit: u16) -> Option<usize> {
    #[cfg(target_endian = "little")]
    if unit < 0x80 {
        let bytes = unit_bytes(text);
        let mut end = bytes.len();
        while let Some(index) = memchr::memrchr(unit as u8, &bytes[..end]) {
            if index % 2 == 0 && bytes[index + 1] == 0 {
                return Some(index / 2);
            }
            end = index;
        }
        return None;
    }
    text.iter().rposition(|&candidate| candidate == unit)
}

/// `text.lastIndexOf(pattern, from)`, `from` defaulting to the end.
fn last_index_of(text: &[u16], pattern: &[u16], from: Option<isize>) -> Option<usize> {
    let (&first, rest) = pattern.split_first()?;
    if pattern.len() > text.len() {
        return None;
    }
    let limit = text.len() - pattern.len();
    let mut end = match from {
        Some(from) => usize::try_from(from.max(0)).unwrap_or(0).min(limit),
        None => limit,
    } + 1;
    while end > 0 {
        let found = rfind_unit(&text[..end], first)?;
        if text[found + 1..found + pattern.len()] == *rest {
            return Some(found);
        }
        end = found;
    }
    None
}

/// JavaScript's `\s` (and what `trim` removes).
fn js_space(unit: u16) -> bool {
    matches!(
        unit,
        0x09..=0x0d
            | 0x20
            | 0xa0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
            | 0xfeff
    )
}

fn line_terminator(unit: u16) -> bool {
    matches!(unit, 0x0a | 0x0d | 0x2028 | 0x2029)
}

fn trim(text: &[u16]) -> &[u16] {
    trim_end(trim_start(text))
}

fn trim_start(text: &[u16]) -> &[u16] {
    let start = text
        .iter()
        .position(|&unit| !js_space(unit))
        .unwrap_or(text.len());
    &text[start..]
}

fn trim_end(text: &[u16]) -> &[u16] {
    let end = text
        .iter()
        .rposition(|&unit| !js_space(unit))
        .map_or(0, |index| index + 1);
    &text[..end]
}

fn concat(parts: &[&[u16]]) -> Units {
    let mut out = Vec::with_capacity(parts.iter().map(|part| part.len()).sum());
    for part in parts {
        out.extend_from_slice(part);
    }
    out
}

fn appended(mut text: Units, suffix: &str) -> Units {
    text.extend(suffix.encode_utf16());
    text
}

fn letter_or_number(code_point: u32) -> bool {
    char::from_u32(code_point).is_some_and(rill_cmark::letter_or_number)
}

/// `isWordChar(text[index])`: one code unit (a lone half of a pair is neither).
fn word_unit(unit: Option<u16>) -> bool {
    match unit {
        None => false,
        Some(unit) if unit < 0x80 => unit == UNDERSCORE || (unit as u8).is_ascii_alphanumeric(),
        Some(unit) => letter_or_number(u32::from(unit)),
    }
}

/// The code point at `index` and its length in units (`/u` regular expressions).
fn code_point_at(text: &[u16], index: usize) -> (u32, usize) {
    let unit = text[index];
    if (0xd800..0xdc00).contains(&unit)
        && let Some(&low) = text.get(index + 1)
        && (0xdc00..0xe000).contains(&low)
    {
        let code_point = 0x10000 + ((u32::from(unit) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
        return (code_point, 2);
    }
    (u32::from(unit), 1)
}

/// `/[\p{L}\p{N}_]/u` at a code point.
fn word_code_point(code_point: u32) -> bool {
    code_point == u32::from(UNDERSCORE) || letter_or_number(code_point)
}

/// `/^[\s_~*`]*$/`.
fn whitespace_or_markers(text: &[u16]) -> bool {
    text.iter()
        .all(|&unit| js_space(unit) || matches!(unit, UNDERSCORE | TILDE | STAR | BACKTICK))
}

/// `/^[\s]*[-*+][\s]+$/`.
fn list_item_prefix(text: &[u16]) -> bool {
    let rest = trim_start(text);
    match rest.split_first() {
        Some((&(DASH | STAR | PLUS), after)) => {
            !after.is_empty() && after.iter().all(|&unit| js_space(unit))
        }
        _ => false,
    }
}

// -------------------------------------------------------------------------------------
// scan.ts: code and prose regions
// -------------------------------------------------------------------------------------

const PROSE: u8 = 0;
const FENCE_MARKER: u8 = 1;
const FENCE_INFO: u8 = 2;
const FENCE_BODY: u8 = 3;
const CODE_SPAN: u8 = 4;
const CODE_SPAN_OPEN: u8 = 5;

#[derive(Clone, Copy)]
struct OpenFence {
    marker: u16,
    length: usize,
    list_indent: usize,
    quote_depth: usize,
}

#[derive(Clone, Copy)]
struct OpenSpan {
    run_length: usize,
    start: usize,
}

/// A text's regions (`TextScan` without the text).
struct ScanData {
    regions: Vec<u8>,
    open_fence: Option<OpenFence>,
    open_span: Option<OpenSpan>,
    math: OnceCell<Vec<u8>>,
    has_link: OnceCell<bool>,
    has_less: OnceCell<bool>,
}

struct Scan<'t> {
    text: &'t [u16],
    data: Rc<ScanData>,
}

impl std::ops::Deref for Scan<'_> {
    type Target = ScanData;

    fn deref(&self) -> &ScanData {
        &self.data
    }
}

thread_local! {
    /// scan.ts `getScan`'s single-entry memo: handlers mostly pass the text on as they
    /// got it, so one scan serves several of them.
    static LAST_SCAN: RefCell<Option<(Units, Rc<ScanData>)>> = const { RefCell::new(None) };
}

fn digit(unit: Option<u16>) -> bool {
    unit.is_some_and(|unit| (u16::from(b'0')..=u16::from(b'9')).contains(&unit))
}

/// Index just past a list marker and its space at `start`, if one is there.
fn skip_list_marker(text: &[u16], start: usize) -> Option<usize> {
    let mut i = start;
    let first = text.get(i).copied();
    if matches!(first, Some(DASH | STAR | PLUS)) {
        i += 1;
    } else {
        while digit(text.get(i).copied()) && i - start < 9 {
            i += 1;
        }
        if i == start || !matches!(text.get(i).copied(), Some(DOT | CLOSE_PAREN)) {
            return None;
        }
        i += 1;
    }
    (text.get(i) == Some(&SPACE)).then_some(i + 1)
}

struct ContainerPrefix {
    content_start: usize,
    list_indent: usize,
    quote_depth: usize,
}

fn skip_container_prefix(text: &[u16], line_start: usize, line_end: usize) -> ContainerPrefix {
    let mut quote_depth = 0;
    let mut after_quotes = line_start;
    let mut starts_item = false;
    let mut i = line_start;
    loop {
        while i < line_end && text.get(i) == Some(&SPACE) {
            i += 1;
        }
        if text.get(i) == Some(&GREATER) {
            quote_depth += 1;
            i += if text.get(i + 1) == Some(&SPACE) {
                2
            } else {
                1
            };
            after_quotes = i;
            starts_item = false;
            continue;
        }
        let after_marker = if i < line_end {
            skip_list_marker(text, i)
        } else {
            None
        };
        let Some(after_marker) = after_marker else {
            return ContainerPrefix {
                content_start: i,
                list_indent: if starts_item { i - after_quotes } else { 0 },
                quote_depth,
            };
        };
        i = after_marker;
        starts_item = true;
    }
}

fn skip_quote_markers(
    text: &[u16],
    line_start: usize,
    line_end: usize,
    depth: usize,
) -> Option<usize> {
    let mut i = line_start;
    for _ in 0..depth {
        while i < line_end && text.get(i) == Some(&SPACE) {
            i += 1;
        }
        if text.get(i) != Some(&GREATER) {
            return None;
        }
        i += 1;
        if text.get(i) == Some(&SPACE) {
            i += 1;
        }
    }
    Some(i)
}

fn fill(regions: &mut [u8], value: u8, start: usize, end: usize) {
    let end = end.min(regions.len());
    if start < end {
        regions[start..end].fill(value);
    }
}

fn is_fence_closer(text: &[u16], content_start: usize, line_end: usize, fence: OpenFence) -> bool {
    let mut i = content_start;
    while i < line_end && text[i] == SPACE {
        i += 1;
    }
    let mut run = 0;
    while i < line_end && text[i] == fence.marker {
        i += 1;
        run += 1;
    }
    if run < fence.length {
        return false;
    }
    text.get(i..line_end)
        .unwrap_or(&[])
        .iter()
        .all(|&unit| matches!(unit, SPACE | TAB | CR))
}

fn open_fence_at(
    text: &[u16],
    regions: &mut [u8],
    line_start: usize,
    line_end: usize,
) -> Option<OpenFence> {
    let content_end = if line_end > line_start && text[line_end - 1] == CR {
        line_end - 1
    } else {
        line_end
    };
    let prefix = skip_container_prefix(text, line_start, content_end);
    let content = text.get(prefix.content_start..content_end).unwrap_or(&[]);
    // `/^(`{3,}|~{3,})(.*)$/`
    let marker = *content.first()?;
    if marker != BACKTICK && marker != TILDE {
        return None;
    }
    let run = content.iter().take_while(|&&unit| unit == marker).count();
    if run < 3 {
        return None;
    }
    let info = &content[run..];
    if info.iter().any(|&unit| line_terminator(unit)) {
        return None;
    }
    if marker == BACKTICK && info.contains(&BACKTICK) {
        return None;
    }
    let marker_start = prefix.content_start;
    fill(regions, FENCE_MARKER, marker_start, marker_start + run);
    fill(regions, FENCE_INFO, marker_start + run, line_end + 1);
    Some(OpenFence {
        marker,
        length: run,
        list_indent: prefix.list_indent,
        quote_depth: prefix.quote_depth,
    })
}

fn fence_content_start(
    text: &[u16],
    line_start: usize,
    line_end: usize,
    fence: OpenFence,
) -> Option<usize> {
    let content_start = skip_quote_markers(text, line_start, line_end, fence.quote_depth)?;
    if fence.list_indent == 0 {
        return Some(content_start);
    }
    let mut i = content_start;
    while i < line_end && text[i] == SPACE {
        i += 1;
    }
    let blank = i == line_end || (text.get(i) == Some(&CR) && i + 1 == line_end);
    (blank || i - content_start >= fence.list_indent).then_some(content_start)
}

fn paint_fences(text: &[u16], regions: &mut [u8]) -> Option<OpenFence> {
    let n = text.len();
    let mut open: Option<OpenFence> = None;
    let mut line_start = 0;
    while line_start < n {
        let line_end = index_of_unit(text, NEWLINE, line_start).unwrap_or(n);
        let content_start =
            open.and_then(|fence| fence_content_start(text, line_start, line_end, fence));
        if open.is_some() && content_start.is_none() {
            open = None;
        }
        match (open, content_start) {
            (None, _) => open = open_fence_at(text, regions, line_start, line_end),
            (Some(fence), Some(content_start))
                if is_fence_closer(text, content_start, line_end, fence) =>
            {
                fill(regions, FENCE_MARKER, line_start, line_end);
                open = None;
            }
            _ => fill(regions, FENCE_BODY, line_start, line_end + 1),
        }
        line_start = line_end + 1;
    }
    open
}

fn backtick_run_end(text: &[u16], start: usize) -> usize {
    let mut end = start + 1;
    while end < text.len() && text[end] == BACKTICK {
        end += 1;
    }
    end
}

fn paragraph_break_at(text: &[u16], newline: usize) -> bool {
    let mut j = newline + 1;
    while j < text.len() && matches!(text[j], SPACE | TAB | CR) {
        j += 1;
    }
    j < text.len() && text[j] == NEWLINE
}

fn next_span_opener(text: &[u16], regions: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    loop {
        let next = index_of_unit(text, BACKTICK, i)?;
        let escaped = next > i && text[next - 1] == BACKSLASH && regions[next - 1] == PROSE;
        if regions[next] == PROSE && !escaped {
            return Some(next);
        }
        i = next + 1;
    }
}

fn paint_spans(text: &[u16], regions: &mut [u8]) -> Option<OpenSpan> {
    let n = text.len();
    let mut span: Option<OpenSpan> = None;
    let mut i = 0;
    while i < n {
        let Some(open) = span else {
            let Some(next) = next_span_opener(text, regions, i) else {
                break;
            };
            let end = backtick_run_end(text, next);
            span = Some(OpenSpan {
                start: next,
                run_length: end - next,
            });
            i = end;
            continue;
        };
        if regions[i] != PROSE {
            fill(regions, CODE_SPAN_OPEN, open.start, i);
            span = None;
            i += 1;
            continue;
        }
        if text[i] == NEWLINE && paragraph_break_at(text, i) {
            span = None;
            i += 1;
            continue;
        }
        if text[i] != BACKTICK {
            i += 1;
            continue;
        }
        let end = backtick_run_end(text, i);
        if end - i == open.run_length {
            fill(regions, CODE_SPAN, open.start, end);
            span = None;
        }
        i = end;
    }
    if let Some(open) = span {
        fill(regions, CODE_SPAN_OPEN, open.start, n);
        return Some(open);
    }
    None
}

impl<'t> Scan<'t> {
    fn new(text: &'t [u16]) -> Self {
        let cached = LAST_SCAN.with_borrow(|last| {
            last.as_ref()
                .filter(|(units, _)| units.as_slice() == text)
                .map(|(_, data)| Rc::clone(data))
        });
        if let Some(data) = cached {
            return Scan { text, data };
        }
        let mut regions = vec![PROSE; text.len()];
        let open_fence = paint_fences(text, &mut regions);
        let open_span = paint_spans(text, &mut regions);
        let data = Rc::new(ScanData {
            regions,
            open_fence,
            open_span,
            math: OnceCell::new(),
            has_link: OnceCell::new(),
            has_less: OnceCell::new(),
        });
        LAST_SCAN.set(Some((text.to_vec(), Rc::clone(&data))));
        Scan { text, data }
    }

    fn region(&self, position: isize) -> Option<u8> {
        usize::try_from(position)
            .ok()
            .and_then(|position| self.regions.get(position).copied())
    }

    /// `isCodeAt`: a fence or inline span.
    fn code_at(&self, position: isize) -> bool {
        if position >= self.regions.len() as isize {
            return self.open_fence.is_some() || self.open_span.is_some();
        }
        if position < 0 {
            return false;
        }
        self.regions[position as usize] != PROSE
    }

    fn complete_span_at(&self, position: isize) -> bool {
        self.region(position) == Some(CODE_SPAN)
    }

    fn mask_at(mask: &[u8], text: &[u16], position: isize) -> bool {
        if position < 0 || position >= text.len() as isize {
            return false;
        }
        mask.get(position as usize) == Some(&1)
    }

    fn in_math(&self, position: isize) -> bool {
        let mask = self.math.get_or_init(|| {
            let text = self.text;
            let has = text.contains(&DOLLAR)
                || text.windows(2).any(|pair| {
                    pair[0] == BACKSLASH && matches!(pair[1], OPEN_PAREN | OPEN_BRACKET)
                });
            if has { self.math_mask() } else { Vec::new() }
        });
        Self::mask_at(mask, self.text, position)
    }

    /// The link URL mask at `position` (`inLinkUrlAt`), replaying the position's line:
    /// inside a `](` URL with a `)` still to come on the line.
    fn in_link_url(&self, position: isize) -> bool {
        let text = self.text;
        let Ok(position) = usize::try_from(position) else {
            return false;
        };
        if position >= text.len()
            || text[position] == NEWLINE
            || !*self
                .has_link
                .get_or_init(|| index_of(text, &[CLOSE_BRACKET, OPEN_PAREN], 0).is_some())
        {
            return false;
        }
        let line_start = rfind_unit(&text[..position], NEWLINE).map_or(0, |index| index + 1);
        let line_end = index_of_unit(text, NEWLINE, position).unwrap_or(text.len());
        let mut in_url = false;
        for i in line_start..position {
            if self.regions[i] != PROSE {
                continue;
            }
            if text[i] == CLOSE_PAREN {
                in_url = false;
            } else if text[i] == OPEN_PAREN {
                in_url = i > 0 && text[i - 1] == CLOSE_BRACKET;
            }
        }
        in_url && (position..line_end).any(|i| text[i] == CLOSE_PAREN && self.regions[i] == PROSE)
    }

    /// The HTML tag mask at `position` (`inHtmlTagAt`). The mask is line by line, so this
    /// replays the position's line instead of building it.
    fn in_html_tag(&self, position: isize) -> bool {
        let text = self.text;
        let Ok(position) = usize::try_from(position) else {
            return false;
        };
        if position >= text.len()
            || text[position] == NEWLINE
            || !*self
                .has_less
                .get_or_init(|| find_unit(text, LESS).is_some())
        {
            return false;
        }
        let line_start = rfind_unit(&text[..position], NEWLINE).map_or(0, |index| index + 1);
        let mut in_tag = false;
        for i in line_start..position {
            if self.regions[i] != PROSE {
                continue;
            }
            if text[i] == GREATER {
                in_tag = false;
            } else if text[i] == LESS {
                in_tag = text.get(i + 1).is_some_and(|&next| {
                    (next < 0x80 && (next as u8).is_ascii_alphabetic()) || next == SLASH
                });
            }
        }
        in_tag
    }

    fn math_mask(&self) -> Vec<u8> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Context {
            None,
            InlineDollar,
            BlockDollar,
            InlineLatex,
            BlockLatex,
        }
        let text = self.text;
        let n = text.len();
        let mut mask = vec![0; n];
        let mut context = Context::None;
        let mut i = 0;
        while i < n {
            mask[i] = u8::from(context != Context::None);
            let delimiter = if self.regions[i] == PROSE {
                let next = text.get(i + 1).copied();
                if text[i] == BACKSLASH {
                    if next == Some(DOLLAR) {
                        Some((context, 2))
                    } else {
                        let latex = match (next, context) {
                            (Some(OPEN_BRACKET), Context::None) => Some(Context::BlockLatex),
                            (Some(CLOSE_BRACKET), Context::BlockLatex) => Some(Context::None),
                            (Some(OPEN_PAREN), Context::None) => Some(Context::InlineLatex),
                            (Some(CLOSE_PAREN), Context::InlineLatex) => Some(Context::None),
                            _ => None,
                        };
                        latex.map(|latex| (latex, 2))
                    }
                } else if text[i] == DOLLAR
                    && !matches!(context, Context::InlineLatex | Context::BlockLatex)
                {
                    let block = next == Some(DOLLAR);
                    let after = if block {
                        if context == Context::BlockDollar {
                            Context::None
                        } else {
                            Context::BlockDollar
                        }
                    } else if context == Context::BlockDollar {
                        context
                    } else if context == Context::InlineDollar {
                        Context::None
                    } else {
                        Context::InlineDollar
                    };
                    Some((after, if block { 2 } else { 1 }))
                } else {
                    None
                }
            } else {
                None
            };
            let Some((after, length)) = delimiter else {
                i += 1;
                continue;
            };
            context = after;
            if length == 2 && i + 1 < n {
                mask[i + 1] = u8::from(context != Context::None);
            }
            i += length;
        }
        mask
    }

    /// `countDoublePairs`: `**`, `~~` or `$$` in prose.
    fn count_double_pairs(&self, unit: u16) -> usize {
        let text = self.text;
        let pair = [unit, unit];
        let mut count = 0;
        let mut cursor = index_of(text, &pair, 0);
        while let Some(i) = cursor {
            let next = if self.regions[i] == PROSE {
                count += 1;
                i + 2
            } else {
                i + 1
            };
            cursor = index_of(text, &pair, next);
        }
        count
    }
}

// -------------------------------------------------------------------------------------
// patterns.ts: end-anchored delimiter patterns
// -------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Tail {
    /// `(marker)([^x]*?)$`
    Plain,
    /// `(\*\*)([^*]*\*?)$`
    OptionalStar,
    /// `(marker)([^x]+)x$`
    HalfComplete,
}

/// `matchTrailing`: the content group of the leftmost match, if any.
fn match_trailing(text: &[u16], delimiter: u16, marker_length: usize, tail: Tail) -> Option<Units> {
    let last = last_index_of(text, &[delimiter], Some(text.len() as isize - 2));
    let marker_end = last.map_or(text.len() as isize - 1, |last| last as isize);
    let start = usize::try_from((marker_end - marker_length as isize + 1).max(0)).unwrap_or(0);
    let slice = text.get(start..).unwrap_or(&[]);
    let n = slice.len();
    // `last` is the slice's last delimiter before its final unit, so a match's rest
    // holds a delimiter only as that final unit: the tests below need no scan. A marker
    // ending before `last` leaves it in the rest, so the search starts just past it.
    let before_final = last.map(|last| last - start);
    let final_delimiter = slice.last() == Some(&delimiter);
    let first = before_final.map_or(0, |at| (at + 1).saturating_sub(marker_length));
    for p in first..n {
        if !slice[p..]
            .iter()
            .take(marker_length)
            .eq(std::iter::repeat_n(&delimiter, marker_length))
            || p + marker_length > n
        {
            continue;
        }
        let rest_start = p + marker_length;
        if before_final.is_some_and(|at| at >= rest_start) {
            continue;
        }
        // The rest is `slice[rest_start..]`, with no delimiter except perhaps the final.
        let rest_len = n - rest_start;
        let rest_has_final = final_delimiter && rest_len > 0;
        let matched = match tail {
            Tail::Plain => !rest_has_final,
            Tail::OptionalStar => true,
            Tail::HalfComplete => rest_has_final && rest_len >= 2,
        };
        if matched {
            let content_end = match tail {
                Tail::HalfComplete => n - 1,
                _ => n,
            };
            return Some(slice[rest_start..content_end].to_vec());
        }
    }
    None
}

// -------------------------------------------------------------------------------------
// utils.ts
// -------------------------------------------------------------------------------------

fn find_matching_opening_bracket(text: &[u16], close: usize) -> Option<usize> {
    let mut depth = 1;
    for i in (0..close).rev() {
        if text[i] == CLOSE_BRACKET {
            depth += 1;
        } else if text[i] == OPEN_BRACKET {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn find_matching_closing_bracket(text: &[u16], open: usize) -> Option<usize> {
    let mut depth = 1;
    for (i, &unit) in text.iter().enumerate().skip(open + 1) {
        if unit == OPEN_BRACKET {
            depth += 1;
        } else if unit == CLOSE_BRACKET {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn is_horizontal_rule(text: &[u16], marker_index: usize, marker: u16) -> bool {
    let line_start = text[..marker_index]
        .iter()
        .rposition(|&unit| unit == NEWLINE)
        .map_or(0, |index| index + 1);
    let line_end = index_of_unit(text, NEWLINE, marker_index).unwrap_or(text.len());
    let mut count = 0;
    for &unit in &text[line_start..line_end] {
        if unit == marker {
            count += 1;
        } else if unit != SPACE && unit != TAB {
            return false;
        }
    }
    count >= 3
}

// -------------------------------------------------------------------------------------
// Handlers, in remend's priority order
// -------------------------------------------------------------------------------------

/// single-tilde-handler.ts: `20~25` → `20\~25`.
fn single_tilde(text: Units) -> Units {
    if !text.contains(&TILDE) {
        return text;
    }
    let scan = Scan::new(&text);
    let mut out = Vec::with_capacity(text.len() + 4);
    let mut copied = 0;
    let mut i = 0;
    while i < text.len() {
        let (code_point, length) = code_point_at(&text, i);
        let tilde = i + length;
        let matched = word_code_point(code_point)
            && text.get(tilde) == Some(&TILDE)
            && text.get(tilde + 1) != Some(&TILDE)
            && tilde + 1 < text.len()
            && word_code_point(code_point_at(&text, tilde + 1).0);
        if matched {
            if !scan.code_at(tilde as isize) {
                out.extend_from_slice(&text[copied..tilde]);
                out.push(BACKSLASH);
                out.push(TILDE);
                copied = tilde + 1;
            }
            i = tilde + 1;
        } else {
            i += length;
        }
    }
    if copied == 0 {
        return text;
    }
    out.extend_from_slice(&text[copied..]);
    out
}

/// comparison-operator-handler.ts: `- > 25` → `- \> 25`.
fn comparison_operators(text: Units) -> Units {
    if !text.contains(&GREATER) {
        return text;
    }
    // `/^(\s*(?:[-*+]|\d+[.)]) +)>(=?\s*[$]?\d)/gm`: the position of `>` and the end.
    let attempt = |p: usize| -> Option<(usize, usize)> {
        let mut i = p;
        while i < text.len() && js_space(text[i]) {
            i += 1;
        }
        match text.get(i).copied() {
            Some(DASH | STAR | PLUS) => i += 1,
            _ => {
                let digits = i;
                while digit(text.get(i).copied()) {
                    i += 1;
                }
                if i == digits || !matches!(text.get(i).copied(), Some(DOT | CLOSE_PAREN)) {
                    return None;
                }
                i += 1;
            }
        }
        let spaces = i;
        while text.get(i) == Some(&SPACE) {
            i += 1;
        }
        if i == spaces || text.get(i) != Some(&GREATER) {
            return None;
        }
        let greater = i;
        i += 1;
        if text.get(i) == Some(&EQUALS) {
            i += 1;
        }
        while i < text.len() && js_space(text[i]) {
            i += 1;
        }
        if text.get(i) == Some(&DOLLAR) {
            i += 1;
        }
        digit(text.get(i).copied()).then_some((greater, i + 1))
    };
    let mut scan: Option<Scan<'_>> = None;
    let mut escapes = Vec::new();
    // Without U+2028 or U+2029, lines end only at `\n` or `\r`, which memchr finds.
    let separators = text.iter().any(|&unit| matches!(unit, 0x2028 | 0x2029));
    let mut p = 0;
    while p <= text.len() {
        let line_start = p == 0 || line_terminator(text[p - 1]);
        if let Some((greater, end)) = line_start.then(|| attempt(p)).flatten() {
            let scan = scan.get_or_insert_with(|| Scan::new(&text));
            if !scan.code_at(p as isize) {
                escapes.push(greater);
            }
            p = end;
            continue;
        }
        // `^` only matches at line starts: go to the next one.
        let next = if separators {
            text[p..].iter().position(|&unit| line_terminator(unit))
        } else {
            find_unit2(&text[p..], NEWLINE, CR)
        };
        match next {
            Some(offset) => p += offset + 1,
            None => break,
        }
    }
    if escapes.is_empty() {
        return text;
    }
    let mut out = Vec::with_capacity(text.len() + escapes.len());
    let mut copied = 0;
    for greater in escapes {
        out.extend_from_slice(&text[copied..greater]);
        out.push(BACKSLASH);
        copied = greater;
    }
    out.extend_from_slice(&text[copied..]);
    out
}

/// html-tag-handler.ts: drops an incomplete trailing tag (`text <custom`).
fn incomplete_html_tag(text: Units) -> Units {
    let starts_tag = |index: usize| {
        text.get(index + 1).is_some_and(|&next| {
            (next < 0x80 && (next as u8).is_ascii_alphabetic()) || next == SLASH
        })
    };
    // `/<[a-zA-Z/][^>]*$/`: the first such `<` after the last `>`.
    let after = text
        .iter()
        .rposition(|&unit| unit == GREATER)
        .map_or(0, |index| index + 1);
    let Some(first) = (after..text.len()).find(|&index| text[index] == LESS && starts_tag(index))
    else {
        return text;
    };
    let check_math = text.contains(&DOLLAR)
        || text
            .windows(2)
            .any(|pair| pair[0] == BACKSLASH && matches!(pair[1], OPEN_PAREN | OPEN_BRACKET));
    let scan = Scan::new(&text);
    for index in first..text.len() {
        if text[index] != LESS || !starts_tag(index) {
            continue;
        }
        if scan.code_at(index as isize) {
            continue;
        }
        if check_math && scan.in_math(index as isize) {
            continue;
        }
        return trim_end(&text[..index]).to_vec();
    }
    drop(scan);
    text
}

/// setext-heading-handler.ts: breaks a `-`/`=` line that would make a heading.
fn incomplete_setext_heading(text: Units) -> Units {
    let Some(last_newline) = text.iter().rposition(|&unit| unit == NEWLINE) else {
        return text;
    };
    let last_line = &text[last_newline + 1..];
    let previous = &text[..last_newline];
    let trimmed = trim(last_line);
    for marker in [DASH, EQUALS] {
        // `/^-{1,2}$/` on the trimmed line, and not `/^[\s]*-{1,2}[\s]+$/`.
        let only = matches!(trimmed.len(), 1 | 2) && trimmed.iter().all(|&unit| unit == marker);
        let with_space = {
            let rest = trim_start(last_line);
            let run = rest.iter().take_while(|&&unit| unit == marker).count();
            let after = &rest[run..];
            matches!(run, 1 | 2) && !after.is_empty() && after.iter().all(|&unit| js_space(unit))
        };
        if only && !with_space {
            let previous_line = previous
                .iter()
                .rposition(|&unit| unit == NEWLINE)
                .map_or(previous, |index| &previous[index + 1..]);
            if !previous_line.is_empty() && !trim(previous_line).is_empty() {
                return appended(text, "\u{200b}");
            }
        }
    }
    text
}

/// link-image-handler.ts, `linkMode: "protocol"`.
fn incomplete_links_and_images(text: Units) -> Units {
    let mut current = text;
    for _ in 0..32 {
        let next = heal_trailing_link_or_image(&current);
        if next.len() >= current.len() {
            return next;
        }
        current = next;
    }
    current
}

fn heal_trailing_link_or_image(text: &[u16]) -> Units {
    let scan = Scan::new(text);
    let paren = last_index_of(text, &units("]("), None);
    if let Some(paren) = paren
        && !scan.code_at(paren as isize)
        && let Some(result) = incomplete_url(text, &scan, paren)
    {
        return result;
    }
    // The last `[` outside code with no matching `]` (`findMatchingClosingBracket`, which
    // counts every bracket): one pass pairs them all, instead of a scan per `[`.
    let mut open = Vec::new();
    for_each_unit2(text, OPEN_BRACKET, CLOSE_BRACKET, |index| {
        if text[index] == OPEN_BRACKET {
            open.push(index);
        } else {
            open.pop();
        }
    });
    for &i in open.iter().rev() {
        if !scan.code_at(i as isize)
            && let Some(result) = incomplete_link_text(text, i)
        {
            return result;
        }
    }
    text.to_vec()
}

fn incomplete_url(text: &[u16], scan: &Scan<'_>, paren: usize) -> Option<Units> {
    if text[paren + 2..].contains(&CLOSE_PAREN) {
        return None;
    }
    let open = find_matching_opening_bracket(text, paren)?;
    if scan.code_at(open as isize) {
        return None;
    }
    let image = open > 0 && text[open - 1] == BANG;
    let start = if image { open - 1 } else { open };
    let label = &text[open + 1..paren];
    let target = if image {
        "](streamdown:incomplete-image)"
    } else {
        "](streamdown:incomplete-link)"
    };
    let opener: &[u16] = if image {
        &[BANG, OPEN_BRACKET]
    } else {
        &[OPEN_BRACKET]
    };
    Some(concat(&[&text[..start], opener, label, &units(target)]))
}

fn incomplete_link_text(text: &[u16], i: usize) -> Option<Units> {
    let image = i > 0 && text[i - 1] == BANG;
    let open = if image { i - 1 } else { i };
    let unclosed =
        !text[i + 1..].contains(&CLOSE_BRACKET) || find_matching_closing_bracket(text, i).is_none();
    if !unclosed {
        return None;
    }
    if image {
        let alt = &text[i + 1..];
        return Some(concat(&[
            &text[..open],
            &[BANG, OPEN_BRACKET],
            alt,
            &units("](streamdown:incomplete-image)"),
        ]));
    }
    Some(appended(text.to_vec(), "](streamdown:incomplete-link)"))
}

/// emphasis-handlers.ts `shouldSkipAsterisk`.
fn skip_asterisk(scan: &Scan<'_>, index: usize, previous: Option<u16>, next: Option<u16>) -> bool {
    // `shouldSkipAsterisk` is an or of its tests; the math test, which needs the math
    // mask, goes last.
    let skip = if previous == Some(BACKSLASH) {
        true
    } else if previous != Some(STAR) && next == Some(STAR) {
        let after = if index + 2 < scan.text.len() {
            Some(scan.text[index + 2])
        } else {
            None
        };
        after != Some(STAR)
    } else if previous == Some(STAR) {
        true
    } else {
        let previous_space = previous.is_none_or(|unit| matches!(unit, SPACE | TAB | NEWLINE));
        let next_space = next.is_none_or(|unit| matches!(unit, SPACE | TAB | NEWLINE));
        previous_space && next_space
    };
    skip || scan.in_math(index as isize)
}

fn whitespace_unit(unit: u16) -> bool {
    matches!(unit, SPACE | TAB | NEWLINE)
}

fn count_single_asterisks(scan: &Scan<'_>) -> usize {
    let text = scan.text;
    let mut count = 0;
    let mut chain = false;
    for index in 0..text.len() {
        if text[index] != STAR || scan.regions[index] != PROSE {
            if !word_unit(Some(text[index])) {
                chain = false;
            }
            continue;
        }
        let previous = if index > 0 {
            Some(text[index - 1])
        } else {
            None
        };
        let next = text.get(index + 1).copied();
        if skip_asterisk(scan, index, previous, next) {
            continue;
        }
        let internal = word_unit(previous) && word_unit(next);
        let can_open = next.is_some_and(|unit| !whitespace_unit(unit));
        let can_close = previous.is_some_and(|unit| !whitespace_unit(unit));
        if internal && count % 2 == 0 && !chain {
            continue;
        }
        if (can_close && count % 2 == 1) || can_open {
            count += 1;
            chain = internal;
        }
    }
    count
}

fn count_single_underscores(scan: &Scan<'_>) -> usize {
    let text = scan.text;
    let mut count = 0;
    let mut cursor = index_of_unit(text, UNDERSCORE, 0);
    while let Some(index) = cursor {
        cursor = index_of_unit(text, UNDERSCORE, index + 1);
        if scan.regions[index] != PROSE {
            continue;
        }
        let previous = if index > 0 {
            Some(text[index - 1])
        } else {
            None
        };
        let next = text.get(index + 1).copied();
        // An or of `shouldSkipUnderscore`'s tests, the ones needing masks last.
        let skip = previous == Some(BACKSLASH)
            || previous == Some(UNDERSCORE)
            || next == Some(UNDERSCORE)
            || (word_unit(previous) && word_unit(next))
            || scan.in_math(index as isize)
            || scan.in_link_url(index as isize)
            || scan.in_html_tag(index as isize);
        if !skip {
            count += 1;
        }
    }
    count
}

fn count_triple_asterisks(scan: &Scan<'_>) -> usize {
    let mut count = 0;
    let mut run = 0;
    for (index, &unit) in scan.text.iter().enumerate() {
        if unit == STAR && scan.regions[index] == PROSE {
            run += 1;
        } else {
            count += run / 3;
            run = 0;
        }
    }
    count + run / 3
}

/// `hasUnmatchedDoubleUnderscore`.
fn unmatched_double_underscore(scan: &Scan<'_>) -> bool {
    let text = scan.text;
    let n = text.len();
    // `isThematicBreakRun`'s memo: the line end and verdict.
    let mut memo: (isize, bool) = (-1, false);
    let mut unmatched = false;
    let mut i = 0;
    while i < n {
        if text[i] != UNDERSCORE || scan.regions[i] != PROSE {
            i += 1;
            continue;
        }
        let first = i;
        let mut end = i + 1;
        while end < n && text[end] == UNDERSCORE && scan.regions[end] == PROSE {
            end += 1;
        }
        i = end;
        let mut start = first;
        let mut escaped = false;
        if start > 0 && text[start - 1] == BACKSLASH {
            start += 1;
            escaped = true;
        }
        if end < start + 2 {
            continue;
        }
        let before = if start > 0 {
            Some(text[start - 1])
        } else {
            None
        };
        let previous = if escaped { Some(BACKSLASH) } else { before };
        let next = text.get(end).copied();
        if word_unit(previous) && word_unit(next) {
            continue;
        }
        let boundary =
            |unit: Option<u16>| unit.is_none_or(|unit| matches!(unit, SPACE | TAB | NEWLINE));
        if boundary(previous) && boundary(next) {
            if start as isize > memo.0 {
                let line_end = index_of_unit(text, NEWLINE, start).unwrap_or(n);
                memo = (
                    line_end as isize,
                    is_horizontal_rule(text, start, UNDERSCORE),
                );
            }
            if memo.1 {
                continue;
            }
        }
        if scan.in_math(start as isize)
            || scan.in_link_url(start as isize)
            || scan.in_html_tag(start as isize)
        {
            continue;
        }
        if ((end - start) / 2) % 2 == 1 {
            unmatched = !unmatched;
        }
    }
    unmatched
}

/// `shouldSkipBoldCompletion` / `shouldSkipItalicCompletion` (which differ in the rule's
/// marker).
fn skip_completion(text: &[u16], content: &[u16], marker_index: usize, rule: u16) -> bool {
    if content.is_empty() || whitespace_or_markers(content) {
        return true;
    }
    let line_start = text[..marker_index]
        .iter()
        .rposition(|&unit| unit == NEWLINE)
        .map_or(0, |index| index + 1);
    if list_item_prefix(&text[line_start..marker_index]) && content.contains(&NEWLINE) {
        return true;
    }
    is_horizontal_rule(text, marker_index, rule)
}

fn in_code(scan: &Scan<'_>, index: usize) -> bool {
    scan.code_at(index as isize) || scan.complete_span_at(index as isize)
}

fn incomplete_bold(text: Units) -> Units {
    let Some(content) = match_trailing(&text, STAR, 2, Tail::OptionalStar) else {
        return text;
    };
    let marker = last_index_of(&text, &[STAR, STAR], None).expect("the match holds **");
    let scan = Scan::new(&text);
    if in_code(&scan, marker) || skip_completion(&text, &content, marker, STAR) {
        return text;
    }
    if scan.count_double_pairs(STAR) % 2 == 1 {
        drop(scan);
        return appended(
            text,
            if content.last() == Some(&STAR) {
                "*"
            } else {
                "**"
            },
        );
    }
    drop(scan);
    text
}

fn incomplete_double_underscore_italic(text: Units) -> Units {
    let Some(content) = match_trailing(&text, UNDERSCORE, 2, Tail::Plain) else {
        if match_trailing(&text, UNDERSCORE, 2, Tail::HalfComplete).is_some() {
            let marker =
                last_index_of(&text, &[UNDERSCORE, UNDERSCORE], None).expect("the match holds __");
            let scan = Scan::new(&text);
            if !in_code(&scan, marker) && unmatched_double_underscore(&scan) {
                drop(scan);
                return appended(text, "_");
            }
        }
        return text;
    };
    let marker = last_index_of(&text, &[UNDERSCORE, UNDERSCORE], None).expect("the match holds __");
    let scan = Scan::new(&text);
    if in_code(&scan, marker) || skip_completion(&text, &content, marker, UNDERSCORE) {
        return text;
    }
    if unmatched_double_underscore(&scan) {
        drop(scan);
        return appended(text, "__");
    }
    drop(scan);
    text
}

fn first_single_asterisk(scan: &Scan<'_>) -> Option<usize> {
    let text = scan.text;
    let mut cursor = index_of_unit(text, STAR, 0);
    while let Some(i) = cursor {
        cursor = index_of_unit(text, STAR, i + 1);
        let previous = at(text, i as isize - 1);
        let next = text.get(i + 1).copied();
        let lone = scan.regions[i] == PROSE
            && previous != Some(STAR)
            && next != Some(STAR)
            && previous != Some(BACKSLASH);
        if !lone {
            continue;
        }
        let previous_space = previous.is_none_or(whitespace_unit);
        let next_space = next.is_none_or(whitespace_unit);
        if previous_space && next_space {
            continue;
        }
        if previous.is_some() && next.is_some() && word_unit(previous) && word_unit(next) {
            continue;
        }
        if next_space {
            continue;
        }
        // The last of `isLoneProseAsterisk`'s tests, which needs the math mask.
        if scan.in_math(i as isize) {
            continue;
        }
        return Some(i);
    }
    None
}

fn incomplete_single_asterisk_italic(text: Units) -> Units {
    if match_trailing(&text, STAR, 1, Tail::Plain).is_none() {
        return text;
    }
    let scan = Scan::new(&text);
    let Some(first) = first_single_asterisk(&scan) else {
        return text;
    };
    if in_code(&scan, first) {
        return text;
    }
    let content = &text[first + 1..];
    if content.is_empty() || whitespace_or_markers(content) {
        return text;
    }
    if count_single_asterisks(&scan) % 2 == 1 {
        drop(scan);
        return appended(text, "*");
    }
    drop(scan);
    text
}

fn first_single_underscore(scan: &Scan<'_>) -> Option<usize> {
    let text = scan.text;
    let mut cursor = index_of_unit(text, UNDERSCORE, 0);
    while let Some(i) = cursor {
        cursor = index_of_unit(text, UNDERSCORE, i + 1);
        let previous = at(text, i as isize - 1);
        let next = text.get(i + 1).copied();
        // Word-internal underscores are skipped either way; testing them first spares
        // the masks.
        if scan.regions[i] == PROSE
            && previous != Some(UNDERSCORE)
            && next != Some(UNDERSCORE)
            && previous != Some(BACKSLASH)
            && !(previous.is_some() && next.is_some() && word_unit(previous) && word_unit(next))
            && !scan.in_math(i as isize)
            && !scan.in_link_url(i as isize)
        {
            return Some(i);
        }
    }
    None
}

fn incomplete_single_underscore_italic(text: Units) -> Units {
    if match_trailing(&text, UNDERSCORE, 1, Tail::Plain).is_none() {
        return text;
    }
    let scan = Scan::new(&text);
    let Some(first) = first_single_underscore(&scan) else {
        return text;
    };
    let content = &text[first + 1..];
    if content.is_empty() || whitespace_or_markers(content) {
        return text;
    }
    if in_code(&scan, first) {
        return text;
    }
    if count_single_underscores(&scan).is_multiple_of(2) {
        return text;
    }
    drop(scan);
    // `handleTrailingAsterisksForUnderscore`: close `_` before a `**` that closed an
    // earlier one.
    if text.ends_with(&[STAR, STAR]) {
        let without = &text[..text.len() - 2];
        let scan = Scan::new(without);
        if scan.count_double_pairs(STAR) % 2 == 1 {
            let bold = index_of(without, &[STAR, STAR], 0);
            let underscore = first_single_underscore(&scan);
            if let (Some(bold), Some(underscore)) = (bold, underscore)
                && bold < underscore
            {
                return concat(&[without, &units("_**")]);
            }
        }
    }
    // `insertClosingUnderscore`: before any trailing newlines.
    let end = text
        .iter()
        .rposition(|&unit| unit != NEWLINE)
        .map_or(0, |index| index + 1);
    concat(&[&text[..end], &[UNDERSCORE], &text[end..]])
}

fn incomplete_bold_italic(text: Units) -> Units {
    if text.len() >= 4 && text.iter().all(|&unit| unit == STAR) {
        return text;
    }
    let Some(content) = match_trailing(&text, STAR, 3, Tail::Plain) else {
        return text;
    };
    let marker = last_index_of(&text, &[STAR, STAR, STAR], None).expect("the match holds ***");
    if content.is_empty() || whitespace_or_markers(&content) {
        return text;
    }
    let scan = Scan::new(&text);
    if in_code(&scan, marker) || is_horizontal_rule(&text, marker, STAR) {
        return text;
    }
    if count_triple_asterisks(&scan) % 2 == 1 {
        let balanced = scan.count_double_pairs(STAR).is_multiple_of(2)
            && count_single_asterisks(&scan).is_multiple_of(2);
        drop(scan);
        if balanced {
            return text;
        }
        return appended(text, "***");
    }
    drop(scan);
    text
}

fn incomplete_inline_code(text: Units) -> Units {
    let scan = Scan::new(&text);
    if scan.open_fence.is_some() {
        return text;
    }
    let Some(span) = scan.open_span else {
        return text;
    };
    drop(scan);
    let content = &text[span.start + span.run_length..];
    if content.is_empty() || whitespace_or_markers(content) {
        return text;
    }
    let trailing = text
        .iter()
        .rev()
        .take_while(|&&unit| unit == BACKTICK)
        .count();
    if trailing >= span.run_length {
        return text;
    }
    let mut out = text;
    out.extend(std::iter::repeat_n(BACKTICK, span.run_length - trailing));
    out
}

fn incomplete_strikethrough(text: Units) -> Units {
    if let Some(content) = match_trailing(&text, TILDE, 2, Tail::Plain) {
        if content.is_empty() || whitespace_or_markers(&content) {
            return text;
        }
        let marker = last_index_of(&text, &[TILDE, TILDE], None).expect("the match holds ~~");
        let scan = Scan::new(&text);
        if in_code(&scan, marker) {
            return text;
        }
        if scan.count_double_pairs(TILDE) % 2 == 1 {
            drop(scan);
            return appended(text, "~~");
        }
    } else if match_trailing(&text, TILDE, 2, Tail::HalfComplete).is_some() {
        let marker = last_index_of(&text, &[TILDE, TILDE], None).expect("the match holds ~~");
        let scan = Scan::new(&text);
        if in_code(&scan, marker) {
            return text;
        }
        if scan.count_double_pairs(TILDE) % 2 == 1 {
            drop(scan);
            return appended(text, "~");
        }
    }
    text
}

fn incomplete_block_katex(text: Units) -> Units {
    let pairs = Scan::new(&text).count_double_pairs(DOLLAR);
    if pairs.is_multiple_of(2) {
        return text;
    }
    if text.ends_with(&[DOLLAR]) && !text.ends_with(&[DOLLAR, DOLLAR]) {
        return appended(text, "$");
    }
    let first = index_of(&text, &[DOLLAR, DOLLAR], 0);
    let newline_after = first.is_some_and(|first| index_of_unit(&text, NEWLINE, first).is_some());
    if newline_after && text.last() != Some(&NEWLINE) {
        return appended(text, "\n$$");
    }
    appended(text, "$$")
}

#[cfg(test)]
mod whole_timing {
    #[test]
    #[ignore = "timing, run by hand: REMEND_DOC=file cargo test --release -p rill remend_time -- --ignored --nocapture"]
    fn remend_time() {
        let path = std::env::var("REMEND_DOC").expect("REMEND_DOC");
        let text: super::Units = std::fs::read_to_string(path)
            .unwrap()
            .encode_utf16()
            .collect();
        let start = std::time::Instant::now();
        for _ in 0..std::env::var("REMEND_ROUNDS").map_or(500, |r| r.parse().unwrap()) {
            super::remend_units(text.clone());
        }
        println!(
            "remend {:>7.1} µs",
            start.elapsed().as_secs_f64() * 1e6 / 500.0
        );
    }
}
