//! Port of mdast-util-gfm-autolink-literal 2.0.1 (with mdast-util-find-and-replace 3.0.3)
//! `transformGfmAutolinkLiterals`: the regex find-and-replace pass that links literals the
//! tokenizer left as text (for example after an unbalanced `[`). Like
//! mdast-util-find-and-replace, every node of a changed text node is new and has no
//! position. The arena applies the pieces.

use rill_cmark::autolink_literal::{unicode_punctuation, unicode_whitespace};

/// A replacement node for one changed text node.
pub enum Piece {
    Text(String),
    Link { url: String, text: String },
}

/// One regex match and what the replace function returned for it.
pub struct Match {
    pub start: usize,
    pub end: usize,
    /// The replacement nodes, or `None` where the replace function returned false.
    pub replacement: Option<Vec<Piece>>,
}

/// One global regex with its replace function: the leftmost match at or after a byte.
pub type Finder = fn(&str, usize) -> Option<Match>;

/// mdast-util-find-and-replace 3.0.3's `add`: adjacent text pieces merge.
fn add(nodes: &mut Vec<Piece>, piece: Piece) {
    match (nodes.last_mut(), piece) {
        (Some(Piece::Text(previous)), Piece::Text(text)) => previous.push_str(&text),
        (_, piece) => nodes.push(piece),
    }
}

/// mdast-util-find-and-replace for one global regex over one text value; `None` when
/// nothing changed.
pub fn replace(value: &str, finder: Finder) -> Option<Vec<Piece>> {
    let mut nodes = Vec::new();
    let mut start = 0;
    let mut search = 0;
    let mut changed = false;
    while search <= value.len() {
        let Some(found) = finder(value, search) else {
            break;
        };
        let Some(replacement) = found.replacement else {
            // The replace function returned false: retry one character later.
            search = found.start
                + value[found.start..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            continue;
        };
        if start != found.start {
            add(
                &mut nodes,
                Piece::Text(value[start..found.start].to_owned()),
            );
        }
        for piece in replacement {
            add(&mut nodes, piece);
        }
        start = found.end;
        search = found.end.max(found.start + 1);
        changed = true;
    }
    if !changed {
        return None;
    }
    if start < value.len() {
        add(&mut nodes, Piece::Text(value[start..].to_owned()));
    }
    Some(nodes)
}

/// Which literal a match is; `previous` treats a `/` before an email as a word character.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Literal {
    Url,
    Email,
}

/// `previous(match, email)`: JavaScript reads the UTF-16 code unit before the match, so a
/// supplementary character (a low surrogate there) is neither whitespace nor punctuation.
fn previous(value: &str, at: usize, literal: Literal) -> bool {
    let Some(before) = value[..at].chars().next_back() else {
        return true;
    };
    if before.len_utf16() == 2 {
        return false;
    }
    (unicode_whitespace(before) || unicode_punctuation(before))
        && !(literal == Literal::Email && before == '/')
}

fn ascii_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `/(https?:\/\/|www(?=\.))([-.\w]+)([^ \t\r\n]*)/gi` with `findUrl`.
pub fn find_url(value: &str, from: usize) -> Option<Match> {
    let bytes = value.as_bytes();
    // Every match holds `://` or `www.`; most text has neither.
    let rest = bytes.get(from..)?;
    if memchr::memmem::find(rest, b"://").is_none()
        && !rest
            .windows(4)
            .any(|window| window[3] == b'.' && window[..3].eq_ignore_ascii_case(b"www"))
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
        let starts_with = |prefix: &[u8]| {
            rest.get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        };
        let protocol_len = if starts_with(b"https://") {
            8
        } else if starts_with(b"http://") {
            7
        } else if starts_with(b"www") && rest.get(3) == Some(&b'.') {
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
        let www = protocol_len == 3;
        let replacement = url_replacement(
            value,
            at,
            if www { at } else { domain_start },
            domain_end,
            path_end,
        );
        return Some(Match {
            start: at,
            end: path_end,
            replacement,
        });
    }
    None
}

/// `findUrl` for the match `value[at..path_end]`, whose domain (with `www.` for www
/// links) is `value[host..domain_end]`.
fn url_replacement(
    value: &str,
    at: usize,
    host: usize,
    domain_end: usize,
    path_end: usize,
) -> Option<Vec<Piece>> {
    if !previous(value, at, Literal::Url) || !is_correct_domain(&value[host..domain_end]) {
        return None;
    }
    let split = host + split_url(&value[host..path_end]);
    if split == host {
        return None;
    }
    let text = &value[at..split];
    let url = if host == at {
        format!("http://{text}")
    } else {
        text.to_owned()
    };
    let mut nodes = vec![Piece::Link {
        url,
        text: text.to_owned(),
    }];
    if split < path_end {
        nodes.push(Piece::Text(value[split..path_end].to_owned()));
    }
    Some(nodes)
}

fn is_correct_domain(domain: &str) -> bool {
    let mut parts = domain.rsplit('.');
    let (Some(last), Some(second)) = (parts.next(), parts.next()) else {
        return false;
    };
    let bad = |part: &str| {
        !part.is_empty()
            && (part.contains('_') || !part.bytes().any(|byte| byte.is_ascii_alphanumeric()))
    };
    !(bad(last) || bad(second))
}

/// `splitUrl`: the length of the link, leaving trailing punctuation out except for `)`s
/// that close a `(` in the link.
fn split_url(url: &str) -> usize {
    let mut split = url
        .bytes()
        .rposition(|byte| !b"!\"&'),.:;<>?]}".contains(&byte))
        .map_or(0, |last| last + 1);
    let opening = url[..split].matches('(').count();
    let mut closing = url[..split].matches(')').count();
    while opening > closing {
        let Some(close) = url[split..].find(')') else {
            break;
        };
        split += close + 1;
        closing += 1;
    }
    split
}

/// `/(?<=^|\s|\p{P}|\p{S})([-.\w+]+)@([-\w]+(?:\.[-\w]+)+)/gu` with `findEmail`.
///
/// The leftmost match is found per `@`: its candidate starts are the positions of the
/// atext run before it whose lookbehind holds, tried left to right from `from`.
pub fn find_email(value: &str, from: usize) -> Option<Match> {
    let bytes = value.as_bytes();
    let atext = |byte: u8| ascii_word(byte) || matches!(byte, b'-' | b'.' | b'+');
    let label_char = |byte: u8| ascii_word(byte) || byte == b'-';
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
        while bytes.get(end) == Some(&b'.')
            && bytes.get(end + 1).is_some_and(|&byte| label_char(byte))
        {
            end += 1;
            while end < bytes.len() && label_char(bytes[end]) {
                end += 1;
            }
            dots += 1;
        }
        if dots == 0 {
            continue;
        }
        let bad_ending =
            bytes[end - 1] == b'-' || bytes[end - 1] == b'_' || bytes[end - 1].is_ascii_digit();
        for start in run.max(from)..at_sign {
            let lookbehind = value[..start]
                .chars()
                .next_back()
                .is_none_or(|before| unicode_whitespace(before) || unicode_punctuation(before));
            if !lookbehind {
                continue;
            }
            let replacement = (!bad_ending && previous(value, start, Literal::Email)).then(|| {
                let address = &value[start..end];
                vec![Piece::Link {
                    url: format!("mailto:{address}"),
                    text: address.to_owned(),
                }]
            });
            return Some(Match {
                start,
                end,
                replacement,
            });
        }
    }
    None
}
