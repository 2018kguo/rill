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
    BlockPolicy, Context, Harden, Kind, Options, Plugin, Rendered, Sanitize, Schema, render,
    render_json, render_ops,
};
pub use markup::{MarkupOptions, render_markup};
pub use mdast::{Error, Syntax, parse_json};

/// The WebAssembly boundary the `@2018kguo/rill` JavaScript package reads. A build is one
/// [`wasm_exports!`] call: it exports every entry point the package uses (HAST for
/// plugins, Streamdown's DOM for the drop-in, remend, block splitting), all with the
/// build's [`Options`] and its own extra arguments. `--features wasm-exports` builds the
/// prebuilt package's, with Streamdown's options.
#[cfg(feature = "wasm")]
pub mod wasm {
    pub use wasm_bindgen;
    use wasm_bindgen::prelude::*;

    use crate::{Error, MarkupOptions, Options, hast};

    /// The boundary's version, which the JavaScript package checks before it calls
    /// anything else.
    pub const ABI: u32 = 3;

    /// One rendered document: how far Rust took it (a [`crate::Kind`], `"markup"`,
    /// `"fallback"` where only Streamdown renders a block exactly, or `"unsupported"`
    /// where a plugin left it to JavaScript) and its opcodes, decoded by the JavaScript
    /// package.
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

    impl RenderedHast {
        fn empty(kind: &'static str) -> Self {
            RenderedHast {
                kind,
                ops: Vec::new(),
                strings: String::new(),
            }
        }
    }

    /// `options` with Streamdown's `plugins.math` and `plugins.cjk` as the JavaScript
    /// package passes them, on top of the build's own.
    #[doc(hidden)]
    #[must_use]
    pub fn with_plugins(mut options: Options, math: bool, cjk: bool) -> Options {
        options.math |= math;
        options.cjk |= cjk;
        options
    }

    /// The HAST for `source` (NUL-free and well formed: the JavaScript package sees to
    /// both).
    #[doc(hidden)]
    pub fn render_hast(source: &str, options: &Options) -> Result<RenderedHast, JsError> {
        match hast::render_ops(source, options) {
            Ok(encoded) => Ok(RenderedHast {
                kind: encoded.kind.as_str(),
                ops: encoded.ops,
                strings: encoded.strings,
            }),
            Err(Error::Unsupported) => Ok(RenderedHast::empty("unsupported")),
            Err(error) => Err(JsError::new(&error.to_string())),
        }
    }

    /// Streamdown's DOM for one block (see [`crate::markup`]). `flags`: bit 0
    /// `linkSafety.enabled`, bit 1 server rendering, bit 2 the streaming spine, bit 3
    /// KaTeX available; `overrides`: the tag names with components of the page's own,
    /// space-separated.
    #[doc(hidden)]
    pub fn render_markup(
        source: &str,
        mut options: Options,
        prefix: String,
        flags: u32,
        overrides: &str,
    ) -> Result<RenderedHast, JsError> {
        options.close_open_raw = true;
        let markup = MarkupOptions {
            prefix,
            link_safety: flags & 1 != 0,
            ssr: flags & 2 != 0,
            overrides: overrides.split_whitespace().map(str::to_owned).collect(),
            math: false,
            katex: flags & 8 != 0,
            spine: flags & 4 != 0,
        };
        let encoded = crate::render_markup(source, &options, &markup)
            .map_err(|error| JsError::new(&error.to_string()))?;
        Ok(match encoded {
            Some(encoded) => RenderedHast {
                kind: "markup",
                ops: encoded.ops,
                strings: encoded.strings,
            },
            None => RenderedHast::empty("fallback"),
        })
    }

    /// The root-level link definitions of `source` as UTF-16 `[start, end)` pairs, or
    /// `[u32::MAX]` when a definition sits elsewhere or `source` has a footnote
    /// definition (see [`crate::mdast::root_definitions`]).
    #[doc(hidden)]
    #[must_use]
    pub fn definitions(source: &str, options: &Options) -> Vec<u32> {
        match crate::mdast::root_definitions(source, options.syntax()) {
            Some(ranges) => ranges
                .into_iter()
                .flat_map(|(start, end)| [start, end])
                .collect(),
            None => vec![u32::MAX],
        }
    }

    thread_local! {
        static STREAM: std::cell::RefCell<crate::remend::Stream> =
            std::cell::RefCell::default();
        static MIDDLE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }

