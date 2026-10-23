//! `cargo run --example tokens -- DATA_DIR LANG FILE...`: prints, per file, one JSON
//! line with Shiki-shaped dual-theme tokens (github-light / github-dark).
use rill_highlight::Highlighter;
use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = &args[1];
    let lang = &args[2];
    let languages: Value = serde_json::from_str(&std::fs::read_to_string(format!("{data}/languages.json")).unwrap()).unwrap();
    let entry = &languages[lang.as_str()];
    let files: Vec<String> = entry["grammars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| std::fs::read_to_string(format!("{data}/grammars/{}.json", name.as_str().unwrap())).unwrap())
        .collect();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let light = std::fs::read_to_string(format!("{data}/themes/github-light.json")).unwrap();
    let dark = std::fs::read_to_string(format!("{data}/themes/github-dark.json")).unwrap();
    let mut highlighter = match Highlighter::new(&refs, entry["name"].as_str().unwrap(), [&light, &dark]) {
        Ok(h) => h,
        Err(e) => {
            println!("{}", json!({ "error": e }));
            return;
        }
    };
    for path in &args[3..] {
        let code: Vec<u16> = std::fs::read_to_string(path).unwrap().encode_utf16().collect();
        match highlighter.tokenize(&code, None) {
            Ok(result) => {
                let tokens: Vec<Value> = result
                    .lines
                    .iter()
                    .map(|line| {
                        Value::Array(
                            line.iter()
                                .map(|t| {
                                    let content = String::from_utf16_lossy(&code[t.offset as usize..(t.offset + t.length) as usize]);
                                    let style: serde_json::Map<String, Value> = highlighter.styles[t.style as usize].iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
                                    json!([content, t.offset, style])
                                })
                                .collect(),
                        )
                    })
                    .collect();
                let (l, d) = (highlighter.theme(0), highlighter.theme(1));
                println!(
                    "{}",
                    json!({ "file": path, "fg": format!("{};--shiki-dark:{}", l.fg, d.fg), "bg": format!("{};--shiki-dark-bg:{}", l.bg, d.bg), "themeName": format!("shiki-themes {} {}", l.name, d.name), "tokens": tokens })
                );
            }
            Err(e) => println!("{}", json!({ "file": path, "error": e })),
        }
    }
}
