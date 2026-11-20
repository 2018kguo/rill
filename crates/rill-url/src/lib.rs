//! The URL rules both renderers apply to links and images: micromark's `normalizeUri`,
//! hast-util-sanitize's protocol check, and rehype-harden's `transformUrl` (with the
//! WHATWG URL fields it reads).

use std::borrow::Cow;
use std::fmt::Write as _;

mod url;
pub use url::Url;

/// micromark-util-sanitize-uri 2.0.1 `normalizeUri`.
#[must_use]
pub fn normalize_uri(value: Cow<'_, str>) -> Cow<'_, str> {
    let keep = |byte: u8| matches!(byte, b'!' | b'#' | b'$' | b'&'..=b';' | b'=' | b'?'..=b'Z' | b'_' | b'a'..=b'z' | b'~');
    let bytes = value.as_bytes();
    let escape_ok = |index: usize| {
        bytes.get(index + 1).is_some_and(u8::is_ascii_alphanumeric)
            && bytes.get(index + 2).is_some_and(u8::is_ascii_alphanumeric)
    };
    let clean = bytes.iter().enumerate().all(|(index, &byte)| {
        (byte < 128 && byte != b'%' && keep(byte)) || (byte == b'%' && escape_ok(index))
    });
    if clean {
        return value;
    }
    let mut out = String::with_capacity(value.len() + 8);
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' && escape_ok(index) {
            out.push_str(&value[index..index + 3]);
            index += 3;
            continue;
        }
        if byte < 128 {
            if keep(byte) {
                out.push(byte as char);
            } else {
                let _ = write!(out, "%{byte:02X}");
            }
            index += 1;
            continue;
        }
        let Some(character) = value[index..].chars().next() else {
            break;
        };
        let mut buffer = [0u8; 4];
        let encoded = character.encode_utf8(&mut buffer);
        for utf8_byte in encoded.as_bytes() {
            let _ = write!(out, "%{utf8_byte:02X}");
        }
        index += character.len_utf8();
    }
    Cow::Owned(out)
}

/// hast-util-sanitize's protocol test: whether `url` has no protocol, or one of
/// `protocols`.
#[must_use]
pub fn protocol_allowed(url: &str, protocols: &[&str]) -> bool {
    let Some(colon) = url.find(':') else {
        return true;
    };
    let before = |c: char| url.find(c).is_some_and(|at| colon > at);
    if before('/') || before('?') || before('#') {
        return true;
    }
    protocols
        .iter()
        .any(|protocol| colon == protocol.len() && url.starts_with(protocol))
}

fn path_relative(url: &str) -> bool {
    url.starts_with('/') || url.starts_with("./") || url.starts_with("../")
}

/// The base rehype-harden resolves relative URLs against.
pub const HARDEN_BASE: &str = "http://example.com";

/// The two kinds of element rehype-harden checks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Link,
    Image,
}

/// Why `transformUrl` returned null.
pub struct Blocked;

/// `transformUrl`; `Ok(None)` when the URL comes back unchanged.
pub fn transform_url(url: &str, resource: Resource) -> Result<Option<String>, Blocked> {
    let image = resource == Resource::Image;
    if url.is_empty() {
        return Err(Blocked);
    }
    if url.starts_with('#')
        && !image
        && Url::parse_with_base(url, HARDEN_BASE).is_some_and(|test| test.hash() == url)
    {
        return Ok(None);
    }
    if url.starts_with("data:") {
        return if image && url.starts_with("data:image/") {
            Ok(None)
        } else {
            Err(Blocked)
        };
    }
    if let Some(after) = url.strip_prefix("blob:") {
        let parsed = Url::parse(url).ok_or(Blocked)?;
        return if parsed.scheme() == "blob" && !after.is_empty() && after != "invalid" {
            Ok(None)
        } else {
            Err(Blocked)
        };
    }
    // A path-relative URL has no scheme, so only the base can parse it; skipping the
    // absolute attempt matters in WebAssembly, where a failed `new URL` throws.
    let parsed = if path_relative(url) {
        Url::parse_with_base(url, HARDEN_BASE).ok_or(Blocked)?
    } else {
        Url::parse(url).ok_or(Blocked)?
    };
    if matches!(parsed.scheme(), "javascript" | "data" | "file" | "vbscript") {
        return Err(Blocked);
    }
    let result = if matches!(parsed.scheme(), "http" | "https") && path_relative(url) {
        format!("{}{}{}", parsed.pathname(), parsed.search(), parsed.hash())
    } else {
        parsed.into_href()
    };
    Ok((result != url).then_some(result))
}

