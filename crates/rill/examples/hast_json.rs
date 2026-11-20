//! Prints the HAST JSON of each argument (`\n` escapes allowed).
fn main() {
    for text in std::env::args().skip(1) {
        let text = text.replace("\\n", "\n");
        println!("{}", rill::hast::render_json(&text, &rill::Options::default()).unwrap());
    }
}
