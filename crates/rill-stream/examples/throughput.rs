//! `cargo run --release -p rill-stream --example throughput -- FILE`: MiB/s of `render`.
use std::time::Instant;
fn main() {
    let source = std::fs::read_to_string(std::env::args().nth(1).expect("file")).unwrap();
    let mut out = String::new();
    if rill_stream::render(&source, &mut out).is_err() {
        println!("falls back");
        return;
    }
    let mut best = f64::MAX;
    for _ in 0..std::env::var("N").map_or(500, |n| n.parse().unwrap()) {
        out = String::new();
        let t = Instant::now();
        rill_stream::render(&source, &mut out).unwrap();
        best = best.min(t.elapsed().as_secs_f64());
    }
    println!("{} bytes: {:.1} µs, {:.0} MiB/s", source.len(), best * 1e6, source.len() as f64 / best / 1048576.0);
}
