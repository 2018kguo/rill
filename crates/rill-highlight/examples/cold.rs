//! `cargo run --release --example cold -- DATA_DIR LANG FILE`: repeats creating a
//! highlighter and tokenizing FILE once (the cold path), for profiling.
use rill_highlight::Highlighter;
use serde_json::Value;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (data, lang) = (&args[1], &args[2]);
    let code: Vec<u16> = std::fs::read_to_string(&args[3]).unwrap().encode_utf16().collect();
    let languages: Value = serde_json::from_str(&std::fs::read_to_string(format!("{data}/languages.json")).unwrap()).unwrap();
    let entry = &languages[lang.as_str()];
    let files: Vec<String> = entry["grammars"].as_array().unwrap().iter().map(|n| std::fs::read_to_string(format!("{data}/grammars/{}.json", n.as_str().unwrap())).unwrap()).collect();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let light = std::fs::read_to_string(format!("{data}/themes/github-light.json")).unwrap();
    let dark = std::fs::read_to_string(format!("{data}/themes/github-dark.json")).unwrap();
    let mut best = f64::MAX;
    for _ in 0..20 {
        let start = Instant::now();
        let mut highlighter = Highlighter::new(&refs, entry["name"].as_str().unwrap(), [&light, &dark]).unwrap();
        highlighter.tokenize(&code, None).unwrap();
        best = best.min(start.elapsed().as_secs_f64() * 1000.0);
    }
    println!("{lang} cold {best:.2} ms");
}
