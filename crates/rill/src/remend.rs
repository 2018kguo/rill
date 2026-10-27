//! remend 1.4.0 (Streamdown's healing of incomplete streaming Markdown) with its
//! default options, ported from packages/remend/src (Apache-2.0, Copyright 2023
//! Vercel, Inc.). It works on UTF-16 code units, as the JavaScript does, so every
//! index, slice and comparison is the same; `conformance/remend.mjs` checks the output
//! against the package.

use std::cell::OnceCell;

use crate::text::js_space;

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
    String::from_utf16_lossy(&remend_units(&units(text)))
}

/// [`remend`] on UTF-16 code units.
pub fn remend_units(text: &[u16]) -> Units {
    let mut healed = Units::new();
    heal_from(text, &Settled::default(), &mut healed);
    healed
}

/// A healed text as an edit of the one healed before it: that text's first `keep`
/// units, then `middle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Healing {
    pub keep: usize,
    pub middle: String,
}

/// remend over a text that streams. It keeps the text and what it has settled about a
/// prefix of it, so healing an update costs what the text after that prefix costs, not
/// the whole text; and it answers with an edit of the text it healed last time, so
/// only the changes cross a language boundary.
///
/// The settled prefix ends after a blank line, where nothing remend scans for is
/// open: no inline code span (a blank line ends one), no math, and no comparison the
/// next text could complete. Every handler of remend then sees the prefix the same
/// way whatever follows it: its escapes are final, and what the handlers read from it
/// (counts, the first and last markers, open brackets) is folded into [`Settled`]
/// once. The text after it is healed as remend heals the whole text, with those
/// folds standing in for the prefix; an edit that would reach into the prefix (rare:
/// it needs the text after it to be blank) heals the whole text instead.
#[derive(Debug, Default)]
pub struct Stream {
    /// The text so far.
    input: Units,
    /// `input[..settled]` is the settled prefix.
    settled: usize,
    /// The fence open at `settled` in `input` (single-tilde's scan).
    fence1: Option<OpenFence>,
    /// The fence open at the end of single-tilde's output for the settled prefix
    /// (comparison-operator's scan).
    fence2: Option<OpenFence>,
    /// What the later handlers need of the prefix comparison-operator makes of it.
    prefix: Settled,
    /// The settled prefix before the last one grew it: an edit that reaches back
    /// over the whitespace ending the prefix (a trailing tag dropped with the blank
    /// line before it) heals from there instead.
    earlier: Option<Settled>,
    /// The regions of the settled prefix's text.
    regions: Vec<u8>,
    /// That prefix's text (`prefix.len` units), then the rest of the last healed text
    /// when the last heal took the fast path.
    work: Units,
    /// The last healed text when it came from healing the whole text instead.
    whole: Option<Units>,
}

impl Stream {
    /// Heals the text so far plus `appended` (or, with `reset`, just `appended`).
    pub fn heal(&mut self, appended: &str, reset: bool) -> Healing {
        let (keep, middle) = self.heal_with(appended, reset, |middle, _| {
            String::from_utf16_lossy(middle)
        });
        Healing { keep, middle }
    }

    /// [`Stream::heal`], handing the middle (and the whole input text) to `take` as
    /// UTF-16 instead of building a `String` of it.
    pub fn heal_with<M>(
        &mut self,
        appended: &str,
        reset: bool,
        take: impl FnOnce(&[u16], &[u16]) -> M,
    ) -> (usize, M) {
        if reset {
            *self = Stream::default();
        }
        self.input.extend(appended.encode_utf16());
        let known = self.prefix.len;
        let previous_whole = self.whole.take();
        let previous_tail = self.work.split_off(known);
        self.settle();
        // What the last heal returned: `work[..known]` and then `previous_tail`, or
        // `previous_whole`.
        let keep_of = |work: &[u16], new: &[u16]| -> usize {
            if reset {
                return 0;
            }
            if let Some(previous) = &previous_whole {
                return common_prefix(previous, new);
            }
            // The settled text only grows, so the new text keeps `work[..known]` when it
            // starts with the settled prefix (the fast path); a whole-text heal may not.
            let head = if std::ptr::eq(work, new) {
                known
            } else {
                common_prefix(&work[..known], &new[..known.min(new.len())])
            };
            if head < known {
                return head;
            }
            known + common_prefix(&previous_tail, &new[known..])
        };
        if self.heal_tail().is_some() {
            let keep = keep_of(&self.work, &self.work);
            let middle = take(&self.work[keep..], &self.input);
            return (keep, middle);
        }
        let whole = self.heal_earlier();
        self.work.truncate(self.prefix.len);
        let keep = keep_of(&self.work, &whole);
        let middle = take(&whole[keep..], &self.input);
        self.whole = Some(whole);
        (keep, middle)
    }

    /// The text after the settled prefix after the line-local escapes.
    fn escaped_tail(&self) -> Units {
        let input = &self.input;
        let end = stripped_end(input);
        let tail1 = single_tilde(&input[..end], self.settled, self.fence1);
        comparison_operators(&tail1, 0, self.fence2).0
    }

    /// Heals `input` after the settled prefix into `work` (after the prefix's text);
    /// `None` when an edit would reach into the prefix.
    fn heal_tail(&mut self) -> Option<()> {
        self.work.truncate(self.prefix.len);
        if self.input.is_empty() {
            return Some(());
        }
        let tail = self.escaped_tail();
        self.work.extend_from_slice(&tail);
        heal_handlers(&mut self.work, &self.prefix, &self.regions)
    }

    /// The healed text when an edit would reach into the settled prefix: from the
    /// prefix before it (an edit that drops a trailing tag with the line ending
    /// before it), else from the whole text.
    fn heal_earlier(&self) -> Units {
        if let Some(earlier) = &self.earlier {
            let mut text = self.work[..self.prefix.len].to_vec();
            text.extend_from_slice(&self.escaped_tail());
            if heal_handlers(&mut text, earlier, &self.regions[..earlier.len]).is_some() {
                return text;
            }
        }
        remend_units(&self.input)
    }

    /// Moves the settled prefix up to the last line start that qualifies.
    fn settle(&mut self) {
        let input = &self.input;
        let end = stripped_end(input);
        let Some(cut) = settle_point(input, self.settled, end) else {
            return;
        };
        let delta = &input[self.settled..cut];
        let scan1 = TailScan::new(delta, self.fence1);
        if scan1.open_span.is_some() {
            return;
        }
        let escaped = single_tilde(delta, 0, self.fence1);
        let scan2 = TailScan::new(&escaped, self.fence2);
        if scan2.open_span.is_some() {
            return;
        }
        let (compared, open) = comparison_operators(&escaped, 0, self.fence2);
        if open {
            return;
        }
        let from = self.work.len();
        self.work.extend_from_slice(&compared);
        let Some((prefix, regions)) = self.prefix.extended(&self.work, from, &self.regions) else {
            self.work.truncate(from);
            return;
        };
        self.earlier = Some(std::mem::replace(&mut self.prefix, prefix));
        self.regions.extend_from_slice(&regions);
        self.settled = cut;
        self.fence1 = scan1.open_fence;
        self.fence2 = scan2.open_fence;
    }
}

