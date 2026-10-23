//! Shiki's syntax highlighting in Rust: TextMate grammars run the way
//! `@shikijs/vscode-textmate` runs them on Shiki's JavaScript regex engine, with
//! the patterns oniguruma-to-es translates, so the tokens are identical.

mod fxhash;
pub mod regex;
pub mod shiki;
pub mod textmate;

pub use shiki::{Highlighter, Token, Tokenized};

#[cfg(feature = "wasm-exports")]
pub mod wasm;
