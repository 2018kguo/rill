//! rill-stream against the exact pipeline (`rill`, which matches Streamdown): over random
//! documents, whatever rill-stream renders has the same elements and text as the HAST
//! `rill` builds, after Streamdown's components (`MemoLi` renders an item holding only a
//! paragraph as the paragraph's content). Attributes and text-node splits are
//! js/test/stream*.test.ts's to check, against Streamdown itself.
//!   STRUCTURE_ROUNDS=1000000 cargo test --release -p rill-stream --test structure

use serde_json::Value;

const PIECES: [&str; 61] = [
    "www.",
    "https://",
    "a@b.co",
    ".",
    ")",
    "@",
    "~",
    "~~",
    "[ ] ",
    "[x] ",
    "a",
    "b",
    " ",
    "  ",
    "   ",
    "    ",
    "\n",
    "\n",
    "\n\n",
    "> ",
    ">",
    "- ",
    "* ",
    "1. ",
    "2) ",
    "# ",
    "---",
    "*",
    "**",
    "_",
    "`",
    "\\",
    "[x](/u)",
    "[",
    "]",
    "&amp;",
    "  \n",
    "x",
    "=",
    "+ ",
    "10. ",
    "é",
    "\n> ",
    "\n  ",
    "```",
    "\n```\n",
    "~~~",
    "```js",
    "| ",
    "  ```",
    "|",
    "--",
    ":",
    "-|-",
    "\n|--|\n",
    "| a |",
    "[^a]",
    "\n[^a]: ",
    "[c]",
    "\n[c]: /u",
    "[c][]",
];

#[test]
fn same_structure_as_the_exact_pipeline() {
    let rounds = std::env::var("STRUCTURE_ROUNDS").map_or(20_000, |n| n.parse().unwrap());
    let mut seed =
        std::env::var("STRUCTURE_SEED").map_or(0x2545_f491_4f6c_dd1du64, |s| s.parse().unwrap());
    let mut next = |bound: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % bound as u64) as usize
    };
    // STRUCTURE_TEXT: one text, both renders printed.
    if let Ok(text) = std::env::var("STRUCTURE_TEXT") {
        let text = text.replace("\\n", "\n");
        println!("ours   {:?}\ntheirs {:?}", ours(&text), theirs(&text));
        return;
    }
    let (mut rendered, mut total) = (0, 0);
    for _ in 0..rounds {
        let mut text = String::new();
        for _ in 0..1 + next(24) {
            text.push_str(PIECES[next(PIECES.len())]);
        }
        total += 1;
        let Some(ours) = ours(&text) else { continue };
        let Some(theirs) = theirs(&text) else {
            continue;
        };
        rendered += 1;
        assert_eq!(ours, theirs, "{text:?}");
    }
    eprintln!("rendered {rendered}/{total}");
}

