//! rill: port of micromark-extension-gfm-autolink-literal 2.1.0 (`dev/lib/syntax.js`).
//!
//! Each function mirrors one micromark state machine. `previous` is the character
//! micromark's text tokenizer saw before the candidate; `None` is the start of the
//! text or a line ending (both pass every `previous*` check identically).

use crate::unicode_tables::{in_table, PUNCTUATION_OR_SYMBOL, WHITESPACE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiteralKind {
    Email,
    Http,
    Www,
}

/// micromark-util-character `unicodePunctuation` over one UTF-16 code unit: a
/// supplementary character's surrogate halves are not punctuation.
pub fn unicode_punctuation(c: char) -> bool {
    (c as u32) <= 0xFFFF && in_table(PUNCTUATION_OR_SYMBOL, c)
}

/// micromark-util-character `unicodeWhitespace` (no supplementary whitespace exists).
pub fn unicode_whitespace(c: char) -> bool {
    in_table(WHITESPACE, c)
}

fn gfm_atext(c: Option<char>) -> bool {
    matches!(c, Some(c) if c == '+' || c == '-' || c == '.' || c == '_' || c.is_ascii_alphanumeric())
}

fn line_ending_or_space(c: Option<char>) -> bool {
    matches!(c, Some('\t' | '\n' | '\r' | ' '))
}

fn previous_www(c: Option<char>) -> bool {
    matches!(c, None | Some('(' | '*' | '_' | '[' | ']' | '~')) || line_ending_or_space(c)
}

fn previous_protocol(c: Option<char>) -> bool {
    !matches!(c, Some(c) if c.is_ascii_alphabetic())
}

fn previous_email(c: Option<char>) -> bool {
    !(c == Some('/') || gfm_atext(c))
}

/// A cursor over `text` from byte `at`.
#[derive(Clone, Copy)]
struct Cursor<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }
    fn consume(&mut self) {
        if let Some(c) = self.peek() {
            self.at += c.len_utf8();
        }
    }
}

fn whitespace_like(c: Option<char>) -> bool {
    match c {
        None => true,
        Some(c) => line_ending_or_space(Some(c)) || unicode_whitespace(c),
    }
}

/// `tokenizeTrail`: whether the rest is trailing punctuation before whitespace, EOF or `<`.
fn trail(mut cursor: Cursor<'_>) -> bool {
    loop {
        match cursor.peek() {
            Some('!' | '"' | '\'' | ')' | '*' | ',' | '.' | ':' | ';' | '?' | '_' | '~') => {
                cursor.consume()
            }
            Some('&') => {
                cursor.consume();
                if !matches!(cursor.peek(), Some(c) if c.is_ascii_alphabetic()) {
                    return false;
                }
                loop {
                    match cursor.peek() {
                        Some(';') => {
                            cursor.consume();
                            break;
                        }
                        Some(c) if c.is_ascii_alphabetic() => cursor.consume(),
                        _ => return false,
                    }
                }
            }
            Some(']') => {
                cursor.consume();
                let next = cursor.peek();
                if matches!(next, Some('(' | '[')) || whitespace_like(next) {
                    return true;
                }
            }
            Some('<') => return true,
            next if whitespace_like(next) => return true,
            _ => return false,
        }
    }
}

/// `tokenizeDomain`: returns the end of the domain or `None` (nok).
fn domain(mut cursor: Cursor<'_>) -> Option<usize> {
    let mut underscore_in_last = false;
    let mut underscore_in_last_last = false;
    let mut seen = false;
    loop {
        let code = cursor.peek();
        match code {
            Some('.' | '_') => {
                if trail(cursor) {
                    break;
                }
                if code == Some('_') {
                    underscore_in_last = true;
                } else {
                    underscore_in_last_last = underscore_in_last;
                    underscore_in_last = false;
                }
                cursor.consume();
            }
            Some(c) if c != '-' && unicode_punctuation(c) => break,
            next if whitespace_like(next) => break,
            _ => {
                seen = true;
                cursor.consume();
            }
        }
    }
    (!(underscore_in_last_last || underscore_in_last || !seen)).then_some(cursor.at)
}

/// `tokenizePath`: always succeeds; returns its end.
fn path(mut cursor: Cursor<'_>) -> usize {
    let mut open = 0usize;
    let mut close = 0usize;
    loop {
        match cursor.peek() {
            Some('(') => {
                open += 1;
                cursor.consume();
            }
            Some(')') if close < open => {
                close += 1;
                cursor.consume();
            }
            Some(
                '!' | '"' | '&' | '\'' | ')' | '*' | ',' | '.' | ':' | ';' | '<' | '?' | ']' | '_'
                | '~',
            ) => {
                if trail(cursor) {
                    return cursor.at;
                }
                if cursor.peek() == Some(')') {
                    close += 1;
                }
                cursor.consume();
            }
            next if whitespace_like(next) => return cursor.at,
            _ => cursor.consume(),
        }
    }
}

