//! Dev probe: where a Session's updates allocate (first rill frame of sampled
//! allocations). `cargo run --release -p rill-stream --example allocs -- FILE`
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

struct Sampling;
static ON: AtomicBool = AtomicBool::new(false);
static SITES: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);
thread_local!(static INSIDE: Cell<bool> = const { Cell::new(false) });
fn record() {
    if !ON.load(Relaxed) || INSIDE.with(|i| i.replace(true)) {
        return;
    }
    let trace = std::backtrace::Backtrace::force_capture().to_string();
    let frames: Vec<&str> = trace
        .lines()
        .filter(|l| l.contains("at ") && l.contains("/crates/rill"))
        .map(|l| l.trim().rsplit("/crates/").next().unwrap_or(l))
        .take(3)
        .collect();
    let key = frames.join(" <- ");
    let mut sites = SITES.lock().unwrap();
    *sites
        .get_or_insert_with(HashMap::new)
        .entry(key)
        .or_default() += 1;
    INSIDE.with(|i| i.set(false));
}
unsafe impl GlobalAlloc for Sampling {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static GLOBAL: Sampling = Sampling;

fn main() {
    let source = std::fs::read_to_string(std::env::args().nth(1).expect("file")).unwrap();
    let mut session = rill_stream::Session::default();
    let (mut start, mut updates) = (0, 0);
    while start < source.len() {
        let mut end = (start + 256).min(source.len());
        while !source.is_char_boundary(end) {
            end += 1;
        }
        // Warm the buffers on the first tenth, then sample.
        ON.store(start > source.len() / 10, Relaxed);
        updates += usize::from(start > source.len() / 10);
        session.update(&source[start..end], start == 0);
        start = end;
    }
    ON.store(false, Relaxed);
    let sites = SITES.lock().unwrap().take().unwrap_or_default();
    let mut sites: Vec<_> = sites.into_iter().collect();
    sites.sort_by_key(|site| std::cmp::Reverse(site.1));
    for (site, count) in sites.iter().take(25) {
        println!("{:5.2}/update {site}", *count as f64 / updates as f64);
    }
}
