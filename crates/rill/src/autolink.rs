//! Port of mdast-util-gfm-autolink-literal 2.0.1 (with mdast-util-find-and-replace 3.0.3) `transformGfmAutolinkLiterals`: the
//! regex find-and-replace pass that links literals the tokenizer left as text (for
//! example after an unbalanced `[`). Like mdast-util-find-and-replace, every node of a
//! changed text node is new and has no position. The arena applies the pieces.

use rill_cmark::autolink_literal::{unicode_punctuation, unicode_whitespace};

/// A replacement node for one changed text node.
pub enum Piece {
    Text(String),
    Link { url: String, text: String },
}

pub type Finder = fn(&str, usize) -> Option<(usize, usize, Vec<Piece>)>;

fn text(value: &str) -> Piece {
    Piece::Text(value.to_owned())
}

fn link(url: String, value: &str) -> Piece {
    Piece::Link {
        url,
        text: value.to_owned(),
    }
}

/// mdast-util-find-and-replace 3.0.3's `add`: adjacent text pieces merge.
fn add(nodes: &mut Vec<Piece>, piece: Piece) {
    match (nodes.last_mut(), piece) {
        (Some(Piece::Text(previous)), Piece::Text(text)) => previous.push_str(&text),
        (_, piece) => nodes.push(piece),
    }
}

