//! Conformance driver: reads records of `<byte length>\n<source>` from stdin and writes one
//! line per record, `{"mdast":…,"hast":{"kind":…,"tree":…}}` or `{"error":"…"}`. `--math`
//! and `--cjk` turn on Streamdown's `plugins.math` and `plugins.cjk` (`--single-dollar`:
//! remark-math's default `singleDollarTextMath`); `--breaks`, `--slug` and `--external`
//! remark-breaks, rehype-slug and rehype-external-links with `target: "_blank"`; `--classic` is
//! Streamdown 1.x's default plugins (conformance/pipeline.mjs `CLASSIC`).

use std::io::{self, BufRead, Read, Write};

fn main() -> io::Result<()> {
    let flags: Vec<String> = std::env::args().skip(1).collect();
    let options = if flags.iter().any(|flag| flag == "--classic") {
        // conformance/pipeline.mjs `CLASSIC`: Streamdown 1.x's default plugins.
        rill::Options {
            math: true,
            legacy_cjk: true,
            code_meta: false,
            sanitize: rill::Sanitize {
                code_classes: &["math", "math-display", "math-inline"],
                schema: rill::Schema::Default,
            },
            harden: rill::Harden {
                link_policy: rill::BlockPolicy::TextOnly,
                ..rill::Harden::default()
            },
            ..rill::Options::default()
        }
    } else {
        let flag = |name: &str| flags.iter().any(|flag| flag == name);
        rill::Options {
            math: flag("--math"),
            cjk: flag("--cjk"),
            single_dollar: flag("--single-dollar"),
            breaks: flag("--breaks"),
            slug: flag("--slug").then(rill::extras::Slug::default),
            external_links: flag("--external").then(|| rill::extras::ExternalLinks {
                target: Some("_blank".to_owned()),
                ..rill::extras::ExternalLinks::default()
            }),
            ..rill::Options::default()
        }
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
