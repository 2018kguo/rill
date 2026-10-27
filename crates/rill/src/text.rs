//! The string helpers the stages share.

use std::borrow::Cow;

/// JavaScript's `\s` (what `trim()` removes), for a character or a UTF-16 unit. It is
/// micromark's `unicodeWhitespace` too.
pub(crate) fn js_space(c: impl Into<u32>) -> bool {
    matches!(
        c.into(),
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

/// Whether JavaScript's `trim()` leaves nothing of `text`.
pub(crate) fn js_blank(text: &str) -> bool {
    text.chars().all(js_space)
}

/// The UTF-16 units of the character that starts at `byte`: none for a continuation
/// byte, two for a four-byte (supplementary) character.
pub(crate) fn utf16_units(byte: u8) -> u32 {
    u32::from(byte & 0xC0 != 0x80) + u32::from(byte >= 0xF0)
}

/// `text`'s length in UTF-16 units.
pub(crate) fn utf16_len(text: &str) -> u32 {
    text.bytes().map(utf16_units).sum()
}

/// Whether `c` falls in one of `ranges` (sorted, disjoint, inclusive).
pub(crate) fn in_ranges<T: Copy + Ord>(ranges: &[(T, T)], c: T) -> bool {
    let after = ranges.partition_point(|&(start, _)| start <= c);
    after > 0 && c <= ranges[after - 1].1
}

/// Slices a `Cow`, keeping a borrow borrowed.
pub(crate) fn slice_cow<'s>(value: &Cow<'s, str>, range: std::ops::Range<usize>) -> Cow<'s, str> {
    match value {
        Cow::Borrowed(borrowed) => Cow::Borrowed(&borrowed[range]),
        Cow::Owned(owned) => Cow::Owned(owned[range].to_owned()),
    }
}

/// Appends `value`, copying a borrowed `target` once, into a string that fits both.
pub(crate) fn push_cow(target: &mut Cow<'_, str>, value: &str) {
    match target {
        Cow::Owned(owned) => owned.push_str(value),
        Cow::Borrowed(borrowed) => *target = Cow::Owned([*borrowed, value].concat()),
    }
}
