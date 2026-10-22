//! rill: Streamdown's Markdown pipeline in Rust.
//!
//! [`mdast`] parses Markdown into the tree remark builds with Streamdown's remark plugins
//! (remark-gfm, remark-math with `singleDollarTextMath: false`, remark-cjk-friendly), and
//! [`hast`] turns it into the HAST its rehype chain produces (rehype-raw, rehype-sanitize,
//! rehype-harden). Both match the JavaScript output node for node, positions included;
//! `conformance/` checks that against remark itself.

pub mod hast;
pub mod mdast;

mod autolink;
mod json;
mod url;

pub use hast::{
    BlockPolicy, Context, Harden, Kind, Options, Plugin, Rendered, Sanitize, render, render_json,
    render_ops,
};
pub use mdast::{Error, parse_json};

/// The WebAssembly boundary the `@2018kguo/rill` JavaScript package reads: a rendered
/// document as HAST opcodes. A build with its own [`Plugin`]s exports one function that
/// returns [`render`](wasm::render)'s result; `--features wasm-exports` adds the stock
/// exports that the prebuilt package ships.
#[cfg(feature = "wasm")]
pub mod wasm {
    use wasm_bindgen::prelude::*;

    use crate::{Options, hast};

    /// One rendered document: how far Rust took it ([`crate::Kind`]) and the HAST as
    /// opcodes, decoded by the JavaScript package.
    #[wasm_bindgen]
    pub struct RenderedHast {
        kind: &'static str,
        ops: Vec<u32>,
        strings: String,
    }

    #[wasm_bindgen]
    impl RenderedHast {
        #[wasm_bindgen(getter)]
        pub fn kind(&self) -> String {
            self.kind.to_owned()
        }
        /// Moves the opcodes out; a second call returns none.
        pub fn ops(&mut self) -> Vec<u32> {
            std::mem::take(&mut self.ops)
        }
        /// Moves the string table out; a second call returns an empty string.
        pub fn strings(&mut self) -> String {
            std::mem::take(&mut self.strings)
        }
    }

    /// Renders `source` for JavaScript. `source` must be NUL-free and well formed; the
    /// JavaScript package sees to both.
    pub fn render(source: &str, options: &Options) -> Result<RenderedHast, JsError> {
        let encoded =
            hast::render_ops(source, options).map_err(|error| JsError::new(&error.to_string()))?;
        Ok(RenderedHast {
            kind: encoded.kind.as_str(),
            ops: encoded.ops,
            strings: encoded.strings,
        })
    }

    #[cfg(feature = "wasm-exports")]
    mod exports {
        use wasm_bindgen::prelude::*;

        use super::RenderedHast;
        use crate::{Options, hast, mdast};

        /// Streamdown's pipeline with its default options.
        #[wasm_bindgen(js_name = renderHast)]
        pub fn render_hast(source: &str) -> Result<RenderedHast, JsError> {
            super::render(source, &Options::default())
        }

        /// MDAST JSON for `source`.
        #[wasm_bindgen(js_name = parseMdast)]
        pub fn parse_mdast(source: &str) -> Result<String, JsError> {
            mdast::parse_json(source).map_err(|error| JsError::new(&error.to_string()))
        }

        /// `{"kind","tree"}` HAST JSON for `source`.
        #[wasm_bindgen(js_name = renderJson)]
        pub fn render_json(source: &str) -> Result<String, JsError> {
            hast::render_json(source, &Options::default())
                .map_err(|error| JsError::new(&error.to_string()))
        }
    }
}
