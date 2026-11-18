//! `cargo run --release -p rill-stream --example session -- FILE [CHUNK]`: streams FILE
//! through a Session and prints the time per update for each tenth of the stream.
use std::time::Instant;
fn main() {
    let mut args = std::env::args().skip(1);
    let source = std::fs::read_to_string(args.next().expect("file")).unwrap();
    let chunk: usize = args.next().map_or(256, |c| c.parse().unwrap());
    let mut ends = Vec::new();
    let mut end = 0;
    while end < source.len() {
        end = (end + chunk).min(source.len());
        while !source.is_char_boundary(end) { end += 1; }
        ends.push(end);
    }
    let mut best = vec![f64::MAX; ends.len()];
    for _ in 0..20 {
        let mut session = rill_stream::Session::default();
        let mut start = 0;
        for (index, &end) in ends.iter().enumerate() {
            let t = Instant::now();
            let outcome = session.update(&source[start..end], start == 0);
            best[index] = best[index].min(t.elapsed().as_secs_f64() * 1e6);
            assert_ne!(outcome, rill_stream::Outcome::Fallback, "fell back at {end}");
            start = end;
        }
    }
    let tenth = ends.len().div_ceil(10);
    for (part, times) in best.chunks(tenth).enumerate() {
        println!("part {part}: {:.1} µs/update", times.iter().sum::<f64>() / times.len() as f64);
    }
    println!("total {:.0} µs for {} updates", best.iter().sum::<f64>(), ends.len());
}
