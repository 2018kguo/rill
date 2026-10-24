//! `cargo run --release -p rill --example doubling [filter]`: times every stage on
//! normal and adversarial documents at n, 2n, 4n and 8n bytes and prints the growth
//! per doubling (about 2 is linear, 4 quadratic). tests/doubling.rs keeps a CI-sized
//! version of the same families.

use std::time::Instant;

#[path = "../tests/common/families.rs"]
mod families;

/// A stage's name and what it runs on a document.
type Stage<'a> = (&'static str, &'a dyn Fn(&str));

fn best(run: &dyn Fn()) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..5 {
        let start = Instant::now();
        run();
        best = best.min(start.elapsed().as_secs_f64() * 1e3);
    }
    best
}

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let base: usize = std::env::var("N")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(16_000);
    let sizes = [base, base * 2, base * 4, base * 8];
    let markup = rill::MarkupOptions {
        link_safety: true,
        katex: true,
        ..rill::MarkupOptions::default()
    };
    for family in families::FAMILIES {
        if !family.name.contains(&filter) {
            continue;
        }
        let options = rill::Options {
            math: family.math,
            cjk: family.cjk,
            close_open_raw: true,
            ..rill::Options::default()
        };
        let docs: Vec<String> = sizes.iter().map(|&n| (family.build)(n)).collect();
        let stages: [Stage<'_>; 5] = [
            ("markup", &|doc| {
                let _ = rill::render_markup(doc, &options, &markup);
            }),
            ("ops", &|doc| {
                let _ = rill::render_ops(doc, &options);
            }),
            ("remend", &|doc| {
                let _ = rill::remend::remend(doc);
            }),
            ("definitions", &|doc| {
                let _ = rill::mdast::root_definitions(doc, options.syntax());
            }),
            ("splits_at", &|doc| {
                let middle = doc.encode_utf16().count() as u32 / 2;
                let _ = rill::mdast::splits_at(doc, options.syntax(), middle);
            }),
        ];
        for (stage, run) in stages {
            let times: Vec<f64> = docs.iter().map(|doc| best(&|| run(doc))).collect();
            let worst = times
                .windows(2)
                .map(|pair| pair[1] / pair[0].max(1e-3))
                .fold(0.0, f64::max);
            let flag = if worst > 3.0 && times[3] > 1.0 {
                "SUPERLINEAR"
            } else {
                ""
            };
            println!(
                "{:<28} {:<12} {:>9.3} {:>9.3} {:>9.3} {:>9.3} ms  x{:<5.2} {flag}",
                family.name, stage, times[0], times[1], times[2], times[3], worst
            );
        }
    }
}
