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

pub use hast::{BlockPolicy, Harden, Kind, render, render_json, render_ops};
pub use mdast::{Error, parse_json};

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    use crate::{Harden, hast, mdast};

    /// MDAST JSON for `source`.
    #[wasm_bindgen(js_name = parseMdast)]
    pub fn parse_mdast(source: &str) -> Result<String, JsError> {
        mdast::parse_json(source).map_err(|error| JsError::new(&error.to_string()))
    }

    /// `{"kind","tree"}` HAST JSON for `source`.
    #[wasm_bindgen(js_name = renderJson)]
    pub fn render_json(source: &str) -> Result<String, JsError> {
        hast::render_json(source, &Harden::default())
            .map_err(|error| JsError::new(&error.to_string()))
    }
}
