//! C-ABI exports for the JavaScript package (`highlight/js/rill-highlight.js`).
//!
//! Handles are small integers. Strings go in as UTF-8 (JSON) or UTF-16 (code) in
//! buffers from `rhl_alloc`; results come back in a buffer the caller reads
//! before the next call.

use crate::shiki::Highlighter;
use crate::textmate::tokenizer::State;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
struct Exports {
    /// Grammar files collected for the next `rhl_highlighter_new`.
    pending_grammars: Vec<String>,
    themes: HashMap<u32, String>,
    highlighters: HashMap<u32, Highlighter>,
    states: HashMap<u32, State>,
    next_id: u32,
    result: Vec<u32>,
    text: Vec<u8>,
}

thread_local! {
    static EXPORTS: RefCell<Exports> = RefCell::new(Exports::default());
}

impl Exports {
    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn set_text(&mut self, text: &str) -> *const u8 {
        self.text.clear();
        self.text.extend_from_slice(text.as_bytes());
        self.text.as_ptr()
    }
}

/// Allocates `len` bytes for the caller to fill.
#[unsafe(no_mangle)]
pub extern "C" fn rhl_alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len.max(1));
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Frees a buffer from `rhl_alloc`.
///
/// # Safety
/// `ptr` and `len` must come from one `rhl_alloc` call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rhl_free(ptr: *mut u8, len: usize) {
    drop(unsafe { Vec::from_raw_parts(ptr, 0, len.max(1)) });
}

unsafe fn read_utf8(ptr: *const u8, len: usize) -> String {
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(ptr, len) }).into_owned()
}

/// Queues one grammar file (UTF-8 JSON) for the next `rhl_highlighter_new`.
///
/// # Safety
/// `ptr..ptr+len` must be readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rhl_add_grammar(ptr: *const u8, len: usize) {
    let text = unsafe { read_utf8(ptr, len) };
    EXPORTS.with_borrow_mut(|e| e.pending_grammars.push(text));
}

/// Registers a theme (UTF-8 JSON); returns its handle.
///
/// # Safety
/// `ptr..ptr+len` must be readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rhl_add_theme(ptr: *const u8, len: usize) -> u32 {
    let text = unsafe { read_utf8(ptr, len) };
    EXPORTS.with_borrow_mut(|e| {
        let id = e.id();
        e.themes.insert(id, text);
        id
    })
}

/// Creates a highlighter for the grammar named `name` from the queued grammar
/// files and two themes. Returns 0 on error (see `rhl_error`).
///
/// # Safety
/// `ptr..ptr+len` must be readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rhl_highlighter_new(ptr: *const u8, len: usize, light: u32, dark: u32) -> u32 {
    let name = unsafe { read_utf8(ptr, len) };
    EXPORTS.with_borrow_mut(|e| {
        let grammars = std::mem::take(&mut e.pending_grammars);
        let refs: Vec<&str> = grammars.iter().map(String::as_str).collect();
        let (Some(light), Some(dark)) = (e.themes.get(&light), e.themes.get(&dark)) else {
            e.set_text("unknown theme");
            return 0;
        };
        match Highlighter::new(&refs, &name, [light, dark]) {
            Ok(highlighter) => {
                let id = e.id();
                e.highlighters.insert(id, highlighter);
                id
            }
            Err(error) => {
                e.set_text(&error);
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn rhl_highlighter_free(highlighter: u32) {
    EXPORTS.with_borrow_mut(|e| e.highlighters.remove(&highlighter));
}

/// Tokenizes UTF-16 `code` after grammar state `state` (0: initial). Returns a
/// pointer to u32s: [state handle (0: initial), line count, then per line the
/// token count and (offset, length, style) per token], or null on error.
///
/// # Safety
/// `ptr..ptr+len*2` must be readable UTF-16.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rhl_tokenize(highlighter: u32, ptr: *const u16, len: usize, state: u32) -> *const u32 {
    let code = unsafe { std::slice::from_raw_parts(ptr, len) };
    EXPORTS.with_borrow_mut(|e| {
        let previous = if state == 0 { None } else { e.states.get(&state).cloned() };
        let Some(h) = e.highlighters.get_mut(&highlighter) else {
            e.set_text("unknown highlighter");
            return std::ptr::null();
        };
        match h.tokenize(code, previous.as_ref()) {
            Ok(tokenized) => {
                let state_id = match tokenized.state {
                    Some(state) => {
                        let id = e.id();
                        e.states.insert(id, state);
                        id
                    }
                    None => 0,
                };
                let result = &mut e.result;
                result.clear();
                result.push(state_id);
                result.push(tokenized.lines.len() as u32);
                for line in &tokenized.lines {
                    result.push(line.len() as u32);
                    for token in line {
                        result.extend_from_slice(&[token.offset, token.length, token.style]);
                    }
                }
                e.result.as_ptr()
            }
            Err(error) => {
                e.set_text(&error);
                std::ptr::null()
            }
        }
    })
}

/// Releases a grammar state handle.
#[unsafe(no_mangle)]
pub extern "C" fn rhl_state_free(state: u32) {
    EXPORTS.with_borrow_mut(|e| e.states.remove(&state));
}

/// The `htmlStyle` objects of styles `from..` of a highlighter, as a JSON array
/// of [property, value] pair arrays. Sets the text buffer; returns its length.
#[unsafe(no_mangle)]
pub extern "C" fn rhl_styles(highlighter: u32, from: u32) -> usize {
    EXPORTS.with_borrow_mut(|e| {
        let Some(h) = e.highlighters.get(&highlighter) else { return 0 };
        let styles: Vec<Vec<[&str; 2]>> = h.styles[from as usize..].iter().map(|s| s.iter().map(|(k, v)| [k.as_str(), v.as_str()]).collect()).collect();
        let text = serde_json::to_string(&styles).unwrap_or_default();
        e.set_text(&text);
        e.text.len()
    })
}

/// The `fg`, `bg` and `themeName` of a highlighter's results, as a JSON array.
/// Sets the text buffer; returns its length.
#[unsafe(no_mangle)]
pub extern "C" fn rhl_colors(highlighter: u32) -> usize {
    EXPORTS.with_borrow_mut(|e| {
        let Some(h) = e.highlighters.get(&highlighter) else { return 0 };
        let (light, dark) = (h.theme(0), h.theme(1));
        let colors = [
            format!("{};--shiki-dark:{}", light.fg, dark.fg),
            format!("{};--shiki-dark-bg:{}", light.bg, dark.bg),
            format!("shiki-themes {} {}", light.name, dark.name),
        ];
        let text = serde_json::to_string(&colors).unwrap_or_default();
        e.set_text(&text);
        e.text.len()
    })
}

/// Pointer to the text buffer set by the last call that returned text or failed.
#[unsafe(no_mangle)]
pub extern "C" fn rhl_text() -> *const u8 {
    EXPORTS.with_borrow(|e| e.text.as_ptr())
}

/// Length of the text buffer (the last error message after a failure).
#[unsafe(no_mangle)]
pub extern "C" fn rhl_text_len() -> usize {
    EXPORTS.with_borrow(|e| e.text.len())
}
