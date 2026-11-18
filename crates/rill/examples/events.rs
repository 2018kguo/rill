//! Raw rill-cmark event throughput (no trees): `cargo run --release -p rill --example events -- FILE`.
use std::time::Instant;
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let source = std::fs::read_to_string(path).unwrap();
    let options = rill::Options::default();
    let syntax = options.syntax();
    let mut best = f64::MAX;
    for _ in 0..300 {
        let t = Instant::now();
        let mut n = 0usize;
        for (event, range) in rill_cmark::Parser::with_footnotes(&source, syntax, Default::default()).into_offset_iter() {
            n += range.end + std::mem::size_of_val(&event);
        }
        std::hint::black_box(n);
        best = best.min(t.elapsed().as_secs_f64());
    }
    println!("{} bytes: {:.1} µs, {:.0} MiB/s", source.len(), best * 1e6, source.len() as f64 / best / 1048576.0);
}