/// rill-stream's rendering, as nested tags and merged text.
fn ours(text: &str) -> Option<String> {
    let mut html = String::new();
    rill_stream::render(text, &mut html).ok()?;
    // A table: the table alone, without its wrapper, controls and scroll container.
    while let Some(start) = html.find(r#"<div class="my-4 flex flex-col gap-2 rounded-lg"#) {
        let table = start + html[start..].find("<table").unwrap();
        let end = table + html[table..].find("</table></div></div>").unwrap();
        html = format!(
            "{}{}</table>{}",
            &html[..start],
            &html[table..end],
            &html[end + 20..]
        );
    }
    // Parse our own HTML back: tags by name, text unescaped.
    let mut out = String::new();
    let mut rest = html.as_str();
    while !rest.is_empty() {
        // A code block: its language and lines, as `pre > code`.
        if let Some(after) = rest.strip_prefix(r#"<div class="relative my-4"#) {
            let language = &after[after.find("data-language=\"").unwrap() + 15..];
            let language = &language[..language.find('"').unwrap()];
            let body = &after[after.find("<code class=").unwrap()..];
            let body =
                &body[body.find('>').unwrap() + 1..body.find("</code></pre></div></div>").unwrap()];
            let lines: Vec<String> = body
                .split(r#"<span class="block "#)
                .skip(1)
                .map(|line| {
                    let line = &line[line.find('>').unwrap() + 1..];
                    let text = match line.strip_prefix("<span") {
                        Some(token) => {
                            &token[token.find('>').unwrap() + 1..token.find("</span>").unwrap()]
                        }
                        None => &line[..line.find("</span>").unwrap()],
                    };
                    if text == "\n" {
                        String::new()
                    } else {
                        text.to_owned()
                    }
                })
                .collect();
            out.push_str(&format!(
                "<pre><code {}>{}</code></pre>",
                unescape(language).replace("&quot;", "\""),
                unescape(&lines.join("\n"))
            ));
            rest = &after[after.find("</code></pre></div></div>").unwrap() + 25..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("</") {
            let end = after.find('>').unwrap();
            out.push_str(&format!("</{}>", hast_name(&after[..end])));
            rest = &after[end + 1..];
        } else if let Some(after) = rest.strip_prefix('<') {
            let end = after.find('>').unwrap();
            let name = &after[..after[..end].find([' ', '>']).unwrap_or(end)];
            out.push_str(&format!("<{}>", hast_name(name)));
            if matches!(name, "br" | "hr" | "img" | "input") {
                out.push_str(&format!("</{name}>"));
            }
            rest = &after[end + 1..];
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            out.push_str(&unescape(&rest[..end]));
            rest = &rest[end..];
        }
    }
    Some(out)
}

fn unescape(html: &str) -> String {
    html.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The HAST tag Streamdown's component for `name` renders from.
fn hast_name(name: &str) -> &str {
    match name {
        "button" => "a",
        other => other,
    }
}

/// `rill`'s HAST of each block Streamdown's split makes (rill-stream's cuts, which the JS
/// tests check against Streamdown's), as nested tags and merged text, with Streamdown's
/// components applied.
fn theirs(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut from = 0;
    for cut in rill_stream::block_cuts(text)
        .into_iter()
        .chain([text.len()])
    {
        out.push_str(&block(&text[from..cut])?);
        from = cut;
    }
    Some(out)
}

fn block(text: &str) -> Option<String> {
    let json = rill::hast::render_json(text, &rill::Options::default()).ok()?;
    let value: Value = serde_json::from_str(&json).ok()?;
    if value["kind"] != "complete" {
        return None;
    }
    let mut out = String::new();
    // Streamdown renders blocks back to back; within one, the root's line endings
    // between its children stay.
    for child in value["tree"]["children"].as_array()? {
        write(child, &mut out)?;
    }
    Some(out)
}

fn write(node: &Value, out: &mut String) -> Option<()> {
    match node["type"].as_str()? {
        "text" => out.push_str(node["value"].as_str()?),
        "element" if node["tagName"] == "pre" => {
            // Streamdown's code block: the code without its trailing line endings.
            let code = &node["children"][0];
            let language = code["properties"]["className"]
                .as_array()
                .and_then(|names| names.first())
                .and_then(|name| name.as_str())
                .map_or("", |name| name.strip_prefix("language-").unwrap_or(name));
            let text = code["children"]
                .as_array()?
                .iter()
                .map(|child| child["value"].as_str())
                .collect::<Option<String>>()?;
            out.push_str(&format!(
                "<pre><code {language}>{}</code></pre>",
                text.trim_end_matches('\n')
            ));
        }
        "element" => {
            let tag = node["tagName"].as_str()?;
            // MarkdownSection: a footnote item holding only its back references is dropped.
            let backref = |child: &Value| child["properties"].get("dataFootnoteBackref").is_some();
            if tag == "li"
                && node["children"].as_array()?.iter().all(|child| {
                    child["value"]
                        .as_str()
                        .is_some_and(|text| text.trim().is_empty())
                        || backref(child)
                })
                && node["children"].as_array()?.iter().any(backref)
            {
                return Some(());
            }
            // `strong` renders as a span; the HTML parse above names it by tag.
            let shown = if tag == "strong" { "span" } else { tag };
            out.push_str(&format!("<{shown}>"));
            let children = node["children"].as_array()?;
            let kept: Vec<&Value> = children
                .iter()
                .filter(|c| !(c["type"] == "text" && c["value"] == "\n"))
                .collect();
            // MemoLi: an item holding only a paragraph renders the paragraph's content.
            if tag == "li" && kept.len() == 1 && kept[0]["tagName"] == "p" {
                for child in kept[0]["children"].as_array()? {
                    write(child, out)?;
                }
            } else {
                for child in children {
                    write(child, out)?;
                }
            }
            out.push_str(&format!("</{shown}>"));
        }
        _ => return None,
    }
    Some(())
}