/// Where `strip_single_trailing_space` ends the text: one trailing space goes.
fn stripped_end(text: &[u16]) -> usize {
    let n = text.len();
    if n >= 1 && text[n - 1] == SPACE && !(n >= 2 && text[n - 2] == SPACE) {
        n - 1
    } else {
        n
    }
}

/// The start of the last line in `text[from..end]` with text that is not whitespace
/// from it on (so an edit at the end seldom reaches back over it).
fn settle_point(text: &[u16], from: usize, end: usize) -> Option<usize> {
    let last_content = text[from..end].iter().rposition(|&unit| !js_space(unit))? + from;
    let newline = rfind_unit(&text[from..last_content], NEWLINE)? + from;
    (newline + 1 > from).then_some(newline + 1)
}

fn common_prefix(left: &[u16], right: &[u16]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count()
}

/// remend on `text` after the settled `prefix` of its comparison output.
fn heal_from(text: &[u16], prefix: &Settled, out: &mut Units) {
    debug_assert_eq!(prefix.len, 0);
    out.clear();
    if text.is_empty() {
        return;
    }
    let end = stripped_end(text);
    let escaped = single_tilde(&text[..end], 0, None);
    out.extend_from_slice(&comparison_operators(&escaped, 0, None).0);
    heal_handlers(out, prefix, &[]).expect("an empty prefix holds no edit");
}

