//! The WHATWG URL fields rehype-harden reads. WebAssembly builds ask the browser's own
//! `URL`, the parser rehype-harden itself uses, so its IDNA and normalization tables stay
//! out of the module; native builds use the `url` crate.

/// A parsed URL whose fields read as JavaScript's `URL` reports them. Natively the
/// fields are slices of the one serialized `href`.
#[cfg(not(target_arch = "wasm32"))]
pub struct Url(url::Url);

#[cfg(not(target_arch = "wasm32"))]
impl Url {
    /// `new URL(input)`, or `None` where it throws.
    pub fn parse(input: &str) -> Option<Url> {
        url::Url::parse(input).ok().map(Url)
    }

    /// `new URL(input, base)`, or `None` where it throws.
    pub fn parse_with_base(input: &str, base: &str) -> Option<Url> {
        let base = url::Url::parse(base).ok()?;
        base.join(input).ok().map(Url)
    }

    /// The scheme, lowercase and without the trailing `:`.
    pub fn scheme(&self) -> &str {
        self.0.scheme()
    }

    pub fn pathname(&self) -> &str {
        self.0.path()
    }

    /// `?query`, or empty when the query is missing or empty.
    pub fn search(&self) -> &str {
        use url::Position;
        match self.0.query() {
            Some(query) if !query.is_empty() => &self.0[Position::AfterPath..Position::AfterQuery],
            _ => "",
        }
    }

    /// `#fragment`, or empty when the fragment is missing or empty.
    pub fn hash(&self) -> &str {
        match self.0.fragment() {
            Some(fragment) if !fragment.is_empty() => &self.0[url::Position::AfterQuery..],
            _ => "",
        }
    }

    pub fn into_href(self) -> String {
        self.0.into()
    }
}

#[cfg(all(target_arch = "wasm32", not(feature = "wasm")))]
compile_error!("WebAssembly builds parse URLs with JavaScript's `URL`: enable the `wasm` feature");

#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
mod js {
    use wasm_bindgen::prelude::wasm_bindgen;

    // One call per URL: the fields come back in one string, newline-separated, which no
    // field can contain (the URL parser strips tabs and newlines from its input).
    #[wasm_bindgen(inline_js = "
// URL.parse returns null where the constructor throws, and throwing is slow.
const parse = URL.parse ?? ((input, base) => {
  try {
    return new URL(input, base);
  } catch {
    return null;
  }
});
export function urlFields(input, base) {
  const url = parse(input, base);
  return url === null
    ? undefined
    : [url.protocol, url.pathname, url.search, url.hash, url.href].join('\\n');
}")]
    extern "C" {
        #[wasm_bindgen(js_name = urlFields)]
        pub fn url_fields(input: &str, base: Option<&str>) -> Option<String>;
    }
}

/// A parsed URL: JavaScript's `protocol`, `pathname`, `search`, `hash` and `href`, one
/// per line.
#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
pub struct Url {
    fields: String,
}

#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
impl Url {
    fn field(&self, index: usize) -> &str {
        self.fields
            .split('\n')
            .nth(index)
            .expect("urlFields returns five fields")
    }

    /// The scheme, lowercase and without the trailing `:`.
    pub fn scheme(&self) -> &str {
        let protocol = self.field(0);
        &protocol[..protocol.len() - 1]
    }

    pub fn pathname(&self) -> &str {
        self.field(1)
    }

    /// `?query`, or empty when the query is missing or empty.
    pub fn search(&self) -> &str {
        self.field(2)
    }

    /// `#fragment`, or empty when the fragment is missing or empty.
    pub fn hash(&self) -> &str {
        self.field(3)
    }

    pub fn into_href(self) -> String {
        self.field(4).to_owned()
    }

    /// `new URL(input)`, or `None` where it throws.
    pub fn parse(input: &str) -> Option<Url> {
        js::url_fields(input, None).map(|fields| Url { fields })
    }

    /// `new URL(input, base)`, or `None` where it throws.
    pub fn parse_with_base(input: &str, base: &str) -> Option<Url> {
        js::url_fields(input, Some(base)).map(|fields| Url { fields })
    }
}
