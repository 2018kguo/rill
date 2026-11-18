//! Dev probe: for each update of a 256-byte stream of FILE whose edit removes nodes, the
//! old and new open-tail HTML around the first difference.
fn main() {
    let source = std::fs::read_to_string(std::env::args().nth(1).expect("file")).unwrap();
    let mut session = rill_stream::Session::default();
    let (mut start, mut shown, mut n) = (0, String::new(), 0);
    while start < source.len() {
        let mut end = (start + 256).min(source.len());
        while !source.is_char_boundary(end) { end += 1; }
        session.update(&source[start..end], start == 0);
        let new = session.html().0.to_string() + session.html().1;
        if session.program.ops.contains(&1) && n < 12 {
            let at = shown.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
            let lo = at.saturating_sub(60);
            let cut = |s: &str, a: usize, b: usize| { let mut a = a.min(s.len()); while !s.is_char_boundary(a) { a -= 1; } let mut b = b.min(s.len()); while !s.is_char_boundary(b) { b += 1; } s[a..b].to_string() };
            println!("OLD …{:?}\nNEW …{:?}\n", cut(&shown, lo, at + 50), cut(&new, lo, at + 50));
            n += 1;
        }
        shown = session.html().1.to_string();
        start = end;
    }
}
