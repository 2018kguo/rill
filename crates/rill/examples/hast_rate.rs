//! MiB/s of building HAST (parse + MDAST + HAST) over files.
fn main() {
    let options = rill::Options::default();
    for path in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&path).unwrap();
        let rounds = (100_000_000 / text.len().max(1)).clamp(3, 2000);
        let start = std::time::Instant::now();
        let mut nodes = 0usize;
        for _ in 0..rounds {
            nodes += rill::markup::markup(&text, &options, &rill::markup::MarkupOptions::default()).map_or(0, |m| m.ops.len());
        }
        let secs = start.elapsed().as_secs_f64();
        println!("{:24} {:7.1} MiB/s  {} nodes", path.rsplit('/').next().unwrap(), (text.len() * rounds) as f64 / secs / 1048576.0, nodes / rounds);
    }
}
