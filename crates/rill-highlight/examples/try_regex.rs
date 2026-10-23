//! `cargo run --example try_regex -- PATTERN FLAGS INPUT [START]`: prints the match.
use rill_highlight::regex::{EmulationOptions, JsRegExp, Matcher};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let regex = JsRegExp::new(&args[1], &args[2], &EmulationOptions::default()).unwrap();
    let input: Vec<u16> = args[3].encode_utf16().collect();
    let start = args.get(4).map_or(0, |s| s.parse().unwrap());
    let (mut raw, mut out) = (Vec::new(), Vec::new());
    let found = regex.exec(&mut Matcher::default(), &input, start, &mut raw, &mut out);
    println!("{}", if found { format!("{out:?}") } else { "null".into() });
}
