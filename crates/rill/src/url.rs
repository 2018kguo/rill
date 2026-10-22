//! The WHATWG URL fields rehype-harden reads. WebAssembly builds ask the browser's own
//! `URL`, the parser rehype-harden itself uses, so its IDNA and normalization tables stay
//! out of the module; native builds use the `url` crate.

/// A parsed URL, with each field as JavaScript's `URL` reports it.
pub struct Url {
    /// The scheme, lowercase and without the trailing `:`.
    pub scheme: String,
    pub href: String,
    pub pathname: String,
    /// `?query`, or empty when the query is missing or empty.
    pub search: String,
    /// `#fragment`, or empty when the fragment is missing or empty.
    pub hash: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl Url {
    /// `new URL(input)`, or `None` where it throws.
    pub fn parse(input: &str) -> Option<Url> {
        url::Url::parse(input).ok().map(Url::from)
    }

    /// `new URL(input, base)`, or `None` where it throws.
    pub fn parse_with_base(input: &str, base: &str) -> Option<Url> {
        let base = url::Url::parse(base).ok()?;
        base.join(input).ok().map(Url::from)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<url::Url> for Url {
    fn from(parsed: url::Url) -> Url {
        let prefixed = |sigil: char, part: Option<&str>| match part {
            Some(part) if !part.is_empty() => format!("{sigil}{part}"),
            _ => String::new(),
        };
        Url {
            scheme: parsed.scheme().to_owned(),
            pathname: parsed.path().to_owned(),
            search: prefixed('?', parsed.query()),
            hash: prefixed('#', parsed.fragment()),
            href: parsed.into(),
        }
    }
}

#[cfg(all(target_arch = "wasm32", not(feature = "wasm")))]
compile_error!("WebAssembly builds parse URLs with JavaScript's `URL`: enable the `wasm` feature");

#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
mod js {
    use wasm_bindgen::JsValue;
    use wasm_bindgen::prelude::wasm_bindgen;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_name = URL)]
        pub type JsUrl;

        #[wasm_bindgen(constructor, js_class = "URL", catch)]
        pub fn new(input: &str) -> Result<JsUrl, JsValue>;

        #[wasm_bindgen(constructor, js_class = "URL", catch)]
        pub fn new_with_base(input: &str, base: &str) -> Result<JsUrl, JsValue>;

        #[wasm_bindgen(method, getter)]
        pub fn href(this: &JsUrl) -> String;

        #[wasm_bindgen(method, getter)]
        pub fn protocol(this: &JsUrl) -> String;

        #[wasm_bindgen(method, getter)]
        pub fn pathname(this: &JsUrl) -> String;

        #[wasm_bindgen(method, getter)]
        pub fn search(this: &JsUrl) -> String;

        #[wasm_bindgen(method, getter)]
        pub fn hash(this: &JsUrl) -> String;
    }
}

#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
impl Url {
    /// `new URL(input)`, or `None` where it throws.
    pub fn parse(input: &str) -> Option<Url> {
        js::JsUrl::new(input).ok().map(Url::from)
    }

    /// `new URL(input, base)`, or `None` where it throws.
    pub fn parse_with_base(input: &str, base: &str) -> Option<Url> {
        js::JsUrl::new_with_base(input, base).ok().map(Url::from)
    }
}

#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
impl From<js::JsUrl> for Url {
    fn from(parsed: js::JsUrl) -> Url {
        let mut scheme = parsed.protocol();
        scheme.pop(); // The trailing `:`.
        Url {
            scheme,
            href: parsed.href(),
            pathname: parsed.pathname(),
            search: parsed.search(),
            hash: parsed.hash(),
        }
    }
}
