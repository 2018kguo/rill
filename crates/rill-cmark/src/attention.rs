//! rill: delimiter-run flanking as remark-cjk-friendly 1.2.3 and
//! remark-cjk-friendly-gfm-strikethrough 1.2.3 decide it (micromark-extension-cjk-friendly
//! `tokenizeAttention`, micromark-extension-cjk-friendly-util `classifyCharacter`). Streamdown
//! installs both, so these replace micromark's attention and strikethrough rules for all text.
//! Characters are code points: the extensions recover surrogate pairs.

use crate::unicode_tables::{in_table, CJK, IVS, PUNCTUATION_OR_SYMBOL, WHITESPACE};

const SPACE: u32 = 1;
const PUNCTUATION: u32 = 2;
const SPACE_OR_PUNCTUATION: u32 = 3;
const CJK_BIT: u32 = 4096;
const CJK_PUNCTUATION: u32 = 4098;
const IVS_CLASS: u32 = 8192;
const CJK_OR_IVS: u32 = 12288;
const NON_EMOJI_VS: u32 = 16384;

fn classify(code: Option<char>) -> u32 {
    let Some(c) = code else {
        return SPACE;
    };
    if matches!(c, '\t' | '\n' | '\r' | ' ') || in_table(WHITESPACE, c) {
        return SPACE;
    }
    let cp = c as u32;
    let mut value = 0;
    if cp >= 4352 {
        if (0xFE00..=0xFE0E).contains(&cp) {
            return NON_EMOJI_VS;
        }
        if in_table(IVS, c) {
            return IVS_CLASS;
        }
        if in_table(CJK, c) {
            value |= CJK_BIT;
        }
    }
    if in_table(PUNCTUATION_OR_SYMBOL, c) {
        value |= PUNCTUATION;
    }
    value
}

/// `classifyPrecedingCharacter`: a non-emoji variation selector takes the class of the
/// character it follows.
fn classify_preceding(before: u32, two_previous: Option<char>, previous: Option<char>) -> u32 {
    if before != NON_EMOJI_VS {
        return before;
    }
    let two_before = classify(two_previous);
    if two_previous.is_none() || two_before & SPACE != 0 {
        before
    } else if previous == Some('\u{FE01}')
        && matches!(
            two_previous,
            Some('\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}')
        )
    {
        CJK_PUNCTUATION
    } else {
        two_before & !IVS_CLASS
    }
}

fn non_cjk_punctuation(class: u32) -> bool {
    class & CJK_PUNCTUATION == PUNCTUATION
}

fn space(class: u32) -> bool {
    class & SPACE != 0
}

fn cjk(class: u32) -> bool {
    class & CJK_BIT != 0
}

fn marker(code: Option<char>) -> bool {
    matches!(code, Some('*' | '_' | '~'))
}

/// `(can_open, can_close)` for an `*` or `_` run between `previous` and `next`.
pub(crate) fn emphasis(
    delimiter: u8,
    two_previous: Option<char>,
    previous: Option<char>,
    next: Option<char>,
) -> (bool, bool) {
    let before = classify_preceding(classify(previous), two_previous, previous);
    let after = classify(next);
    let before_punctuation = non_cjk_punctuation(before);
    let before_space_or_punctuation = before_punctuation || space(before);
    let after_punctuation = non_cjk_punctuation(after);
    let after_space_or_punctuation = after_punctuation || space(after);
    let open = !after_space_or_punctuation
        || after_punctuation && (before_space_or_punctuation || before & CJK_OR_IVS != 0)
        || marker(next);
    let close = !before_space_or_punctuation
        || before_punctuation && (after_space_or_punctuation || cjk(after))
        || marker(previous);
    if delimiter == b'*' {
        (open, close)
    } else {
        (
            open && (before & SPACE_OR_PUNCTUATION != 0 || !close),
            close && (after & SPACE_OR_PUNCTUATION != 0 || !open),
        )
    }
}

/// `(can_open, can_close)` for a `~` or `~~` run; longer runs are text.
pub(crate) fn strikethrough(
    run_length: usize,
    two_previous: Option<char>,
    previous: Option<char>,
    next: Option<char>,
) -> Option<(bool, bool)> {
    if run_length > 2 {
        return None;
    }
    let raw_before = classify(previous);
    let before = classify_preceding(raw_before, two_previous, previous);
    let after = classify(next);
    let before_space_or_punctuation = non_cjk_punctuation(before) || space(before);
    let after_space_or_punctuation = non_cjk_punctuation(after) || space(after);
    let open = !after_space_or_punctuation
        || after == PUNCTUATION
            && (before_space_or_punctuation || cjk(before) || raw_before == IVS_CLASS);
    let close = !before_space_or_punctuation
        || raw_before == PUNCTUATION && (after_space_or_punctuation || cjk(after));
    Some((open, close))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_punctuation_closes_strong() {
        // `**粗体**中文`: CJK after the closer still closes.
        assert_eq!(emphasis(b'*', None, Some('体'), Some('中')), (true, true));
        assert_eq!(emphasis(b'_', None, Some(' '), Some('a')), (true, false));
    }
}
