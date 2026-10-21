//! The README's examples, and every bundled example rendering without error.

use rill::{Harden, Kind};

#[test]
fn readme() -> Result<(), rill::Error> {
    let (kind, hast_json) = rill::render("# Hello *world*", &Harden::default())?;
    assert_eq!(kind, Kind::Complete);
    assert!(hast_json.contains(r#""tagName":"em""#));
    let mdast_json = rill::parse_json("- [x] done")?;
    assert!(mdast_json.contains(r#""checked":true"#));
    Ok(())
}

#[test]
fn examples_render() -> Result<(), rill::Error> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");
    for entry in std::fs::read_dir(dir).expect("examples") {
        let source = std::fs::read_to_string(entry.expect("entry").path()).expect("read");
        rill::render(&source, &Harden::default())?;
        rill::parse_json(&source)?;
    }
    Ok(())
}