/// The handlers after comparison-operator's, in remend's order; `None` when one would
/// edit the settled prefix.
fn heal_handlers(text: &mut Units, prefix: &Settled, head: &[u8]) -> Option<()> {
    let mut ctx = Ctx::new(std::mem::take(text), prefix, head);
    let handlers: [fn(&mut Ctx<'_>) -> Option<()>; 11] = [
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
    let mut result = Some(());
    for (index, handler) in handlers.into_iter().enumerate() {
        if handler(&mut ctx).is_none() {
            result = None;
            break;
        }
        // The links handler (index 2) ends healing once it closed a link or image.
        if index == 2
            && (ends_with(&ctx.text, INCOMPLETE_LINK) || ends_with(&ctx.text, INCOMPLETE_IMAGE))
        {
            *text = ctx.text;
            return Some(());
        }
    }
    *text = ctx.text;
    result?;
    let end = stripped_end(text);
    if end < prefix.len {
        return None;
    }
    text.truncate(end);
    Some(())
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
    if code_point < 0x80 {
        return code_point == u32::from(UNDERSCORE) || (code_point as u8).is_ascii_alphanumeric();
    }
    letter_or_number(code_point)
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

#[derive(Clone, Copy, Debug)]
struct OpenFence {
    marker: u16,
    length: usize,
    list_indent: usize,
    quote_depth: usize,
}

#[derive(Clone, Copy, Debug)]
struct OpenSpan {
    run_length: usize,
    start: usize,
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

/// Paints the fences of `text`, which starts at a line start with `open` open.
fn paint_fences(
    text: &[u16],
    regions: &mut [u8],
    mut open: Option<OpenFence>,
) -> Option<OpenFence> {
    let n = text.len();
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

/// The start of the line holding `index`.
fn line_start(text: &[u16], index: usize) -> usize {
    rfind_unit(&text[..index], NEWLINE).map_or(0, |newline| newline + 1)
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

fn is_horizontal_rule(text: &[u16], marker_index: usize, marker: u16) -> bool {
    let line_start = line_start(text, marker_index);
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
// scan.ts over a text after a settled prefix
// -------------------------------------------------------------------------------------

/// The regions of a text that starts at a line start where no inline code span is
/// open (the start of a text, or a settled prefix's end), given the fence open there.
#[derive(Debug)]
struct TailScan {
    regions: Vec<u8>,
    open_fence: Option<OpenFence>,
    open_span: Option<OpenSpan>,
}

impl TailScan {
    fn new(text: &[u16], fence: Option<OpenFence>) -> TailScan {
        let mut regions = vec![PROSE; text.len()];
        let open_fence = paint_fences(text, &mut regions, fence);
        let open_span = paint_spans(text, &mut regions);
        TailScan {
            regions,
            open_fence,
            open_span,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MathContext {
    #[default]
    None,
    InlineDollar,
    BlockDollar,
    InlineLatex,
    BlockLatex,
}

/// `text` (whole) seen from `split` on: the regions of `text[split..]` and, for the
/// positions before it, the settled prefix's. Every question a handler asks about a
/// position at or after `split` reads only `text[split..]` and units just before it
/// (the prefix ends with a blank line).
struct View<'a> {
    text: &'a [u16],
    split: usize,
    head: &'a [u8],
    scan: &'a TailScan,
    /// The math mask of `text[split..]` (the context at `split` is none), built when
    /// first asked for.
    math: OnceCell<Vec<u8>>,
    has_link: OnceCell<bool>,
    has_less: OnceCell<bool>,
}

impl<'a> View<'a> {
    fn new(text: &'a [u16], split: usize, head: &'a [u8], scan: &'a TailScan) -> View<'a> {
        View {
            text,
            split,
            head,
            scan,
            math: OnceCell::new(),
            has_link: OnceCell::new(),
            has_less: OnceCell::new(),
        }
    }

    fn tail(&self) -> &'a [u16] {
        &self.text[self.split..]
    }

    fn region_at(&self, position: usize) -> u8 {
        if position < self.split {
            self.head[position]
        } else {
            self.scan.regions[position - self.split]
        }
    }

    fn region(&self, position: isize) -> Option<u8> {
        let position = usize::try_from(position).ok()?;
        (position < self.text.len()).then(|| self.region_at(position))
    }

    /// `isCodeAt`: a fence or inline span.
    fn code_at(&self, position: isize) -> bool {
        if position >= self.text.len() as isize {
            return self.scan.open_fence.is_some() || self.scan.open_span.is_some();
        }
        if position < 0 {
            return false;
        }
        self.region_at(position as usize) != PROSE
    }

    fn complete_span_at(&self, position: isize) -> bool {
        self.region(position) == Some(CODE_SPAN)
    }

    /// The math mask at a position at or after `split`.
    fn in_math(&self, position: isize) -> bool {
        let Ok(position) = usize::try_from(position) else {
            return false;
        };
        if position < self.split || position >= self.text.len() {
            return false;
        }
        let mask = self.math.get_or_init(|| {
            let text = self.tail();
            let has = text.contains(&DOLLAR)
                || text.windows(2).any(|pair| {
                    pair[0] == BACKSLASH && matches!(pair[1], OPEN_PAREN | OPEN_BRACKET)
                });
            if has {
                math_mask(text, &self.scan.regions, MathContext::None).0
            } else {
                Vec::new()
            }
        });
        mask.get(position - self.split) == Some(&1)
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
                .get_or_init(|| index_of(self.tail(), &[CLOSE_BRACKET, OPEN_PAREN], 0).is_some())
        {
            return false;
        }
        let line_start = line_start(text, position);
        let line_end = index_of_unit(text, NEWLINE, position).unwrap_or(text.len());
        let mut in_url = false;
        for i in line_start..position {
            if self.region_at(i) != PROSE {
                continue;
            }
            if text[i] == CLOSE_PAREN {
                in_url = false;
            } else if text[i] == OPEN_PAREN {
                in_url = i > 0 && text[i - 1] == CLOSE_BRACKET;
            }
        }
        in_url && (position..line_end).any(|i| text[i] == CLOSE_PAREN && self.region_at(i) == PROSE)
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
                .get_or_init(|| find_unit(self.tail(), LESS).is_some())
        {
            return false;
        }
        let line_start = line_start(text, position);
        let mut in_tag = false;
        for i in line_start..position {
            if self.region_at(i) != PROSE {
                continue;
            }
            if text[i] == GREATER {
                in_tag = false;
            } else if text[i] == LESS {
                in_tag = starts_tag(text, i);
            }
        }
        in_tag
    }

    fn in_code(&self, index: usize) -> bool {
        self.code_at(index as isize) || self.complete_span_at(index as isize)
    }
}

/// The math mask of `text` from `context`, and the context at its end.
fn math_mask(text: &[u16], regions: &[u8], mut context: MathContext) -> (Vec<u8>, MathContext) {
    let n = text.len();
    let mut mask = vec![0; n];
    let mut i = 0;
    while i < n {
        // Only `\` and `$` can change the context: the units before the next one take
        // the current context.
        let next = text[i..]
            .iter()
            .position(|&unit| unit == BACKSLASH || unit == DOLLAR)
            .map_or(n, |offset| i + offset);
        mask[i..next].fill(u8::from(context != MathContext::None));
        i = next;
        if i >= n {
            break;
        }
        mask[i] = u8::from(context != MathContext::None);
        let delimiter = if regions[i] == PROSE {
            let next = text.get(i + 1).copied();
            if text[i] == BACKSLASH {
                if next == Some(DOLLAR) {
                    Some((context, 2))
                } else {
                    let latex = match (next, context) {
                        (Some(OPEN_BRACKET), MathContext::None) => Some(MathContext::BlockLatex),
                        (Some(CLOSE_BRACKET), MathContext::BlockLatex) => Some(MathContext::None),
                        (Some(OPEN_PAREN), MathContext::None) => Some(MathContext::InlineLatex),
                        (Some(CLOSE_PAREN), MathContext::InlineLatex) => Some(MathContext::None),
                        _ => None,
                    };
                    latex.map(|latex| (latex, 2))
                }
            } else if text[i] == DOLLAR
                && !matches!(context, MathContext::InlineLatex | MathContext::BlockLatex)
            {
                let block = next == Some(DOLLAR);
                let after = if block {
                    if context == MathContext::BlockDollar {
                        MathContext::None
                    } else {
                        MathContext::BlockDollar
                    }
                } else if context == MathContext::BlockDollar {
                    context
                } else if context == MathContext::InlineDollar {
                    MathContext::None
                } else {
                    MathContext::InlineDollar
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
            mask[i + 1] = u8::from(context != MathContext::None);
        }
        i += length;
    }
    (mask, context)
}

// -------------------------------------------------------------------------------------
// What the handlers read, from a position on
// -------------------------------------------------------------------------------------

/// `countDoublePairs` from `from`: `**`, `~~` or `$$` in prose.
fn count_double_pairs(view: &View<'_>, unit: u16, from: usize) -> usize {
    let text = view.text;
    let pair = [unit, unit];
    let mut count = 0;
    let mut cursor = index_of(text, &pair, from);
    while let Some(i) = cursor {
        let next = if view.region_at(i) == PROSE {
            count += 1;
            i + 2
        } else {
            i + 1
        };
        cursor = index_of(text, &pair, next);
    }
    count
}

/// emphasis-handlers.ts `shouldSkipAsterisk`.
fn skip_asterisk(view: &View<'_>, index: usize, previous: Option<u16>, next: Option<u16>) -> bool {
    // `shouldSkipAsterisk` is an or of its tests; the math test, which needs the math
    // mask, goes last.
    let skip = if previous == Some(BACKSLASH) {
        true
    } else if previous != Some(STAR) && next == Some(STAR) {
        let after = view.text.get(index + 2).copied();
        after != Some(STAR)
    } else if previous == Some(STAR) {
        true
    } else {
        previous.is_none_or(whitespace_unit) && next.is_none_or(whitespace_unit)
    };
    skip || view.in_math(index as isize)
}

fn whitespace_unit(unit: u16) -> bool {
    matches!(unit, SPACE | TAB | NEWLINE)
}

/// `countSingleAsterisks` from `from`, where the count so far is `count` (and no
/// chain is open: the unit before `from` is a line ending, or there is none).
fn count_single_asterisks(view: &View<'_>, from: usize, mut count: usize) -> usize {
    let text = view.text;
    let mut chain = false;
    // Only stars count; a unit that is not a word unit (a star outside prose included)
    // breaks the chain, so the units between stars are read only while one is open.
    let mut after = from;
    while let Some(index) = index_of_unit(text, STAR, after) {
        if chain && !text[after..index].iter().all(|&unit| word_unit(Some(unit))) {
            chain = false;
        }
        after = index + 1;
        if view.region_at(index) != PROSE {
            chain = false;
            continue;
        }
        let previous = at(text, index as isize - 1);
        let next = text.get(index + 1).copied();
        if skip_asterisk(view, index, previous, next) {
            continue;
        }
        let internal = word_unit(previous) && word_unit(next);
        let can_open = next.is_some_and(|unit| !whitespace_unit(unit));
        let can_close = previous.is_some_and(|unit| !whitespace_unit(unit));
        if internal && count.is_multiple_of(2) && !chain {
            continue;
        }
        if (can_close && count % 2 == 1) || can_open {
            count += 1;
            chain = internal;
        }
    }
    count
}

fn count_single_underscores(view: &View<'_>, from: usize) -> usize {
    let text = view.text;
    let mut count = 0;
    let mut cursor = index_of_unit(text, UNDERSCORE, from);
    while let Some(index) = cursor {
        cursor = index_of_unit(text, UNDERSCORE, index + 1);
        if view.region_at(index) != PROSE {
            continue;
        }
        let previous = at(text, index as isize - 1);
        let next = text.get(index + 1).copied();
        // An or of `shouldSkipUnderscore`'s tests, the ones needing masks last.
        let skip = previous == Some(BACKSLASH)
            || previous == Some(UNDERSCORE)
            || next == Some(UNDERSCORE)
            || (word_unit(previous) && word_unit(next))
            || view.in_math(index as isize)
            || view.in_link_url(index as isize)
            || view.in_html_tag(index as isize);
        if !skip {
            count += 1;
        }
    }
    count
}

fn count_triple_asterisks(view: &View<'_>, from: usize) -> usize {
    let mut count = 0;
    let mut run = 0;
    for (index, &unit) in view.text.iter().enumerate().skip(from) {
        if unit == STAR && view.region_at(index) == PROSE {
            run += 1;
        } else {
            count += run / 3;
            run = 0;
        }
    }
    count + run / 3
}

/// `hasUnmatchedDoubleUnderscore` from `from`: whether an odd number of runs count.
fn unmatched_double_underscore(view: &View<'_>, from: usize) -> bool {
    let text = view.text;
    let n = text.len();
    // `isThematicBreakRun`'s memo: the line end and verdict.
    let mut memo: (isize, bool) = (-1, false);
    let mut unmatched = false;
    let mut i = from;
    while i < n {
        if text[i] != UNDERSCORE || view.region_at(i) != PROSE {
            i += 1;
            continue;
        }
        let first = i;
        let mut end = i + 1;
        while end < n && text[end] == UNDERSCORE && view.region_at(end) == PROSE {
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
        let previous = if escaped {
            Some(BACKSLASH)
        } else {
            at(text, start as isize - 1)
        };
        let next = text.get(end).copied();
        if word_unit(previous) && word_unit(next) {
            continue;
        }
        if previous.is_none_or(whitespace_unit) && next.is_none_or(whitespace_unit) {
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
        if view.in_math(start as isize)
            || view.in_link_url(start as isize)
            || view.in_html_tag(start as isize)
        {
            continue;
        }
        if ((end - start) / 2) % 2 == 1 {
            unmatched = !unmatched;
        }
    }
    unmatched
}

fn first_single_asterisk(view: &View<'_>, from: usize) -> Option<usize> {
    let text = view.text;
    let mut cursor = index_of_unit(text, STAR, from);
    while let Some(i) = cursor {
        cursor = index_of_unit(text, STAR, i + 1);
        let previous = at(text, i as isize - 1);
        let next = text.get(i + 1).copied();
        let lone = view.region_at(i) == PROSE
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
        if view.in_math(i as isize) {
            continue;
        }
        return Some(i);
    }
    None
}

fn first_single_underscore(view: &View<'_>, from: usize) -> Option<usize> {
    let text = view.text;
    let mut cursor = index_of_unit(text, UNDERSCORE, from);
    while let Some(i) = cursor {
        cursor = index_of_unit(text, UNDERSCORE, i + 1);
        let previous = at(text, i as isize - 1);
        let next = text.get(i + 1).copied();
        // Word-internal underscores are skipped either way; testing them first spares
        // the masks.
        if view.region_at(i) == PROSE
            && previous != Some(UNDERSCORE)
            && next != Some(UNDERSCORE)
            && previous != Some(BACKSLASH)
            && !(previous.is_some() && next.is_some() && word_unit(previous) && word_unit(next))
            && !view.in_math(i as isize)
            && !view.in_link_url(i as isize)
        {
            return Some(i);
        }
    }
    None
}

fn starts_tag(text: &[u16], index: usize) -> bool {
    text.get(index + 1)
        .is_some_and(|&next| (next < 0x80 && (next as u8).is_ascii_alphabetic()) || next == SLASH)
}

/// The first `<` from `from` that starts a tag outside code and math (what
/// html-tag-handler drops from).
fn first_tag_start(view: &View<'_>, from: usize) -> Option<usize> {
    let text = view.text;
    let mut cursor = index_of_unit(text, LESS, from);
    while let Some(index) = cursor {
        cursor = index_of_unit(text, LESS, index + 1);
        if starts_tag(text, index) && !view.code_at(index as isize) && !view.in_math(index as isize)
        {
            return Some(index);
        }
    }
    None
}

/// The `[`s from `from` with no `]` to close them (`findMatchingClosingBracket`, which
/// counts every bracket), pushed onto `open`; and for each `](`, the `[` it closes
/// (`findMatchingOpeningBracket`), the last one into `last_link`.
/// A stack of open `[`s: the settled prefix's, less those closed since, then the ones
/// opened after it.
struct Brackets<'p> {
    settled: &'p [usize],
    /// How many of `settled` (from its end) are closed.
    closed: usize,
    after: Vec<usize>,
}

impl<'p> Brackets<'p> {
    fn new(settled: &'p [usize]) -> Self {
        Brackets {
            settled,
            closed: 0,
            after: Vec::new(),
        }
    }

    fn push(&mut self, index: usize) {
        self.after.push(index);
    }

    fn pop(&mut self) -> Option<usize> {
        self.after.pop().or_else(|| {
            let open = self.settled.len() - self.closed;
            (open > 0).then(|| {
                self.closed += 1;
                self.settled[open - 1]
            })
        })
    }

    /// The open ones, last first.
    fn rev(&self) -> impl Iterator<Item = usize> + '_ {
        let open = self.settled.len() - self.closed;
        self.after
            .iter()
            .rev()
            .chain(self.settled[..open].iter().rev())
            .copied()
    }

    /// The open ones, first first.
    fn into_open(self) -> Vec<usize> {
        let mut open = self.settled[..self.settled.len() - self.closed].to_vec();
        open.extend(self.after);
        open
    }
}

fn pair_brackets(
    text: &[u16],
    from: usize,
    open: &mut Brackets<'_>,
    last_link: &mut Option<(usize, Option<usize>)>,
) {
    for_each_unit2(&text[from..], OPEN_BRACKET, CLOSE_BRACKET, |offset| {
        let index = from + offset;
        if text[index] == OPEN_BRACKET {
            open.push(index);
        } else {
            let matched = open.pop();
            if text.get(index + 1) == Some(&OPEN_PAREN) {
                *last_link = Some((index, matched));
            }
        }
    });
}

/// `lastIndexOf(pattern)` with no start limit, in `text[from..]`.
fn last_index_from(text: &[u16], pattern: &[u16], from: usize) -> Option<usize> {
    last_index_of(&text[from..], pattern, None).map(|index| index + from)
}

// -------------------------------------------------------------------------------------
// The settled prefix
// -------------------------------------------------------------------------------------

/// What the handlers read from a settled prefix of their text, folded as it grows.
#[derive(Clone, Debug, Default)]
struct Settled {
    len: usize,
    fence: Option<OpenFence>,
    math: MathContext,
    /// `**`, `~~` and `$$` pairs.
    pairs: [usize; 3],
    single_asterisks: usize,
    single_underscores: usize,
    triple_asterisks: usize,
    double_underscore_odd: bool,
    first_single_asterisk: Option<usize>,
    first_single_underscore: Option<usize>,
    first_double_star: Option<usize>,
    first_double_dollar: Option<usize>,
    /// The last `*`, `_` and `~`.
    last_unit: [Option<usize>; 3],
    /// The last `**`, `__`, `~~` and `***`.
    last_run: [Option<usize>; 4],
    last_close_paren: Option<usize>,
    last_greater: Option<usize>,
    /// The last unit `whitespace_or_markers` does not accept.
    last_content: Option<usize>,
    open_brackets: Vec<usize>,
    last_link: Option<(usize, Option<usize>)>,
    /// The first tag start after the last `>`.
    tag_start: Option<usize>,
}

const MARKERS: [u16; 3] = [STAR, UNDERSCORE, TILDE];
const RUNS: [&[u16]; 4] = [
    &[STAR, STAR],
    &[UNDERSCORE, UNDERSCORE],
    &[TILDE, TILDE],
    &[STAR, STAR, STAR],
];

fn later(old: Option<usize>, new: Option<usize>) -> Option<usize> {
    new.or(old)
}

impl Settled {
    /// This prefix grown to `text` (whose first `from` units are this prefix), with
    /// the regions of `text[from..]` for [`Settled::commit`]; `None` when `text` does
    /// not end where a prefix can (an inline code span or math still open).
    fn extended(&self, text: &[u16], from: usize, head: &[u8]) -> Option<(Settled, Vec<u8>)> {
        let scan = TailScan::new(&text[from..], self.fence);
        if scan.open_span.is_some() {
            return None;
        }
        let (mask, math) = math_mask(&text[from..], &scan.regions, self.math);
        if math != MathContext::None || self.math != MathContext::None {
            return None;
        }
        let view = View::new(text, from, head, &scan);
        // The mask from a context of none is what `View` builds (only text with math
        // delimiters can set it).
        view.math.set(mask).expect("a new view");
        let mut next = Settled {
            len: text.len(),
            fence: scan.open_fence,
            math,
            pairs: [0; 3],
            single_asterisks: count_single_asterisks(&view, from, self.single_asterisks),
            single_underscores: self.single_underscores + count_single_underscores(&view, from),
            triple_asterisks: self.triple_asterisks + count_triple_asterisks(&view, from),
            double_underscore_odd: self.double_underscore_odd
                ^ unmatched_double_underscore(&view, from),
            first_single_asterisk: self
                .first_single_asterisk
                .or_else(|| first_single_asterisk(&view, from)),
            first_single_underscore: self
                .first_single_underscore
                .or_else(|| first_single_underscore(&view, from)),
            first_double_star: self
                .first_double_star
                .or_else(|| index_of(text, &[STAR, STAR], from)),
            first_double_dollar: self
                .first_double_dollar
                .or_else(|| index_of(text, &[DOLLAR, DOLLAR], from)),
            last_unit: [None; 3],
            last_run: [None; 4],
            last_close_paren: later(
                self.last_close_paren,
                last_index_from(text, &[CLOSE_PAREN], from),
            ),
            last_greater: later(self.last_greater, last_index_from(text, &[GREATER], from)),
            last_content: later(
                self.last_content,
                text[from..]
                    .iter()
                    .rposition(|&unit| !whitespace_or_markers(&[unit]))
                    .map(|index| index + from),
            ),
            open_brackets: Vec::new(),
            last_link: self.last_link,
            tag_start: None,
        };
        for (index, unit) in [STAR, TILDE, DOLLAR].into_iter().enumerate() {
            next.pairs[index] = self.pairs[index] + count_double_pairs(&view, unit, from);
        }
        for (index, unit) in MARKERS.into_iter().enumerate() {
            next.last_unit[index] =
                later(self.last_unit[index], last_index_from(text, &[unit], from));
        }
        for (index, run) in RUNS.into_iter().enumerate() {
            next.last_run[index] = later(self.last_run[index], last_index_from(text, run, from));
        }
        let mut open = Brackets::new(&self.open_brackets);
        pair_brackets(text, from, &mut open, &mut next.last_link);
        next.open_brackets = open.into_open();
        next.tag_start = match last_index_from(text, &[GREATER], from) {
            Some(greater) => first_tag_start(&view, greater + 1),
            None => self.tag_start.or_else(|| first_tag_start(&view, from)),
        };
        Some((next, scan.regions))
    }
}

// -------------------------------------------------------------------------------------
// The handlers' text
// -------------------------------------------------------------------------------------

/// A handler's text, `prefix.len` units of it settled, and the scan of the rest.
struct Ctx<'p> {
    text: Units,
    prefix: &'p Settled,
    /// The settled prefix's regions.
    head: &'p [u8],
    scan: TailScan,
}

impl<'p> Ctx<'p> {
    fn split(&self) -> usize {
        self.prefix.len
    }

    fn new(text: Units, prefix: &'p Settled, head: &'p [u8]) -> Ctx<'p> {
        let scan = TailScan::new(&text[prefix.len..], prefix.fence);
        Ctx {
            text,
            prefix,
            head,
            scan,
        }
    }

    fn view(&self) -> View<'_> {
        View::new(&self.text, self.prefix.len, self.head, &self.scan)
    }

    /// Cuts the text at `at` and appends `suffix`; `None` (with no change) when `at`
    /// is inside the settled prefix.
    fn edit(&mut self, at: usize, suffix: &[u16]) -> Option<()> {
        if at < self.prefix.len {
            return None;
        }
        self.text.truncate(at);
        self.text.extend_from_slice(suffix);
        self.scan = TailScan::new(&self.text[self.prefix.len..], self.prefix.fence);
        Some(())
    }

    fn append(&mut self, suffix: &str) -> Option<()> {
        let at = self.text.len();
        self.edit(at, &units(suffix))
    }

    /// `lastIndexOf(pattern)`, limited to starts at or before `limit`.
    fn last_index(
        &self,
        pattern: &[u16],
        limit: Option<usize>,
        settled: Option<usize>,
    ) -> Option<usize> {
        let split = self.split();
        let text = &self.text;
        let tail = match limit {
            Some(limit) if limit < split => None,
            Some(limit) => last_index_of(&text[split..], pattern, Some((limit - split) as isize))
                .map(|index| index + split),
            None => last_index_from(text, pattern, split),
        };
        tail.or(settled)
    }

    /// `whitespace_or_markers(text[from..])`.
    fn blank_from(&self, from: usize) -> bool {
        let split = self.split();
        if from < split && self.prefix.last_content.is_some_and(|last| last >= from) {
            return false;
        }
        whitespace_or_markers(&self.text[from.max(split)..])
    }

    /// Whether `text[from..]` holds a line ending.
    fn newline_from(&self, from: usize) -> bool {
        // The settled prefix ends with one.
        from < self.split() || self.text[from..].contains(&NEWLINE)
    }

    fn pairs(&self, index: usize, unit: u16) -> usize {
        let split = self.split();
        let settled = self.prefix.pairs[index];
        settled + count_double_pairs(&self.view(), unit, split)
    }

    fn single_asterisks(&self) -> usize {
        let split = self.split();
        let settled = self.prefix.single_asterisks;
        count_single_asterisks(&self.view(), split, settled)
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

/// `matchTrailing`: where the content group of the leftmost match starts and ends.
fn match_trailing(
    ctx: &Ctx<'_>,
    delimiter: u16,
    marker_length: usize,
    tail: Tail,
) -> Option<std::ops::Range<usize>> {
    let text = &ctx.text;
    let n = text.len();
    let marker = MARKERS
        .iter()
        .position(|&unit| unit == delimiter)
        .expect("a marker");
    let last = ctx.last_index(
        &[delimiter],
        Some(n.saturating_sub(2)),
        ctx.prefix.last_unit[marker],
    );
    let marker_end = last.map_or(n as isize - 1, |last| last as isize);
    let start = usize::try_from((marker_end - marker_length as isize + 1).max(0)).unwrap_or(0);
    let slice = text.get(start..).unwrap_or(&[]);
    let len = slice.len();
    // `last` is the slice's last delimiter before its final unit, so a match's rest
    // holds a delimiter only as that final unit, and a marker lies within
    // `marker_length` of `last` or of the end: only those starts can match.
    let before_final = last.map(|last| last - start);
    let final_delimiter = slice.last() == Some(&delimiter);
    let first = before_final.map_or(0, |at| (at + 1).saturating_sub(marker_length));
    let near_last = first..before_final.map_or(first, |at| (at + 1).min(len));
    let near_end = len.saturating_sub(marker_length).max(near_last.end)..len;
    for p in near_last.chain(near_end) {
        if p + marker_length > len
            || !slice[p..p + marker_length]
                .iter()
                .all(|&unit| unit == delimiter)
        {
            continue;
        }
        let rest_start = p + marker_length;
        if before_final.is_some_and(|at| at >= rest_start) {
            continue;
        }
        let rest_len = len - rest_start;
        let rest_has_final = final_delimiter && rest_len > 0;
        let matched = match tail {
            Tail::Plain => !rest_has_final,
            Tail::OptionalStar => true,
            Tail::HalfComplete => rest_has_final && rest_len >= 2,
        };
        if matched {
            let content_end = match tail {
                Tail::HalfComplete => len - 1,
                _ => len,
            };
            return Some(start + rest_start..start + content_end);
        }
    }
    None
}

/// `shouldSkipBoldCompletion` / `shouldSkipItalicCompletion` (which differ in the rule's
/// marker), for the content `text[content..]`.
fn skip_completion(ctx: &Ctx<'_>, content: usize, marker_index: usize, rule: u16) -> bool {
    let text = &ctx.text;
    if content == text.len() || ctx.blank_from(content) {
        return true;
    }
    let line_start = line_start(text, marker_index);
    if list_item_prefix(&text[line_start..marker_index]) && ctx.newline_from(content) {
        return true;
    }
    is_horizontal_rule(text, marker_index, rule)
}

// -------------------------------------------------------------------------------------
// Handlers, in remend's priority order
// -------------------------------------------------------------------------------------

/// single-tilde-handler.ts (`20~25` → `20\~25`) on `text[from..]`, which starts at a
/// line start with `fence` open and no inline code span: its output for that range.
/// The escape needs only the units around the tilde and whether it is in code.
fn single_tilde(text: &[u16], from: usize, fence: Option<OpenFence>) -> Units {
    let text = &text[from..];
    // `/([\p{L}\p{N}_])~(?!~)(?=[\p{L}\p{N}_])/gu`: a match is a word code point, a
    // `~`, and a word code point after it (so not a `~`), and its `~` is escaped in
    // prose. A match consumes the code point before its `~` and the `~`, never the code
    // point before the next one, so each `~` is decided on its own: only the tildes are
    // visited (not a table lookup per code point), and the regions are painted once a
    // match needs them.
    let mut scan: Option<TailScan> = None;
    let mut out: Option<Units> = None;
    let mut copied = 0;
    let mut at = 0;
    while let Some(offset) = text[at..].iter().position(|&unit| unit == TILDE) {
        let tilde = at + offset;
        at = tilde + 1;
        if tilde == 0 || tilde + 1 >= text.len() || text[tilde + 1] == TILDE {
            continue;
        }
        if !(word_code_point(code_point_before(text, tilde))
            && word_code_point(code_point_at(text, tilde + 1).0))
        {
            continue;
        }
        let regions = &scan
            .get_or_insert_with(|| TailScan::new(text, fence))
            .regions;
        if regions[tilde] == PROSE {
            let out = out.get_or_insert_with(|| Vec::with_capacity(text.len() + 4));
            out.extend_from_slice(&text[copied..tilde]);
            out.push(BACKSLASH);
            out.push(TILDE);
            copied = tilde + 1;
        }
    }
    match out {
        Some(mut out) => {
            out.extend_from_slice(&text[copied..]);
            out
        }
        None => text.to_vec(),
    }
}

/// The code point that ends right before `index` (`index > 0`), decoded as a scan from
/// the start decodes it (UTF-16 resynchronizes at every unit).
fn code_point_before(text: &[u16], index: usize) -> u32 {
    let unit = text[index - 1];
    if (0xdc00..0xe000).contains(&unit) && index >= 2 && (0xd800..0xdc00).contains(&text[index - 2])
    {
        return code_point_at(text, index - 2).0;
    }
    u32::from(unit)
}

/// comparison-operator-handler.ts (`- > 25` → `- \> 25`) on `text[from..]`, which
/// starts at a line start with `fence` open: its output for that range, and whether
/// a match was still possible at the range's end (more text could complete one).
fn comparison_operators(text: &[u16], from: usize, fence: Option<OpenFence>) -> (Units, bool) {
    let text = &text[from..];
    let n = text.len();
    if !text.contains(&GREATER) {
        return (text.to_vec(), false);
    }
    let mut open = false;
    // `/^(\s*(?:[-*+]|\d+[.)]) +)>(=?\s*[$]?\d)/gm`: the position of `>` and the end.
    let mut attempt = |p: usize| -> Option<(usize, usize)> {
        let mut at_end = |i: usize| {
            if i >= n {
                open = true;
            }
            i >= n
        };
        let mut i = p;
        while i < n && js_space(text[i]) {
            i += 1;
        }
        // Blank to the end: the same attempt as one from where more text starts.
        if i >= n {
            return None;
        }
        match text[i] {
            DASH | STAR | PLUS => i += 1,
            _ => {
                let digits = i;
                while digit(text.get(i).copied()) {
                    i += 1;
                }
                if i == digits {
                    return None;
                }
                if at_end(i) || !matches!(text[i], DOT | CLOSE_PAREN) {
                    return None;
                }
                i += 1;
            }
        }
        let spaces = i;
        while text.get(i) == Some(&SPACE) {
            i += 1;
        }
        if at_end(i) || i == spaces || text[i] != GREATER {
            return None;
        }
        let greater = i;
        i += 1;
        if text.get(i) == Some(&EQUALS) {
            i += 1;
        }
        while i < n && js_space(text[i]) {
            i += 1;
        }
        if text.get(i) == Some(&DOLLAR) {
            i += 1;
        }
        if at_end(i) {
            return None;
        }
        digit(Some(text[i])).then_some((greater, i + 1))
    };
    let mut scan: Option<TailScan> = None;
    let mut escapes = Vec::new();
    // Without U+2028 or U+2029, lines end only at `\n` or `\r`, which memchr finds.
    let separators = text.iter().any(|&unit| matches!(unit, 0x2028 | 0x2029));
    let mut p = 0;
    while p <= n {
        let line_start = p == 0 || line_terminator(text[p - 1]);
        if let Some((greater, end)) = line_start.then(|| attempt(p)).flatten() {
            let scan = scan.get_or_insert_with(|| TailScan::new(text, fence));
            if !(p < n && scan.regions[p] != PROSE)
                && !(p >= n && (scan.open_fence.is_some() || scan.open_span.is_some()))
            {
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
    let mut out = Vec::with_capacity(n + escapes.len());
    let mut copied = 0;
    for greater in escapes {
        out.extend_from_slice(&text[copied..greater]);
        out.push(BACKSLASH);
        copied = greater;
    }
    out.extend_from_slice(&text[copied..]);
    (out, open)
}

/// html-tag-handler.ts: drops an incomplete trailing tag (`text <custom`).
fn incomplete_html_tag(ctx: &mut Ctx<'_>) -> Option<()> {
    let split = ctx.split();
    // `/<[a-zA-Z/][^>]*$/` from the first such `<` after the last `>`, outside code
    // and math (the mask of a text with no math delimiter is empty).
    let tail_greater = rfind_unit(&ctx.text[split..], GREATER).map(|index| index + split);
    let found = match tail_greater {
        Some(greater) => first_tag_start(&ctx.view(), greater + 1),
        None => ctx
            .prefix
            .tag_start
            .or_else(|| first_tag_start(&ctx.view(), split)),
    };
    match found {
        Some(index) => {
            let end = trim_end(&ctx.text[..index]).len();
            ctx.edit(end, &[])
        }
        None => Some(()),
    }
}

/// setext-heading-handler.ts: breaks a `-`/`=` line that would make a heading.
fn incomplete_setext_heading(ctx: &mut Ctx<'_>) -> Option<()> {
    let text = &ctx.text;
    let Some(last_newline) = rfind_unit(text, NEWLINE) else {
        return Some(());
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
            let previous_line =
                rfind_unit(previous, NEWLINE).map_or(previous, |index| &previous[index + 1..]);
            if !previous_line.is_empty() && !trim(previous_line).is_empty() {
                return ctx.append("\u{200b}");
            }
        }
    }
    Some(())
}

/// link-image-handler.ts, `linkMode: "protocol"`.
fn incomplete_links_and_images(ctx: &mut Ctx<'_>) -> Option<()> {
    for _ in 0..32 {
        let before = ctx.text.len();
        let shorter = heal_trailing_link_or_image(ctx)?;
        if !shorter || ctx.text.len() >= before {
            return Some(());
        }
    }
    Some(())
}

/// One pass of the links handler; whether it shortened the text (`Some(false)` when it
/// lengthened or kept it).
fn heal_trailing_link_or_image(ctx: &mut Ctx<'_>) -> Option<bool> {
    let split = ctx.split();
    let paren = ctx.last_index(
        &[CLOSE_BRACKET, OPEN_PAREN],
        None,
        ctx.prefix.last_link.map(|(paren, _)| paren),
    );
    // The brackets after the settled prefix, from the prefix's open ones.
    let mut open = Brackets::new(&ctx.prefix.open_brackets);
    let mut last_link = ctx.prefix.last_link;
    pair_brackets(&ctx.text, split, &mut open, &mut last_link);
    if let Some(paren) = paren {
        let view = ctx.view();
        let text = &ctx.text;
        let closed = if paren + 2 < split {
            ctx.prefix
                .last_close_paren
                .is_some_and(|close| close >= paren + 2)
                || text[split..].contains(&CLOSE_PAREN)
        } else {
            text[paren + 2..].contains(&CLOSE_PAREN)
        };
        // `findMatchingOpeningBracket`: the `[` the forward pairing gave this `]`.
        let matched = last_link
            .filter(|(at, _)| *at == paren)
            .and_then(|(_, open)| open)
            .filter(|&open| !view.code_at(open as isize));
        if !view.code_at(paren as isize)
            && !closed
            && let Some(open) = matched
        {
            let image = open > 0 && text[open - 1] == BANG;
            let target = if image {
                INCOMPLETE_IMAGE
            } else {
                INCOMPLETE_LINK
            };
            let before = text.len();
            ctx.edit(paren, &units(target))?;
            return Some(ctx.text.len() < before);
        }
    }
    // The last `[` outside code with no matching `]`.
    let view = ctx.view();
    let Some(i) = open.rev().find(|&i| !view.code_at(i as isize)) else {
        return Some(false);
    };
    let image = i > 0 && ctx.text[i - 1] == BANG;
    ctx.append(if image {
        INCOMPLETE_IMAGE
    } else {
        INCOMPLETE_LINK
    })?;
    Some(false)
}

fn incomplete_bold(ctx: &mut Ctx<'_>) -> Option<()> {
    let Some(content) = match_trailing(ctx, STAR, 2, Tail::OptionalStar) else {
        return Some(());
    };
    let marker = ctx
        .last_index(&[STAR, STAR], None, ctx.prefix.last_run[0])
        .expect("the match holds **");
    if ctx.view().in_code(marker) || skip_completion(ctx, content.start, marker, STAR) {
        return Some(());
    }
    if ctx.pairs(0, STAR) % 2 == 1 {
        let suffix = if ctx.text.last() == Some(&STAR) {
            "*"
        } else {
            "**"
        };
        return ctx.append(suffix);
    }
    Some(())
}

fn unmatched_double_underscores(ctx: &Ctx<'_>) -> bool {
    let split = ctx.split();
    let settled = ctx.prefix.double_underscore_odd;
    settled ^ unmatched_double_underscore(&ctx.view(), split)
}

fn incomplete_double_underscore_italic(ctx: &mut Ctx<'_>) -> Option<()> {
    let (content, suffix) = match match_trailing(ctx, UNDERSCORE, 2, Tail::Plain) {
        Some(content) => (Some(content), "__"),
        None if match_trailing(ctx, UNDERSCORE, 2, Tail::HalfComplete).is_some() => (None, "_"),
        None => return Some(()),
    };
    let marker = ctx
        .last_index(&[UNDERSCORE, UNDERSCORE], None, ctx.prefix.last_run[1])
        .expect("the match holds __");
    if ctx.view().in_code(marker)
        || content.is_some_and(|content| skip_completion(ctx, content.start, marker, UNDERSCORE))
    {
        return Some(());
    }
    if unmatched_double_underscores(ctx) {
        return ctx.append(suffix);
    }
    Some(())
}

fn incomplete_single_asterisk_italic(ctx: &mut Ctx<'_>) -> Option<()> {
    if match_trailing(ctx, STAR, 1, Tail::Plain).is_none() {
        return Some(());
    }
    let split = ctx.split();
    let settled = ctx.prefix.first_single_asterisk;
    let Some(first) = settled.or_else(|| first_single_asterisk(&ctx.view(), split)) else {
        return Some(());
    };
    if ctx.view().in_code(first) {
        return Some(());
    }
    if first + 1 == ctx.text.len() || ctx.blank_from(first + 1) {
        return Some(());
    }
    if ctx.single_asterisks() % 2 == 1 {
        return ctx.append("*");
    }
    Some(())
}

fn first_underscore(ctx: &Ctx<'_>) -> Option<usize> {
    let split = ctx.split();
    let settled = ctx.prefix.first_single_underscore;
    settled.or_else(|| first_single_underscore(&ctx.view(), split))
}

fn incomplete_single_underscore_italic(ctx: &mut Ctx<'_>) -> Option<()> {
    if match_trailing(ctx, UNDERSCORE, 1, Tail::Plain).is_none() {
        return Some(());
    }
    let Some(first) = first_underscore(ctx) else {
        return Some(());
    };
    if first + 1 == ctx.text.len() || ctx.blank_from(first + 1) {
        return Some(());
    }
    if ctx.view().in_code(first) {
        return Some(());
    }
    let split = ctx.split();
    let settled = ctx.prefix.single_underscores;
    if (settled + count_single_underscores(&ctx.view(), split)).is_multiple_of(2) {
        return Some(());
    }
    // `handleTrailingAsterisksForUnderscore`: close `_` before a `**` that closed an
    // earlier one.
    if ctx.text.ends_with(&[STAR, STAR]) && ctx.text.len() - 2 >= split {
        let without = Ctx::new(
            ctx.text[..ctx.text.len() - 2].to_vec(),
            ctx.prefix,
            ctx.head,
        );
        if without.pairs(0, STAR) % 2 == 1 {
            let bold = ctx
                .prefix
                .first_double_star
                .or_else(|| index_of(&without.text, &[STAR, STAR], split));
            let underscore = first_underscore(&without);
            if let (Some(bold), Some(underscore)) = (bold, underscore)
                && bold < underscore
            {
                let at = without.text.len();
                return ctx.edit(at, &units("_**"));
            }
        }
    }
    // `insertClosingUnderscore`: before any trailing newlines.
    let end = ctx
        .text
        .iter()
        .rposition(|&unit| unit != NEWLINE)
        .map_or(0, |index| index + 1);
    let mut rest = vec![UNDERSCORE];
    rest.extend_from_slice(&ctx.text[end..]);
    ctx.edit(end, &rest)
}

fn incomplete_bold_italic(ctx: &mut Ctx<'_>) -> Option<()> {
    if ctx.text.len() >= 4 && ctx.text.iter().all(|&unit| unit == STAR) {
        return Some(());
    }
    let Some(content) = match_trailing(ctx, STAR, 3, Tail::Plain) else {
        return Some(());
    };
    let marker = ctx
        .last_index(&[STAR, STAR, STAR], None, ctx.prefix.last_run[3])
        .expect("the match holds ***");
    if content.is_empty() || ctx.blank_from(content.start) {
        return Some(());
    }
    if ctx.view().in_code(marker) || is_horizontal_rule(&ctx.text, marker, STAR) {
        return Some(());
    }
    let split = ctx.split();
    let triples = ctx.prefix.triple_asterisks + count_triple_asterisks(&ctx.view(), split);
    if triples % 2 == 1 {
        let balanced =
            ctx.pairs(0, STAR).is_multiple_of(2) && ctx.single_asterisks().is_multiple_of(2);
        if balanced {
            return Some(());
        }
        return ctx.append("***");
    }
    Some(())
}

fn incomplete_inline_code(ctx: &mut Ctx<'_>) -> Option<()> {
    let view = ctx.view();
    if view.scan.open_fence.is_some() {
        return Some(());
    }
    let Some(span) = view.scan.open_span else {
        return Some(());
    };
    let split = ctx.split();
    let content = split + span.start + span.run_length;
    if content >= ctx.text.len() || whitespace_or_markers(&ctx.text[content..]) {
        return Some(());
    }
    let trailing = ctx
        .text
        .iter()
        .rev()
        .take_while(|&&unit| unit == BACKTICK)
        .count();
    if trailing >= span.run_length {
        return Some(());
    }
    let at = ctx.text.len();
    ctx.edit(at, &vec![BACKTICK; span.run_length - trailing])
}

fn incomplete_strikethrough(ctx: &mut Ctx<'_>) -> Option<()> {
    let suffix = match match_trailing(ctx, TILDE, 2, Tail::Plain) {
        Some(content) if content.is_empty() || ctx.blank_from(content.start) => return Some(()),
        Some(_) => "~~",
        None if match_trailing(ctx, TILDE, 2, Tail::HalfComplete).is_some() => "~",
        None => return Some(()),
    };
    let marker = ctx
        .last_index(&[TILDE, TILDE], None, ctx.prefix.last_run[2])
        .expect("the match holds ~~");
    if ctx.view().in_code(marker) || ctx.pairs(1, TILDE).is_multiple_of(2) {
        return Some(());
    }
    ctx.append(suffix)
}

fn incomplete_block_katex(ctx: &mut Ctx<'_>) -> Option<()> {
    if ctx.pairs(2, DOLLAR).is_multiple_of(2) {
        return Some(());
    }
    let text = &ctx.text;
    if text.ends_with(&[DOLLAR]) && !text.ends_with(&[DOLLAR, DOLLAR]) {
        return ctx.append("$");
    }
    let split = ctx.split();
    let first = ctx
        .prefix
        .first_double_dollar
        .or_else(|| index_of(text, &[DOLLAR, DOLLAR], split));
    let newline_after = first.is_some_and(|first| ctx.newline_from(first));
    if newline_after && text.last() != Some(&NEWLINE) {
        return ctx.append("\n$$");
    }
    ctx.append("$$")
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
            super::remend_units(&text);
        }
        println!(
            "remend {:>7.1} µs",
            start.elapsed().as_secs_f64() * 1e6 / 500.0
        );
    }
}

#[cfg(test)]
mod stream_cost {
    use std::time::{Duration, Instant};

    /// The time of the last 64 updates of `text` streamed 24 units at a time, best of 3.
    fn last_updates(text: &str) -> Duration {
        let chars: Vec<char> = text.chars().collect();
        let chunks: Vec<String> = chars
            .chunks(24)
            .map(|chunk| chunk.iter().collect())
            .collect();
        let timed = chunks.len() - 64;
        (0..3)
            .map(|_| {
                let mut stream = super::Stream::default();
                let mut start = Instant::now();
                for (index, chunk) in chunks.iter().enumerate() {
                    if index == timed {
                        start = Instant::now();
                    }
                    stream.heal(chunk, index == 0);
                }
                start.elapsed()
            })
            .min()
            .expect("three rounds")
    }

    /// Per-update cost follows the text after the settled prefix, not the whole text:
    /// doubling the text does not double it.
    #[test]
    fn per_update_cost_does_not_grow_with_the_text() {
        let units = [
            "Text with **bold**, *em*, _under_, ~~gone~~, `code`, [a](https://x.y/a_b) and 3 > 2.\n",
            "- item *one* with $5 and a_b\n- [link](url) **two**\n\n",
            "```js\nconst a = [1, 2] > 0 ? '*' : '_';\n```\n\n",
            // A lone `*` and `[` early stay unmatched for the whole text.
            "Lone * star and [ bracket, then more text.\n\n",
        ];
        for unit in units {
            let sizes = [8192, 16384, 32768, 65536];
            let times: Vec<Duration> = sizes
                .iter()
                .map(|&size| last_updates(&unit.repeat(size / unit.len() + 1)))
                .collect();
            let report = format!("{unit:?}: {times:?}");
            for pair in times.windows(2) {
                assert!(
                    pair[1] <= pair[0].mul_f64(2.2) + Duration::from_micros(200),
                    "{report}"
                );
            }
            assert!(
                times[3] <= times[0].mul_f64(4.0) + Duration::from_micros(200),
                "{report}"
            );
        }
    }
}
