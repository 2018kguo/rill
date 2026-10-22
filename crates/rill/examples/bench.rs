//! `cargo run --release -p rill --example bench`: parse and render the bundled examples
//! without math or raw HTML (those hand off to JavaScript) repeated to about 100 KB,
//! reporting the median of many runs. `conformance/bench.mjs` times JavaScript on the same.

use std::time::Instant;

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("examples directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .filter(|path| {
            let name = path.to_string_lossy();
            !name.contains("math") && !name.contains("html")
        })
        .collect();
    entries.sort();
    let one: String = entries
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("example"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let source = one.repeat((100_000 / one.len()).max(1));
    let options = rill::Options::default();
    let kind = rill::render(&source, &options).expect("render").kind;
    assert_eq!(
        kind,
        rill::Kind::Complete,
        "the bench document runs entirely in Rust"
    );
    let time = |label: &str, run: &dyn Fn()| {
        let mut samples: Vec<f64> = (0..60)
            .map(|_| {
                let start = Instant::now();
                run();
                start.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        samples.sort_by(f64::total_cmp);
        println!("{label:<22} {:>8.3} ms", samples[samples.len() / 2]);
    };
    println!("{} KB of Markdown", source.len() / 1024);
    time("MDAST (arena)", &|| {
        rill::mdast::parse_arena(&source).expect("parse");
    });
    time("MDAST JSON", &|| {
        rill::parse_json(&source).expect("parse");
    });
    time("HAST opcodes", &|| {
        rill::render_ops(&source, &options).expect("render");
    });
    time("HAST JSON", &|| {
        rill::render_json(&source, &options).expect("render");
    });
}
