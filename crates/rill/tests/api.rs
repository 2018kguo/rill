//! The README's examples, and every bundled example rendering without error.

use rill::{Kind, Options};

#[test]
fn readme() -> Result<(), rill::Error> {
    let rendered = rill::render("# Hello *world*", &Options::default())?;
    assert_eq!(rendered.kind, Kind::Complete);
    assert!(rendered.tree.contains(r#""tagName":"em""#));
    let mdast_json = rill::parse_json("- [x] done")?;
    assert!(mdast_json.contains(r#""checked":true"#));
    Ok(())
}

#[test]
fn examples_render() -> Result<(), rill::Error> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");
    for entry in std::fs::read_dir(dir).expect("examples") {
        let source = std::fs::read_to_string(entry.expect("entry").path()).expect("read");
        rill::render(&source, &Options::default())?;
        rill::parse_json(&source)?;
    }
    Ok(())
}

/// A rehype-style plugin: every `<h1>`–`<h6>` gets an `id` from its text.
struct HeadingIds;

impl rill::Plugin for HeadingIds {
    fn hast(&self, tree: &mut rill::hast::Tree<'_>, root: u32, _: &rill::Context<'_>) {
        let mut stack = vec![root];
        while let Some(ix) = stack.pop() {
            stack.extend(tree.child_list(ix));
            let heading = tree
                .tag_name(ix)
                .is_some_and(|tag| matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6"));
            if !heading {
                continue;
            }
            let text: String = tree
                .child_list(ix)
                .into_iter()
                .filter_map(|child| match &tree.nodes[child as usize].kind {
                    rill::hast::NodeKind::Text(value) => {
                        Some(value.to_lowercase().replace(' ', "-"))
                    }
                    _ => None,
                })
                .collect();
            if let Some(props) = tree.properties_mut(ix) {
                rill::hast::set(props, "id", rill::hast::Value::Str(text.into()));
            }
        }
    }
}

#[test]
fn plugin() -> Result<(), rill::Error> {
    let options = Options {
        plugins: vec![Box::new(HeadingIds)],
        ..Options::default()
    };
    let tree = rill::render("## Getting started", &options)?.tree;
    assert!(tree.contains(r#""id":"getting-started""#), "{tree}");
    Ok(())
}
