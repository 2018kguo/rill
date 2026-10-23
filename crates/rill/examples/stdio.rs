//! Conformance driver: reads records of `<byte length>\n<source>` from stdin and writes one
//! line per record, `{"mdast":…,"hast":{"kind":…,"tree":…}}` or `{"error":"…"}`. `--math`
//! and `--cjk` turn on Streamdown's `plugins.math` and `plugins.cjk`.

use std::io::{self, BufRead, Read, Write};

fn main() -> io::Result<()> {
    let flags: Vec<String> = std::env::args().skip(1).collect();
    let options = rill::Options {
        math: flags.iter().any(|flag| flag == "--math"),
        cjk: flags.iter().any(|flag| flag == "--cjk"),
        ..rill::Options::default()
    };
    let mut input = io::stdin().lock();
    let mut out = io::BufWriter::new(io::stdout().lock());
    let mut header = String::new();
    loop {
        header.clear();
        if input.read_line(&mut header)? == 0 {
            break;
        }
        let Ok(length) = header.trim().parse::<usize>() else {
            break;
        };
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes)?;
        let source = String::from_utf8_lossy(&bytes);
        match (
            rill::parse_json(&source, options.syntax()),
            rill::render_json(&source, &options),
        ) {
            (Ok(mdast), Ok(hast)) => writeln!(out, "{{\"mdast\":{mdast},\"hast\":{hast}}}")?,
            (Err(error), _) | (_, Err(error)) => {
                writeln!(out, "{{\"error\":\"{error}\"}}")?;
            }
        }
    }
    out.flush()
}
