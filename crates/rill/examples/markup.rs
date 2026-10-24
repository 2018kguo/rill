//! `cargo run --release -p rill --example markup -- FILE [ITERATIONS]`: renders FILE to
//! Streamdown's DOM markup repeatedly (for profilers) and prints the median time.

use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a Markdown file");
    let iterations: usize = args
        .next()
        .map_or(2000, |count| count.parse().expect("a count"));
    let source = std::fs::read_to_string(path).expect("readable file");
    let options = rill::Options::default();
    let markup = rill::MarkupOptions {
        link_safety: true,
        katex: true,
        ..rill::MarkupOptions::default()
    };
    let time = |label: &str, run: &dyn Fn()| {
        let mut samples: Vec<f64> = (0..iterations)
            .map(|_| {
                let start = Instant::now();
                run();
                start.elapsed().as_secs_f64() * 1e6
            })
            .collect();
        samples.sort_by(f64::total_cmp);
        println!("{label:<10} {:>8.1} µs", samples[samples.len() / 2]);
    };
    println!("{} bytes", source.len());
    time("mdast", &|| {
        rill::mdast::parse_arena(&source, options.syntax()).expect("parse");
    });
    time("hast ops", &|| {
        rill::render_ops(&source, &options).expect("render");
    });
    time("markup", &|| {
        rill::render_markup(&source, &options, &markup).expect("render");
    });
}
