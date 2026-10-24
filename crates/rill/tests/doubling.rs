//! Doubling checks: every family in tests/common/families.rs (normal and adversarial
//! documents) renders at n, 2n, 4n and 8n bytes, and the work may grow about linearly
//! but no faster. Work is counted twice: bytes allocated (exact) and time (the best of a
//! few runs, for rescans that allocate nothing); either may grow 2.5 times as fast as the
//! text (buffers grow by doubling, machines are noisy), a fraction of what quadratic work
//! grows. `cargo run --release -p rill --example doubling`
//! prints the same families at larger sizes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

mod common;

use common::families::FAMILIES;

struct Counting;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
/// Bytes of fresh allocations only (not a buffer growing in place, which a stream's text
/// does, by doubling, in whichever update it crosses a power of two).
static FRESH: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator, counting the bytes asked for.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        FRESH.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATED.fetch_add(new_size, Ordering::Relaxed);
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

const SIZES: [usize; 4] = [1_000, 2_000, 4_000, 8_000];

/// Bytes allocated and the best time (ms) of `run`.
fn measure(run: &dyn Fn()) -> (usize, f64) {
    let before = ALLOCATED.load(Ordering::Relaxed);
    run();
    let bytes = ALLOCATED.load(Ordering::Relaxed) - before;
    let mut best = f64::INFINITY;
    for _ in 0..3 {
        let start = Instant::now();
        run();
        best = best.min(start.elapsed().as_secs_f64() * 1e3);
    }
    (bytes, best)
}

/// A stage's name and what it runs on a document.
type Stage<'a> = (&'static str, &'a dyn Fn(&str));

#[test]
fn every_stage_grows_linearly() {
    let markup = rill::MarkupOptions {
        link_safety: true,
        katex: true,
        ..rill::MarkupOptions::default()
    };
    let mut failures = Vec::new();
    let only = std::env::var("FAMILY").unwrap_or_default();
    for family in FAMILIES.iter().filter(|family| family.name.contains(&only)) {
        let options = rill::Options {
            math: family.math,
            cjk: family.cjk,
            close_open_raw: true,
            ..rill::Options::default()
        };
        let docs: Vec<String> = SIZES.iter().map(|&n| (family.build)(n)).collect();
        let stages: [Stage<'_>; 3] = [
            ("markup", &|doc| {
                let _ = rill::render_markup(doc, &options, &markup);
            }),
            ("remend", &|doc| {
                let _ = rill::remend::remend(doc);
            }),
            ("splits_at", &|doc| {
                let middle = doc.encode_utf16().count() as u32 / 2;
                let _ = rill::mdast::splits_at(doc, options.syntax(), middle);
            }),
        ];
        for (stage, run) in stages {
            let work: Vec<(usize, f64)> = docs.iter().map(|doc| measure(&|| run(doc))).collect();
            let (bytes, times): (Vec<usize>, Vec<f64>) = work.into_iter().unzip();
            let size = docs[3].len() as f64 / docs[0].len() as f64;
            // Linear work is `size` times as much (about 8); quadratic, its square.
            // Buffers grow by doubling, so bytes may be up to twice that.
            let grew_bytes = bytes[3] as f64 / bytes[0].max(1) as f64;
            let grew_time = times[3] / times[0].max(0.05);
            if grew_bytes > 2.5 * size || grew_time > 2.5 * size {
                failures.push(format!(
                    "{} / {stage}: bytes {bytes:?}, ms {times:.3?}",
                    family.name
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "faster than linear:\n{}",
        failures.join("\n")
    );
}

/// Bytes freshly allocated and the best time (ms) of healing the last `TIMED` updates of `text`
/// streamed `CHUNK` bytes at a time.
fn last_heals(text: &str) -> (usize, f64) {
    const CHUNK: usize = 24;
    const TIMED: usize = 20;
    let ends: Vec<usize> = (1..)
        .map(|index| (index * CHUNK).min(text.len()))
        .take_while(|&end| end < text.len())
        .chain([text.len()])
        .filter(|&end| text.is_char_boundary(end))
        .collect();
    let mut best = (usize::MAX, f64::INFINITY);
    for _ in 0..3 {
        let mut stream = rill::remend::Stream::default();
        let mut start = 0;
        let (mut bytes, mut time) = (0, Instant::now());
        for (index, &end) in ends.iter().enumerate() {
            if index == ends.len() - TIMED {
                bytes = FRESH.load(Ordering::Relaxed);
                time = Instant::now();
            }
            let _ = stream.heal(&text[start..end], index == 0);
            start = end;
        }
        let elapsed = time.elapsed().as_secs_f64() * 1e3;
        best = (
            best.0.min(FRESH.load(Ordering::Relaxed) - bytes),
            best.1.min(elapsed),
        );
    }
    best
}

#[test]
fn a_streaming_heal_follows_the_block_being_written() {
    let bounded = [
        "prose",
        "many code blocks",
        "many headings",
        "many definitions",
        "html blocks",
    ];
    let mut failures = Vec::new();
    for family in FAMILIES
        .iter()
        .filter(|family| bounded.contains(&family.name))
    {
        let work: Vec<(usize, f64)> = [2_000, 4_000, 8_000, 16_000]
            .iter()
            .map(|&n| last_heals(&(family.build)(n)))
            .collect();
        let (bytes, times): (Vec<usize>, Vec<f64>) = work.into_iter().unzip();
        // The same work at every size, give or take a buffer's doubling.
        if bytes[3] as f64 > 2.5 * bytes[0] as f64 + 4096.0 || times[3] > 3.0 * times[0].max(0.05) {
            failures.push(format!("{}: bytes {bytes:?}, ms {times:.3?}", family.name));
        }
    }
    assert!(
        failures.is_empty(),
        "a heal's work grows with the text:\n{}",
        failures.join("\n")
    );
}
