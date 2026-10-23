//! Replays RegExp exec traces recorded from Shiki (highlight/bench/profile-shiki.mjs)
//! through the Rust engine, checks every result against V8's and times the replay.
use rill_highlight::regex::{EmulationOptions, JsRegExp, Matcher};
use serde_json::Value;
use std::time::Instant;

fn main() {
    let mut total_ms = 0.0;
    let mut total_bad = 0;
    let mut total_calls = 0;
    for path in std::env::args().skip(1) {
        let trace: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let regexes: Vec<Option<JsRegExp>> = trace["regexes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let options = &r["options"];
                let numbers = |v: &Value| v.as_array().map(|a| a.iter().map(|x| x.as_u64().unwrap() as u32).collect::<Vec<_>>()).unwrap_or_default();
                let emulation = EmulationOptions {
                    hidden_captures: numbers(&options["hiddenCaptures"]),
                    transfers: options["transfers"].as_array().map(|a| a.iter().map(|t| (t[0].as_u64().unwrap() as u32, numbers(&t[1]))).collect()).unwrap_or_default(),
                    clip_search: options["strategy"].as_str() == Some("clip_search"),
                };
                match JsRegExp::new(r["pattern"].as_str().unwrap(), r["flags"].as_str().unwrap(), &emulation) {
                    Ok(regex) => Some(regex),
                    Err(e) => {
                        eprintln!("{path}: compile error {e}: {:.120}", r["pattern"].as_str().unwrap());
                        None
                    }
                }
            })
            .collect();
        let lines: Vec<Vec<u16>> = trace["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().encode_utf16().collect()).collect();
        let calls: Vec<usize> = trace["calls"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as usize).collect();
        let mut kept_results: Vec<Option<Vec<i32>>> = Vec::new();
        let results: Vec<Option<Vec<i32>>> = trace["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_array().map(|a| a.iter().map(|x| x.as_i64().unwrap() as i32).collect()))
            .collect();
        // With KEPT=1, only the calls a regex-wide per-line search cache would still make.
        let calls: Vec<usize> = if std::env::var("KEPT").is_ok() {
            let mut seen: std::collections::HashMap<(usize, usize), Vec<(usize, i32)>> = Default::default();
            let mut kept = Vec::new();
            for (i, call) in calls.chunks(3).enumerate() {
                let found = results[i].as_ref().map_or(-1, |r| r[0]);
                let cacheable = regexes[call[0]].as_ref().is_some_and(|r| r.is_search_cacheable());
                let list = seen.entry((call[0], call[1])).or_default();
                if !cacheable || !list.iter().any(|&(from, at)| from <= call[2] && (at == -1 || at as usize >= call[2])) {
                    kept.extend_from_slice(call);
                    kept_results.push(results[i].clone());
                }
                list.push((call[2], found));
            }
            kept
        } else {
            kept_results = results.clone();
            calls
        };
        let results = kept_results;
        let mut matcher = Matcher::default();
        let mut raw = Vec::new();
        let mut out = Vec::new();
        let mut run = |check: bool| -> usize {
            let mut bad = 0;
            for (i, call) in calls.chunks(3).enumerate() {
                let Some(regex) = &regexes[call[0]] else { continue };
                let found = regex.exec(&mut matcher, &lines[call[1]], call[2], &mut raw, &mut out);
                if check {
                    let got = if found { Some(out.clone()) } else { None };
                    if got != results[i] {
                        bad += 1;
                        if bad <= 3 {
                            eprintln!("{path}: mismatch start {} got {got:?} want {:?}\n  {:.300}\n  line {:?}", call[2], results[i], trace["regexes"][call[0]]["pattern"].as_str().unwrap(), String::from_utf16_lossy(&lines[call[1]]));
                        }
                    }
                } else {
                    std::hint::black_box(found);
                }
            }
            bad
        };
        let bad = run(true);
        if std::env::var("TOP").is_ok() {
            let (mut m2, mut r2, mut o2) = (Matcher::default(), Vec::new(), Vec::new());
            let mut per_regex = vec![(0.0f64, 0usize); regexes.len()];
            for _ in 0..3 {
                for call in calls.chunks(3) {
                    let Some(regex) = &regexes[call[0]] else { continue };
                    let t = Instant::now();
                    regex.exec(&mut m2, &lines[call[1]], call[2], &mut r2, &mut o2);
                    per_regex[call[0]].0 += t.elapsed().as_secs_f64() * 1000.0 / 3.0;
                    per_regex[call[0]].1 += 1;
                }
            }
            let mut order: Vec<usize> = (0..regexes.len()).collect();
            order.sort_by(|&a, &b| per_regex[b].0.partial_cmp(&per_regex[a].0).unwrap());
            for &i in order.iter().take(12) {
                println!("  {:8.2} ms {:7} calls  #{i} {:.160}", per_regex[i].0, per_regex[i].1 / 3, trace["regexes"][i]["pattern"].as_str().unwrap());
            }
        }
        let start = Instant::now();
        let mut iterations = 0;
        while start.elapsed().as_millis() < 300 || iterations < 2 {
            run(false);
            iterations += 1;
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(iterations);
        total_ms += ms;
        total_bad += bad;
        total_calls += calls.len() / 3;
        println!("{:18} {:9.2} ms  calls {:7}  mismatches {}", path.rsplit('/').next().unwrap(), ms, calls.len() / 3, bad);
    }
    println!("TOTAL {total_ms:.1} ms  calls {total_calls}  mismatches {total_bad}");
}
