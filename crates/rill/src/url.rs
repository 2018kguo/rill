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

/// A parsed URL, with each field as JavaScript's `URL` reports it.
#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
pub struct Url {
    scheme: String,
    href: String,
    pathname: String,
    search: String,
    hash: String,
}

#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
impl Url {
    /// The scheme, lowercase and without the trailing `:`.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    pub fn pathname(&self) -> &str {
        &self.pathname
    }

    /// `?query`, or empty when the query is missing or empty.
    pub fn search(&self) -> &str {
        &self.search
    }

    /// `#fragment`, or empty when the fragment is missing or empty.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub fn into_href(self) -> String {
        self.href
    }

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