pub(crate) fn email(text: &str, at: usize, previous: Option<char>) -> Option<usize> {
    let mut cursor = Cursor { text, at };
    if !gfm_atext(cursor.peek()) || !previous_email(previous) {
        return None;
    }
    while gfm_atext(cursor.peek()) {
        cursor.consume();
    }
    if cursor.peek() != Some('@') {
        return None;
    }
    cursor.consume();
    let mut dot = false;
    let mut data = false;
    let mut last = '@';
    loop {
        match cursor.peek() {
            Some('.') => {
                let mut after = cursor;
                after.consume();
                if matches!(after.peek(), Some(c) if c.is_ascii_alphanumeric()) {
                    cursor.consume();
                    dot = true;
                    last = '.';
                } else {
                    break;
                }
            }
            Some(c) if c == '-' || c == '_' || c.is_ascii_alphanumeric() => {
                data = true;
                last = c;
                cursor.consume();
            }
            _ => break,
        }
    }
    (data && dot && last.is_ascii_alphabetic()).then_some(cursor.at)
}

fn protocol(text: &str, at: usize, previous: Option<char>) -> Option<usize> {
    let mut cursor = Cursor { text, at };
    if !matches!(cursor.peek(), Some('h' | 'H')) || !previous_protocol(previous) {
        return None;
    }
    let mut buffer = String::new();
    while let Some(c) = cursor.peek() {
        if c.is_ascii_alphabetic() && buffer.len() < 5 {
            buffer.push(c.to_ascii_lowercase());
            cursor.consume();
        } else {
            break;
        }
    }
    if cursor.peek() != Some(':') || !(buffer == "http" || buffer == "https") {
        return None;
    }
    cursor.consume();
    for _ in 0..2 {
        if cursor.peek() != Some('/') {
            return None;
        }
        cursor.consume();
    }
    match cursor.peek() {
        None => return None,
        Some(c) if (c as u32) < 32 || c as u32 == 127 => return None,
        Some(c)
            if line_ending_or_space(Some(c)) || unicode_whitespace(c) || unicode_punctuation(c) =>
        {
            return None
        }
        _ => {}
    }
    let end = domain(cursor)?;
    Some(path(Cursor { text, at: end }))
}

fn www(text: &str, at: usize, previous: Option<char>) -> Option<usize> {
    let cursor = Cursor { text, at };
    if !matches!(cursor.peek(), Some('w' | 'W')) || !previous_www(previous) {
        return None;
    }
    let bytes = text.as_bytes();
    // `wwwPrefixAfter` refuses EOF after `www.`; a block's final line ending is outside
    // micromark's text, so it counts as EOF too.
    if bytes.len() < at + 5
        || !bytes[at..at + 3].eq_ignore_ascii_case(b"www")
        || bytes[at + 3] != b'.'
        || text[at + 4..].trim_end_matches(['\n', '\r']).is_empty()
    {
        return None;
    }
    let end = domain(cursor)?;
    Some(path(Cursor { text, at: end }))
}

/// micromark's construct order for the character at `at`: email first, then protocol or www.
pub(crate) fn scan(text: &str, at: usize, previous: Option<char>) -> Option<(usize, LiteralKind)> {
    if let Some(end) = email(text, at, previous) {
        return Some((end, LiteralKind::Email));
    }
    if let Some(end) = protocol(text, at, previous) {
        return Some((end, LiteralKind::Http));
    }
    www(text, at, previous).map(|end| (end, LiteralKind::Www))
}

/// Cheap prefilter: a literal needs `@`, `://` or `www.` somewhere in the text.
pub(crate) fn may_contain(text: &str) -> bool {
    let bytes = text.as_bytes();
    memchr::memchr(b'@', bytes).is_some()
        || memchr::memmem::find(bytes, b"://").is_some()
        || bytes
            .windows(4)
            .any(|window| window[3] == b'.' && window[..3].eq_ignore_ascii_case(b"www"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_micromark_examples() {
        assert_eq!(
            scan("https://x.com/*b*", 0, None),
            Some((16, LiteralKind::Http))
        );
        assert_eq!(
            scan("www.x.com)", 0, Some('(')),
            Some((9, LiteralKind::Www))
        );
        assert_eq!(scan("a.b@c.de.", 0, None), Some((8, LiteralKind::Email)));
        assert_eq!(scan("a@b.", 0, None), None);
        assert_eq!(scan("www.x.com", 0, Some('a')), None);
    }
}
