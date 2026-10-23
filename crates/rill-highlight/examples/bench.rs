//! `cargo run --release --example bench -- DATA_DIR LANG FILE...`: prints JSON with
//! the cold time (load grammar + themes, first tokenize) and the warm time of
//! tokenizing the files (concatenated) with a fresh highlighter state each run.
use rill_highlight::Highlighter;
use serde_json::{Value, json};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (data, lang) = (&args[1], &args[2]);
    let code: String = args[3..].iter().map(|p| std::fs::read_to_string(p).unwrap()).collect::<Vec<_>>().join("\n");
    let code: Vec<u16> = code.encode_utf16().collect();
    let start = Instant::now();
    let languages: Value = serde_json::from_str(&std::fs::read_to_string(format!("{data}/languages.json")).unwrap()).unwrap();
    let entry = &languages[lang.as_str()];
    let files: Vec<String> = entry["grammars"].as_array().unwrap().iter().map(|n| std::fs::read_to_string(format!("{data}/grammars/{}.json", n.as_str().unwrap())).unwrap()).collect();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let light = std::fs::read_to_string(format!("{data}/themes/github-light.json")).unwrap();
    let dark = std::fs::read_to_string(format!("{data}/themes/github-dark.json")).unwrap();
    let load_start = Instant::now();
    let mut highlighter = Highlighter::new(&refs, entry["name"].as_str().unwrap(), [&light, &dark]).unwrap();
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
    highlighter.tokenize(&code, None).unwrap();
    let cold_ms = start.elapsed().as_secs_f64() * 1000.0;
    let execs_before = highlighter.regex_execs();
    highlighter.tokenize(&code, None).unwrap();
    let execs = highlighter.regex_execs() - execs_before;
    let bench = Instant::now();
    let mut runs = 0;
    while bench.elapsed().as_millis() < 500 || runs < 3 {
        std::hint::black_box(highlighter.tokenize(&code, None).unwrap());
        runs += 1;
    }
    let warm_ms = bench.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    println!("{}", json!({ "lang": lang, "loadMs": load_ms, "coldMs": cold_ms, "warmMs": warm_ms, "execs": execs }));
}
