//! rill: Streamdown 2.7.1's Markdown pipeline in Rust.
//!
//! [`mdast`] parses Markdown into the tree remark builds with Streamdown's remark plugins
//! (remark-gfm and remarkCodeMeta, plus `@streamdown/math` and `@streamdown/cjk` when
//! [`Options`] turns them on), and [`hast`] turns it into the HAST its rehype chain
//! produces (rehype-raw, rehype-sanitize, rehype-harden; KaTeX stays in JavaScript). Both
//! match the JavaScript output node for node, positions included; `conformance/` checks
//! that against remark itself.

pub mod animate;
pub mod extras;
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
pub use markup::{Fallback, MarkupOptions, markup, render_markup};
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
    pub const ABI: u32 = 5;

    /// One rendered document: how far Rust took it (a [`crate::Kind`], `"markup"`, a
    /// [`crate::Fallback`]'s name where only Streamdown renders a block exactly, or
    /// `"unsupported"` where a plugin left it to JavaScript) and its opcodes, decoded by
    /// the JavaScript package.
    #[wasm_bindgen]
    pub struct RenderedHast {
        kind: &'static str,
        ops: Vec<u32>,
        strings: String,
        animated: Option<crate::animate::Animated>,
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
        /// After an animated render: the text's length in UTF-16 units and the timeline's
        /// cursor (`rill::animate::Animated`); empty otherwise.
        pub fn animated(&self) -> Vec<f64> {
            self.animated
                .map(|animated| vec![f64::from(animated.chars), animated.cursor])
                .unwrap_or_default()
        }
    }

    impl RenderedHast {
        fn empty(kind: &'static str) -> Self {
            RenderedHast {
                kind,
                ops: Vec::new(),
                strings: String::new(),
                animated: None,
            }
        }
    }

    /// `options` with the plugins the JavaScript package found in `<Streamdown>`'s
    /// props, on top of the build's own. `syntax`: bit 0 `plugins.math` (or
    /// remark-math), 1 `plugins.cjk`, 2 remark-math's `singleDollarTextMath`, 3
    /// remark-breaks, 4 no remarkCodeMeta, 5 rehype-slug, 6 rehype-slug after
    /// rehype-katex, 7 rehype-external-links, 8 rehype-sanitize's own default schema.
    /// `extra`: rehype-slug's prefix, then rehype-external-links' target, rel and
    /// protocols (space-separated), joined by NUL.
    #[doc(hidden)]
    #[must_use]
    pub fn with_plugins(mut options: Options, syntax: u32, extra: &str) -> Options {
        let bit = |n: u32| syntax & (1 << n) != 0;
        options.math |= bit(0);
        options.cjk |= bit(1);
        options.single_dollar |= bit(2);
        options.breaks |= bit(3);
        if bit(4) {
            options.code_meta = false;
        }
        if bit(8) {
            options.sanitize.schema = crate::Schema::Default;
        }
        if bit(5) || bit(7) {
            let mut fields = extra.split('\0');
            let prefix = fields.next().unwrap_or_default();
            if bit(5) {
                options.slug = Some(crate::extras::Slug {
                    prefix: prefix.to_owned(),
                    after_katex: bit(6),
                });
            }
            if bit(7) {
                let mut next = || fields.next().unwrap_or_default();
                let target = next();
                let tokens = |list: &str| list.split_whitespace().map(str::to_owned).collect();
                options.external_links = Some(crate::extras::ExternalLinks {
                    target: (!target.is_empty()).then(|| target.to_owned()),
                    rel: tokens(next()),
                    protocols: tokens(next()),
                });
            }
        }
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
                animated: None,
            }),
            Err(Error::Unsupported) => Ok(RenderedHast::empty("unsupported")),
            Err(error) => Err(JsError::new(&error.to_string())),
        }
    }

    /// Streamdown's DOM for one block (see [`crate::markup`]). `flags`: bit 0
    /// `linkSafety.enabled`, bit 1 server rendering, bit 2 the streaming spine, bit 3
    /// KaTeX available, bit 4 an incomplete code fence, bit 5 every element through
    /// React; `overrides`: the tag names with components of the page's own,
    /// space-separated. `animate`, while `animated` streams: the animation, duration and
    /// easing joined by NUL; `timing`: `sep: "char"` (0 or 1), the stagger, the units
    /// already shown, the timeline's cursor, the clock and the backlog (see
    /// [`crate::animate::Animate`]).
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn render_markup(
        source: &str,
        mut options: Options,
        prefix: String,
        flags: u32,
        overrides: &str,
        animate: &str,
        timing: &[f64],
    ) -> Result<RenderedHast, JsError> {
        options.close_open_raw = true;
        let markup = MarkupOptions {
            prefix,
            link_safety: flags & 1 != 0,
            ssr: flags & 2 != 0,
            overrides: overrides.split_whitespace().map(str::to_owned).collect(),
            math: false,
            katex: flags & 8 != 0,
            incomplete: flags & 16 != 0,
            spine: flags & 4 != 0,
            deep: flags & 32 != 0,
            animate: animation(animate, timing),
        };
        Ok(match crate::markup(source, &options, &markup) {
            Ok(encoded) => RenderedHast {
                kind: "markup",
                ops: encoded.ops,
                strings: encoded.strings,
                animated: encoded.animated,
            },
            Err(Ok(fallback)) => RenderedHast::empty(fallback.as_str()),
            Err(Err(error)) => return Err(JsError::new(&error.to_string())),
        })
    }

    /// [`render_markup`]'s `animate` and `timing` as the pass's options.
    fn animation(animate: &str, timing: &[f64]) -> Option<crate::animate::Animate> {
        let mut names = animate.split('\0');
        let (Some(animation), Some(duration), Some(easing)) =
            (names.next(), names.next(), names.next())
        else {
            return None;
        };
        let &[by_char, stagger, shown, cursor, now, backlog] = timing else {
            return None;
        };
        Some(crate::animate::Animate {
            animation: animation.to_owned(),
            duration: duration.to_owned(),
            easing: easing.to_owned(),
            by_char: by_char != 0.0,
            stagger,
            shown: shown as u32,
            cursor,
            now,
            backlog,
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

        /// The HAST for `source`, with the plugins `syntax` and `extra` add (see
        /// `rill::wasm::with_plugins`).
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = renderHast,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_render_hast(
            source: &str,
            syntax: u32,
            extra: &str,
            $($arg: $ty),*
        ) -> Result<$crate::wasm::RenderedHast, $crate::wasm::wasm_bindgen::JsError> {
            let options: $crate::Options = $options;
            $crate::wasm::render_hast(source, &$crate::wasm::with_plugins(options, syntax, extra))
        }

        /// Streamdown's DOM for one block (see `rill::wasm::render_markup`).
        #[allow(clippy::too_many_arguments)]
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = renderMarkup,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_render_markup(
            source: &str,
            syntax: u32,
            extra: &str,
            prefix: String,
            flags: u32,
            overrides: &str,
            animate: &str,
            timing: &[f64],
            $($arg: $ty),*
        ) -> Result<$crate::wasm::RenderedHast, $crate::wasm::wasm_bindgen::JsError> {
            let options: $crate::Options = $options;
            $crate::wasm::render_markup(
                source,
                $crate::wasm::with_plugins(options, syntax, extra),
                prefix,
                flags,
                overrides,
                animate,
                timing,
            )
        }

        /// Whether rill may render a long block as pieces: when every plugin is
        /// `piecewise`.
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = pieces,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_pieces($($arg: $ty),*) -> bool {
            let options: $crate::Options = $options;
            options.plugins.iter().all(|plugin| plugin.piecewise())
        }

        /// The root-level link definitions of `source` (see `rill::wasm::definitions`).
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = definitions,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_definitions(source: &str, syntax: u32, $($arg: $ty),*) -> Vec<u32> {
            let options: $crate::Options = $options;
            $crate::wasm::definitions(source, &$crate::wasm::with_plugins(options, syntax, ""))
        }

        /// Whether no root child of `source` spans UTF-16 offset `at`.
        #[$crate::wasm::wasm_bindgen::prelude::wasm_bindgen(
            js_name = splitsAt,
            wasm_bindgen = $crate::wasm::wasm_bindgen
        )]
        pub fn __rill_splits_at(
            source: &str,
            syntax: u32,
            at: u32,
            $($arg: $ty),*
        ) -> bool {
            let options: $crate::Options = $options;
            let options = $crate::wasm::with_plugins(options, syntax, "");
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
