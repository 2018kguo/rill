//! rill: Streamdown 2.7.1's Markdown pipeline in Rust.
//!
//! [`mdast`] parses Markdown into the tree remark builds with Streamdown's remark plugins
//! (remark-gfm and remarkCodeMeta, plus `@streamdown/math` and `@streamdown/cjk` when
//! [`Options`] turns them on), and [`hast`] turns it into the HAST its rehype chain
//! produces (rehype-raw, rehype-sanitize, rehype-harden; KaTeX stays in JavaScript). Both
//! match the JavaScript output node for node, positions included; `conformance/` checks
//! that against remark itself.

pub mod hast;
pub mod markup;
pub mod mdast;
pub mod remend;

mod autolink;
mod json;
mod url;

pub use hast::{
    BlockPolicy, Context, Harden, Kind, Options, Plugin, Rendered, Sanitize, render, render_json,
    render_ops,
};
pub use markup::{MarkupOptions, render_markup};
pub use mdast::{Error, Syntax, parse_json};

/// The WebAssembly boundary the `@2018kguo/rill` JavaScript package reads: a rendered
/// document as HAST opcodes. A build with its own [`Plugin`]s exports one function that
/// returns [`render`](wasm::render)'s result; `--features wasm-exports` adds the stock
/// `renderHast` export that the prebuilt package ships.
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
    /// JavaScript package sees to both. An export built on this takes `(source, math,
    /// cjk, ...)`, the first three as `createRill` passes them, and sets
    /// [`Options::math`] and [`Options::cjk`] from them.
    pub fn render(source: &str, options: &Options) -> Result<RenderedHast, JsError> {
        let encoded =
            hast::render_ops(source, options).map_err(|error| JsError::new(&error.to_string()))?;
        Ok(RenderedHast {
            kind: encoded.kind.as_str(),
            ops: encoded.ops,
            strings: encoded.strings,
        })
    }

    /// Renders `source` to the segments of Streamdown's DOM (see [`crate::markup`]);
    /// kind `"markup"`, or `"fallback"` with no segments where only Streamdown renders
    /// it exactly.
    pub fn render_markup(
        source: &str,
        options: &Options,
        markup: &crate::MarkupOptions,
    ) -> Result<RenderedHast, JsError> {
        let encoded = crate::render_markup(source, options, markup)
            .map_err(|error| JsError::new(&error.to_string()))?;
        Ok(match encoded {
            Some(encoded) => RenderedHast {
                kind: "markup",
                ops: encoded.ops,
                strings: encoded.strings,
            },
            None => RenderedHast {
                kind: "fallback",
                ops: Vec::new(),
                strings: String::new(),
            },
        })
    }

    #[cfg(feature = "wasm-exports")]
    mod exports {
        use wasm_bindgen::prelude::*;

        use super::RenderedHast;
        use crate::{MarkupOptions, Options};

        /// remend 1.4.0 with its default options (see [`crate::remend`]).
        #[wasm_bindgen(js_name = remend)]
        pub fn remend(text: &str) -> String {
            crate::remend::remend(text)
        }

        thread_local! {
            static STREAM: std::cell::RefCell<crate::remend::Stream> =
                std::cell::RefCell::default();
        }

        /// remend on a streaming text (see [`crate::remend::Stream`]): `appended` is what
        /// was added since the last call (or the whole text, with `reset`). Returns how
        /// many units of the last healed text the new one keeps; the rest is
        /// [`remend_middle`].
        /// Returns `u32::MAX` instead when the healed text is the whole text as given
        /// (`reset`, nothing to heal: the usual case for a finished answer), with no
        /// middle to read.
        #[wasm_bindgen(js_name = remendStream)]
        pub fn remend_stream(appended: &str, reset: bool) -> u32 {
            STREAM.with_borrow_mut(|stream| {
                let (keep, middle) = stream.heal_with(appended, reset, |middle, input| {
                    (!(reset && middle == input)).then(|| String::from_utf16_lossy(middle))
                });
                let same = middle.is_none();
                MIDDLE.set(middle.unwrap_or_default());
                if same { u32::MAX } else { keep as u32 }
            })
        }

        thread_local! {
            static MIDDLE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
        }

        /// The middle of the last [`remend_stream`] edit.
        #[wasm_bindgen(js_name = remendMiddle)]
        pub fn remend_middle() -> String {
            MIDDLE.take()
        }

        /// Streamdown's DOM for one block (see [`crate::markup`]). `flags`: bit 0
        /// `linkSafety.enabled`, bit 1 server rendering, bit 2 the streaming spine;
        /// `overrides`: the tag names with components of the page's own,
        /// space-separated.
        #[wasm_bindgen(js_name = renderMarkup)]
        pub fn render_markup(
            source: &str,
            math: bool,
            cjk: bool,
            prefix: String,
            flags: u32,
            overrides: &str,
        ) -> Result<RenderedHast, JsError> {
            let options = Options {
                math,
                cjk,
                close_open_raw: true,
                ..Options::default()
            };
            let markup = MarkupOptions {
                prefix,
                link_safety: flags & 1 != 0,
                ssr: flags & 2 != 0,
                overrides: overrides.split_whitespace().map(str::to_owned).collect(),
                math: false,
                spine: flags & 4 != 0,
            };
            super::render_markup(source, &options, &markup)
        }

        /// The root-level link definitions of `source` as UTF-16 `[start, end)` pairs,
        /// or `[u32::MAX]` when a definition sits elsewhere or `source` has a footnote
        /// definition (see [`crate::mdast::root_definitions`]).
        #[wasm_bindgen(js_name = definitions)]
        pub fn definitions(source: &str, math: bool, cjk: bool) -> Vec<u32> {
            let syntax = crate::Syntax { math, cjk };
            match crate::mdast::root_definitions(source, syntax) {
                Some(ranges) => ranges
                    .into_iter()
                    .flat_map(|(start, end)| [start, end])
                    .collect(),
                None => vec![u32::MAX],
            }
        }

        /// Whether no root child of `source` spans UTF-16 offset `at` (see
        /// [`crate::mdast::splits_at`]).
        #[wasm_bindgen(js_name = splitsAt)]
        pub fn splits_at(source: &str, math: bool, cjk: bool, at: u32) -> bool {
            crate::mdast::splits_at(source, crate::Syntax { math, cjk }, at)
        }

        /// Streamdown's pipeline, with `plugins.math` and `plugins.cjk` as given.
        #[wasm_bindgen(js_name = renderHast)]
        pub fn render_hast(source: &str, math: bool, cjk: bool) -> Result<RenderedHast, JsError> {
            let options = Options {
                math,
                cjk,
                ..Options::default()
            };
            super::render(source, &options)
        }
    }
}
