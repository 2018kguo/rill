//! Prints rill-stream's HTML for each argument (`\n` escapes allowed).
fn main() {
    for text in std::env::args().skip(1) {
        let text = text.replace("\\n", "\n");
        let mut html = String::new();
        match rill_stream::render(&text, &mut html) {
            Ok(()) => println!("{html}"),
            Err(_) => println!("(fallback)"),
        }
    }
}