/// mdast-util-find-and-replace for one global regex over one text value.
pub fn replace(value: &str, finder: Finder) -> Option<Vec<Piece>> {
    let mut nodes = Vec::new();
    let mut start = 0;
    let mut search = 0;
    let mut changed = false;
    while search <= value.len() {
        let Some((at, end, replacement)) = finder(value, search) else {
            break;
        };
        if replacement.is_empty() {
            // The replace function returned false: retry one character later.
            search = at + value[at..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if start != at {
            add(&mut nodes, text(&value[start..at]));
        }
        for piece in replacement {
            add(&mut nodes, piece);
        }
        start = end;
        search = end.max(at + 1);
        changed = true;
    }
    if !changed {
        return None;
    }
    if start < value.len() {
        add(&mut nodes, text(&value[start..]));
    }
    Some(nodes)
}

/// `previous(match, email)`: JavaScript reads the UTF-16 code unit before the match, so a
/// supplementary character (a low surrogate there) is neither whitespace nor punctuation.
fn previous(value: &str, at: usize, email: bool) -> bool {
    let Some(before) = value[..at].chars().next_back() else {
        return true;
    };
    if before.len_utf16() == 2 {
        return false;
    }
    (unicode_whitespace(before) || unicode_punctuation(before)) && !(email && before == '/')
}

fn ascii_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `/(https?:\/\/|www(?=\.))([-.\w]+)([^ \t\r\n]*)/gi` with `findUrl`; returns the match
/// span and its replacement (empty when `findUrl` returned false).
pub fn find_url(value: &str, from: usize) -> Option<(usize, usize, Vec<Piece>)> {
    let bytes = value.as_bytes();
    // Every match holds `://` or `www.`; most text has neither.
    let rest = bytes.get(from..)?;
    if memchr::memmem::find(rest, b"://").is_none()
        && !rest
            .windows(4)
            .any(|w| w[3] == b'.' && w[..3].eq_ignore_ascii_case(b"www"))
    {
        return None;
    }
    let mut at = from;
    while at < bytes.len() {
        if !value.is_char_boundary(at) {
            at += 1;
            continue;
        }
        let rest = &bytes[at..];
        let protocol_len = if rest.len() >= 8 && rest[..8].eq_ignore_ascii_case(b"https://") {
            8
        } else if rest.len() >= 7 && rest[..7].eq_ignore_ascii_case(b"http://") {
            7
        } else if rest.len() >= 4 && rest[..3].eq_ignore_ascii_case(b"www") && rest[3] == b'.' {
            3
        } else {
            at += 1;
            continue;
        };
        let domain_start = at + protocol_len;
        let mut domain_end = domain_start;
        while domain_end < bytes.len()
            && (ascii_word(bytes[domain_end]) || matches!(bytes[domain_end], b'-' | b'.'))
        {
            domain_end += 1;
        }
        if domain_end == domain_start {
            at += 1;
            continue;
        }
        let mut path_end = domain_end;
        while path_end < bytes.len() && !matches!(bytes[path_end], b' ' | b'\t' | b'\r' | b'\n') {
            path_end += 1;
        }
        let protocol = &value[at..domain_start];
        let domain = &value[domain_start..domain_end];
        let path = &value[domain_end..path_end];
        return Some((
            at,
            path_end,
            url_replacement(value, at, protocol, domain, path),
        ));
    }
    None
}

fn url_replacement(value: &str, at: usize, protocol: &str, domain: &str, path: &str) -> Vec<Piece> {
    if !previous(value, at, false) {
        return Vec::new();
    }
    let (protocol, domain, prefix) = if protocol.as_bytes()[0].eq_ignore_ascii_case(&b'w') {
        ("", format!("{protocol}{domain}"), "http://")
    } else {
        (protocol, domain.to_owned(), "")
    };
    if !is_correct_domain(&domain) {
        return Vec::new();
    }
    let (url, trail) = split_url(&format!("{domain}{path}"));
    if url.is_empty() {
        return Vec::new();
    }
    let mut nodes = vec![link(
        format!("{prefix}{protocol}{url}"),
        &format!("{protocol}{url}"),
    )];
    if let Some(trail) = trail.filter(|trail| !trail.is_empty()) {
        nodes.push(text(&trail));
    }
    nodes
}

fn is_correct_domain(domain: &str) -> bool {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    let bad = |part: &str| {
        !part.is_empty() && (part.contains('_') || !part.bytes().any(|b| b.is_ascii_alphanumeric()))
    };
    !(bad(parts[parts.len() - 1]) || bad(parts[parts.len() - 2]))
}

fn split_url(url: &str) -> (String, Option<String>) {
    let trail_start = url
        .bytes()
        .rposition(|b| !b"!\"&'),.:;<>?]}".contains(&b))
        .map_or(0, |last| last + 1);
    if trail_start == url.len() {
        return (url.to_owned(), None);
    }
    let mut head = url[..trail_start].to_owned();
    let mut trail = url[trail_start..].to_owned();
    let opening = head.matches('(').count();
    let mut closing = head.matches(')').count();
    while let Some(close) = trail.find(')') {
        if opening <= closing {
            break;
        }
        head.push_str(&trail[..=close]);
        trail = trail[close + 1..].to_owned();
        closing += 1;
    }
    (head, Some(trail))
}

/// `/(?<=^|\s|\p{P}|\p{S})([-.\w+]+)@([-\w]+(?:\.[-\w]+)+)/gu` with `findEmail`.
///
/// The leftmost match is found per `@`: its candidate starts are the positions of the
/// atext run before it whose lookbehind holds, tried left to right from `from`.
pub fn find_email(value: &str, from: usize) -> Option<(usize, usize, Vec<Piece>)> {
    let bytes = value.as_bytes();
    let atext = |b: u8| ascii_word(b) || matches!(b, b'-' | b'.' | b'+');
    let label_char = |b: u8| ascii_word(b) || b == b'-';
    let mut search = from;
    while let Some(found) = memchr::memchr(b'@', bytes.get(search..)?) {
        let at_sign = search + found;
        search = at_sign + 1;
        let mut run = at_sign;
        while run > 0 && atext(bytes[run - 1]) {
            run -= 1;
        }
        // The domain: `[-\w]+` then one or more `\.[-\w]+`, greedy.
        let label_start = at_sign + 1;
        let mut end = label_start;
        while end < bytes.len() && label_char(bytes[end]) {
            end += 1;
        }
        if end == label_start {
            continue;
        }
        let mut dots = 0;
        while bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(|b| label_char(*b)) {
            end += 1;
            while end < bytes.len() && label_char(bytes[end]) {
                end += 1;
            }
            dots += 1;
        }
        if dots == 0 {
            continue;
        }
        for start in run.max(from)..at_sign {
            let lookbehind = value[..start]
                .chars()
                .next_back()
                .is_none_or(|c| unicode_whitespace(c) || unicode_punctuation(c));
            if !lookbehind {
                continue;
            }
            let label = &value[label_start..end];
            let rejected = !previous(value, start, true)
                || label
                    .bytes()
                    .last()
                    .is_some_and(|b| b == b'-' || b == b'_' || b.is_ascii_digit());
            let replacement = if rejected {
                Vec::new()
            } else {
                let address = &value[start..end];
                vec![link(format!("mailto:{address}"), address)]
            };
            return Some((start, end, replacement));
        }
    }
    None
}
