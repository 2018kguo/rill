//! Document families for the doubling checks (tests/doubling.rs and
//! examples/doubling.rs): each builds a Markdown text of about `n` bytes, normal or
//! adversarial for some scanner (long lines, deep nesting, many delimiters, many
//! definitions, huge tables, unclosed constructs).

pub struct Family {
    pub name: &'static str,
    pub build: fn(usize) -> String,
    pub math: bool,
    pub cjk: bool,
}

/// `unit` repeated to about `n` bytes.
fn fill(unit: &str, n: usize) -> String {
    unit.repeat(n.div_ceil(unit.len()))
}

/// `n` bytes made of numbered `line(i)`s.
fn lines(n: usize, line: impl Fn(usize) -> String) -> String {
    let mut out = String::new();
    let mut index = 0;
    while out.len() < n {
        out.push_str(&line(index));
        index += 1;
    }
    out
}

const fn family(name: &'static str, build: fn(usize) -> String) -> Family {
    Family {
        name,
        build,
        math: false,
        cjk: false,
    }
}

pub const FAMILIES: &[Family] = &[
    family("prose", |n| {
        fill(
            "Paragraph with **bold**, *emphasis*, `code`, a [link](https://example.com/a_b) \
             and a_snake_case word.\n\n- item with ~~strike~~\n- item two\n  1. nested\n\n> \
             a quote with $5 and 3 > 2\n\n",
            n,
        )
    }),
    family("long line", |n| fill("word *em* **strong** `c` [l](u) ", n)),
    family("open emphasis", |n| fill("*a ", n)),
    family("open strong", |n| fill("**a ", n)),
    family("open underscores", |n| fill("_a ", n)),
    family("mixed runs", |n| fill("*a **b ***c _d __e ", n)),
    family("closers only", |n| fill("a* ", n)),
    family("open strike", |n| fill("~~a ", n)),
    family("open brackets", |n| fill("[a ", n)),
    family("close brackets", |n| fill("a] ", n)),
    family("open links", |n| fill("[a](", n)),
    family("open images", |n| fill("![a](b ", n)),
    family("bracket pairs", |n| fill("[a] ", n)),
    family("nested brackets", |n| {
        format!("{}x{}", "[".repeat(n / 2), "]".repeat(n / 2))
    }),
    family("backtick runs", |n| {
        lines(n, |i| format!("{} a ", "`".repeat(i % 40 + 1)))
    }),
    family("open backticks", |n| fill("`` a ", n)),
    family("open angles", |n| fill("<a ", n)),
    family("lone angles", |n| fill("< ", n)),
    family("open autolinks", |n| fill("<http://a ", n)),
    family("html comments open", |n| fill("<!-- a ", n)),
    family("escapes", |n| fill("\\* \\[ \\` ", n)),
    family("entities", |n| fill("&amp; &#123; &copy; &bogus ", n)),
    family("www literals", |n| fill("www.a.com/b(c) ", n)),
    family("url parens", |n| format!("http://a.com/{}", "(".repeat(n))),
    family("emails", |n| fill("a.b@c.d ", n)),
    family("long url", |n| format!("https://a.com/{}", "a".repeat(n))),
    family("link with long destination", |n| {
        format!("[a]({})", "b".repeat(n))
    }),
    family("link with parens destination", |n| {
        format!("[a]({}{})", "(".repeat(n / 2), ")".repeat(n / 2))
    }),
    family("deep quotes", |n| format!("{}x\n", "> ".repeat(n / 2))),
    family("deep quotes lines", |n| {
        let depth = (n as f64).sqrt() as usize;
        lines(n, |i| format!("{}x\n", "> ".repeat(i % depth + 1)))
    }),
    family("deep lists", |n| format!("{}x\n", "- ".repeat(n / 2))),
    family("deep ordered lists", |n| {
        format!("{}x\n", "1. ".repeat(n / 3))
    }),
    family("deep list lines", |n| {
        let depth = 40;
        lines(n, |i| format!("{}- x\n", "  ".repeat(i % depth)))
    }),
    family("many list items", |n| lines(n, |i| format!("- item {i}\n"))),
    family("loose list", |n| lines(n, |i| format!("1. item {i}\n\n"))),
    family("task list", |n| {
        lines(n, |i| {
            format!("- [{}] task {i}\n", if i % 2 == 0 { " " } else { "x" })
        })
    }),
    family("table rows", |n| {
        format!(
            "| a | b | c |\n|---|:-:|--:|\n{}",
            lines(n, |i| format!("| {i} | *x* | `y` |\n"))
        )
    }),
    family("table wide", |n| {
        let columns = n / 8;
        format!(
            "|{}\n|{}\n|{}\n",
            "a|".repeat(columns),
            "-|".repeat(columns),
            "b|".repeat(columns)
        )
    }),
    family("table open cells", |n| {
        format!(
            "| a | b |\n|---|---|\n{}",
            lines(n, |i| format!("| {i} \n"))
        )
    }),
    family("many definitions", |n| {
        let refs = lines(n / 2, |i| format!("See [ref {i}] and [r{i}][ref {i}].\n\n"));
        let defs = lines(n / 2, |i| {
            format!("[ref {i}]: https://example.com/{i} \"t{i}\"\n")
        });
        format!("{refs}{defs}")
    }),
    family("definitions first", |n| {
        let defs = lines(n / 2, |i| format!("[d{i}]: /u{i}\n"));
        let refs = lines(n / 2, |i| format!("[d{i}] "));
        format!("{defs}\n{refs}")
    }),
    family("undefined references", |n| {
        lines(n, |i| format!("[x{i}][y{i}] "))
    }),
    family("footnotes", |n| {
        let refs = lines(n / 2, |i| format!("Text[^f{i}].\n\n"));
        let defs = lines(n / 2, |i| format!("[^f{i}]: Note {i}.\n"));
        format!("{refs}{defs}")
    }),
    family("many code blocks", |n| {
        lines(n, |i| format!("```ts\nconst x{i} = {i};\n```\n\n"))
    }),
    family("giant code block", |n| {
        format!(
            "```rust\n{}```\n",
            lines(n, |i| format!("let x{i} = {i};\n"))
        )
    }),
    family("unclosed fence", |n| {
        format!("```\n{}", lines(n, |i| format!("line {i}\n")))
    }),
    family("indented code", |n| lines(n, |i| format!("    code {i}\n"))),
    family("many headings", |n| {
        lines(n, |i| format!("## Heading {i}\n\ntext\n\n"))
    }),
    family("setext", |n| lines(n, |i| format!("Title {i}\n---\n\n"))),
    family("html blocks", |n| {
        lines(n, |i| format!("<div>\nblock {i}\n</div>\n\n"))
    }),
    family("nested html", |n| {
        format!("{}x{}", "<div>".repeat(n / 11), "</div>".repeat(n / 11))
    }),
    family("unclosed html", |n| {
        format!("<div>\n{}", lines(n, |i| format!("line {i}\n")))
    }),
    family("inline html", |n| {
        fill("a <b>c</b> <sup>1</sup> <kbd>k</kbd> ", n)
    }),
    // Nesting past `rill::mdast::MAX_DEPTH`: refused (left to JavaScript) at once, not by
    // a stack overflow (in WebAssembly, a trap leaves the instance unusable).
    family("nested inline html", |n| {
        format!("{}x{}", "<b>".repeat(n / 7), "</b>".repeat(n / 7))
    }),
    family("open inline html", |n| {
        format!("a {}x", "<sup>".repeat(n / 5))
    }),
    family("nested images", |n| {
        format!("{}a{}", "![".repeat(n / 6), "](u)".repeat(n / 6))
    }),
    family("nested emphasis", |n| {
        format!("{}a{}", "*".repeat(n / 2), "*".repeat(n / 2))
    }),
    family("hard breaks", |n| lines(n, |i| format!("line {i}  \n"))),
    family("one paragraph lines", |n| {
        lines(n, |i| format!("line {i} with *a* and `b`\n"))
    }),
    family("thematic breaks", |n| fill("***\n", n)),
    family("cr line endings", |n| fill("a *b* c\r\n\r\n- d\r", n)),
    Family {
        math: true,
        ..family("math", |n| {
            fill("Inline $$a^2$$ and\n\n$$\n\\sum_i x_i\n$$\n\n", n)
        })
    },
    Family {
        math: true,
        ..family("open math", |n| fill("$$a ", n))
    },
    Family {
        cjk: true,
        ..family("cjk", |n| {
            fill("**中文**。「引用」*強調*、https://x.com/a。 ", n)
        })
    },
    Family {
        cjk: true,
        ..family("cjk open emphasis", |n| fill("**中", n))
    },
];