    /// remend on a streaming text (see [`crate::remend::Stream`]): `appended` is what was
    /// added since the last call (or the whole text, with `reset`). Returns how many units
    /// of the last healed text the new one keeps; the rest is [`remend_middle`]. Returns
    /// `u32::MAX` instead when the healed text is the whole text as given (`reset`,
    /// nothing to heal: the usual case for a finished answer), with no middle to read.
    #[doc(hidden)]
    #[must_use]
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

    /// The middle of the last [`remend_stream`] edit.
    #[doc(hidden)]
    #[must_use]
    pub fn remend_middle() -> String {
        MIDDLE.take()
    }
}

/// Exports a rill build's WebAssembly entry points, with the [`Options`] the given
/// closure-like expression returns for the build's extra arguments, which JavaScript
/// passes as `engineArgs` (any types `wasm-bindgen` takes):
///
/// ```ignore
/// rill::wasm_exports!(|chat_id: Option<String>, citations: bool| {
///     chat::options(chat_id.as_deref(), citations)
/// });
/// ```
///
/// Every entry point runs with the same options, so the HAST plugins see, Streamdown's
/// DOM and block splitting cannot disagree. Call it once, at the root of a `cdylib`
/// crate that depends on `rill` with the `wasm` feature, and build it with
/// `wasm-bindgen --target web`.
#[cfg(feature = "wasm")]
#[macro_export]
macro_rules! wasm_exports {
    (|| $options:expr) => {
        $crate::wasm_exports!(@build [] $options);
    };
    (|$($arg:ident: $ty:ty),+ $(,)?| $options:expr) => {
        $crate::wasm_exports!(@build [$($arg: $ty),+] $options);
    };
    (@build [$($arg:ident: $ty:ty),*] $options:expr) => {
        /// The boundary's version (`rill::wasm::ABI`).
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = rillAbi,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_abi() -> u32 {
            $crate::wasm::ABI
        }

        /// The HAST for `source`, with Streamdown's `plugins.math` and `plugins.cjk` as
        /// given.
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = renderHast,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_render_hast(
            source: &str,
            math: bool,
            cjk: bool,
            $($arg: $ty),*
        ) -> Result<$crate::wasm::RenderedHast, $crate::wasm::wasm_bindgen::JsError> {
            let options: $crate::Options = $options;
            $crate::wasm::render_hast(source, &$crate::wasm::with_plugins(options, math, cjk))
        }

        /// Streamdown's DOM for one block (see `rill::wasm::render_markup`).
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = renderMarkup,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_render_markup(
            source: &str,
            math: bool,
            cjk: bool,
            prefix: String,
            flags: u32,
            overrides: &str,
            $($arg: $ty),*
        ) -> Result<$crate::wasm::RenderedHast, $crate::wasm::wasm_bindgen::JsError> {
            let options: $crate::Options = $options;
            $crate::wasm::render_markup(
                source,
                $crate::wasm::with_plugins(options, math, cjk),
                prefix,
                flags,
                overrides,
            )
        }

        /// Whether rill may render a long block as pieces: only without plugins, whose
        /// output may depend on the whole text.
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = pieces,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_pieces($($arg: $ty),*) -> bool {
            let options: $crate::Options = $options;
            options.plugins.is_empty()
        }

        /// The root-level link definitions of `source` (see `rill::wasm::definitions`).
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = definitions,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_definitions(source: &str, math: bool, cjk: bool, $($arg: $ty),*) -> Vec<u32> {
            let options: $crate::Options = $options;
            $crate::wasm::definitions(source, &$crate::wasm::with_plugins(options, math, cjk))
        }

        /// Whether no root child of `source` spans UTF-16 offset `at`.
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = splitsAt,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_splits_at(
            source: &str,
            math: bool,
            cjk: bool,
            at: u32,
            $($arg: $ty),*
        ) -> bool {
            let options: $crate::Options = $options;
            let options = $crate::wasm::with_plugins(options, math, cjk);
            $crate::mdast::splits_at(source, options.syntax(), at)
        }

        /// remend 1.4.0 with its default options.
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = remend,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_remend(text: &str) -> String {
            $crate::remend::remend(text)
        }

        /// remend on a streaming text (see `rill::wasm::remend_stream`).
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = remendStream,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_remend_stream(appended: &str, reset: bool) -> u32 {
            $crate::wasm::remend_stream(appended, reset)
        }

        /// The middle of the last `remendStream` edit.
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = remendMiddle,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_remend_middle() -> String {
            $crate::wasm::remend_middle()
        }
    };
}

// The prebuilt package's build: Streamdown's pipeline, with no plugins of its own.
#[cfg(feature = "wasm-exports")]
wasm_exports!(|| Options::default());
