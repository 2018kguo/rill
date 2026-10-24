//! A custom rill build: Streamdown's pipeline with `$$` math of its own and three plugins,
//! each the Rust form of a JavaScript plugin in `../plugins.ts`:
//!
//! - [`Emoji`] (`remarkEmoji`): `:smile:`, `:tada:` and `:rocket:` in text.
//! - [`Mentions`] (`rehypeMentions`, before sanitize): `@name` links to `{base}/name`.
//! - [`Hashtags`] (`rehypeHashtags`, after KaTeX): `#tag` as a tagged `<span>`.
//!
//! `options` is the one definition every entry point renders with; JavaScript passes its
//! arguments as `engineArgs` (`../engine.ts`).

use std::borrow::Cow;

use rill::hast::{NodeKind, Tree, Value};
use rill::mdast::{self, NodeId};
use rill::{Context, Error, Options, Plugin};

/// The build's options for its arguments: `base` turns mentions on, `hashtags` tags.
#[must_use]
pub fn options(base: Option<String>, hashtags: bool) -> Options {
    let mut plugins: Vec<Box<dyn Plugin>> = vec![Box::new(Emoji)];
    if let Some(base) = base {
        plugins.push(Box::new(Mentions { base }));
    }
    if hashtags {
        plugins.push(Box::new(Hashtags));
    }
    Options {
        math: true,
        plugins,
        ..Options::default()
    }
}

#[cfg(target_arch = "wasm32")]
rill::wasm_exports!(|base: Option<String>, hashtags: bool| options(base, hashtags));

/// `remarkEmoji`.
pub struct Emoji;

const EMOJI: [(&str, &str); 3] = [(":smile:", "😄"), (":tada:", "🎉"), (":rocket:", "🚀")];

impl Plugin for Emoji {
    fn mdast(&self, tree: &mut mdast::Tree<'_>) -> Result<(), Error> {
        for node in &mut tree.nodes {
            if let mdast::NodeKind::Text(value) = &mut node.kind
                && value.contains(':')
            {
                let mut out = value.to_string();
                for (code, emoji) in EMOJI {
                    out = out.replace(code, emoji);
                }
                if out != **value {
                    *value = Cow::Owned(out);
                }
            }
        }
        Ok(())
    }
}

/// Whether text under `tag` is left as it is.
fn literal(tag: &str) -> bool {
    matches!(tag, "a" | "code" | "pre")
}

/// Replaces each text node outside links and code with what `split` makes of it (`None`:
/// unchanged).
fn edit_text<'s>(
    tree: &mut Tree<'s>,
    parent: NodeId,
    split: &mut impl FnMut(&mut Tree<'s>, &str) -> Option<Vec<NodeId>>,
) {
    let children = tree.child_list(parent);
    let mut out = Vec::with_capacity(children.len());
    let mut changed = false;
    for child in children {
        match &tree.nodes[child as usize].kind {
            NodeKind::Text(value) => {
                let value = value.to_string();
                match split(tree, &value) {
                    Some(nodes) => {
                        changed = true;
                        out.extend(nodes);
                    }
                    None => out.push(child),
                }
            }
            NodeKind::Element { tag, .. } if literal(tag) => out.push(child),
            _ => {
                edit_text(tree, child, split);
                out.push(child);
            }
        }
    }
    if changed {
        tree.replace_children(parent, &out);
    }
}

/// `rehypeMentions`: `@name` (letters, digits, `_`) links to `{base}/name`.
pub struct Mentions {
    pub base: String,
}

impl Plugin for Mentions {
    fn hast_unsanitized(&self, tree: &mut Tree<'_>, root: NodeId, _: &Context<'_>) {
        edit_text(tree, root, &mut |tree, text| {
            let bytes = text.as_bytes();
            let mut nodes = Vec::new();
            let (mut last, mut at) = (0, 0);
            while let Some(found) = text[at..].find('@') {
                let start = at + found;
                let end = start
                    + 1
                    + bytes[start + 1..]
                        .iter()
                        .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
                        .count();
                at = start + 1;
                if end == start + 1 {
                    continue;
                }
                if start > last {
                    nodes.push(tree.add(NodeKind::Text(Cow::Owned(text[last..start].into()))));
                }
                let label = tree.add(NodeKind::Text(Cow::Owned(text[start..end].into())));
                let href = format!("{}/{}", self.base, &text[start + 1..end]);
                let link = tree.add(NodeKind::Element {
                    tag: "a",
                    props: vec![("href", Value::Str(Cow::Owned(href)))],
                    data: None,
                });
                tree.replace_children(link, &[label]);
                nodes.push(link);
                (last, at) = (end, end);
            }
            if nodes.is_empty() {
                return None;
            }
            if last < text.len() {
                nodes.push(tree.add(NodeKind::Text(Cow::Owned(text[last..].into()))));
            }
            Some(nodes)
        });
    }
}

/// `rehypeHashtags`: `#tag` (a letter, then letters, digits and `-`) at the start of a
/// text or after whitespace, as `<span class="hashtag" data-tag="tag">`.
pub struct Hashtags;

/// The end of a hashtag starting at byte `start` of `text`, if one does.
fn hashtag(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let before = start.checked_sub(1).map(|index| bytes[index]);
    if bytes[start] != b'#' || before.is_some_and(|byte| !byte.is_ascii_whitespace()) {
        return None;
    }
    if !bytes.get(start + 1).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let length = bytes[start + 1..]
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || **b == b'-')
        .count();
    Some(start + 1 + length)
}

impl Plugin for Hashtags {
    /// The JavaScript form runs after KaTeX, on its output too: math that could hold a
    /// hashtag is left to JavaScript.
    fn mdast(&self, tree: &mut mdast::Tree<'_>) -> Result<(), Error> {
        let math_with_hash = tree.nodes.iter().any(|node| match &node.kind {
            mdast::NodeKind::Math(math) => math.value.contains('#'),
            mdast::NodeKind::InlineMath(value) => value.contains('#'),
            _ => false,
        });
        if math_with_hash {
            Err(Error::Unsupported)
        } else {
            Ok(())
        }
    }

    fn hast(&self, tree: &mut Tree<'_>, root: NodeId, _: &Context<'_>) {
        edit_text(tree, root, &mut |tree, text| {
            let mut nodes = Vec::new();
            let mut last = 0;
            let mut at = 0;
            while let Some(found) = text[at..].find('#') {
                let start = at + found;
                at = start + 1;
                let Some(end) = hashtag(text, start) else {
                    continue;
                };
                if start > last {
                    nodes.push(tree.add(NodeKind::Text(Cow::Owned(text[last..start].into()))));
                }
                let label = tree.add(NodeKind::Text(Cow::Owned(text[start..end].into())));
                let tag = text[start + 1..end].to_ascii_lowercase();
                let span = tree.add(NodeKind::Element {
                    tag: "span",
                    props: vec![
                        ("className", Value::Class("hashtag")),
                        ("data-tag", Value::Str(Cow::Owned(tag))),
                    ],
                    data: None,
                });
                tree.replace_children(span, &[label]);
                nodes.push(span);
                (last, at) = (end, end);
            }
            if nodes.is_empty() {
                return None;
            }
            if last < text.len() {
                nodes.push(tree.add(NodeKind::Text(Cow::Owned(text[last..].into()))));
            }
            Some(nodes)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(source: &str, base: Option<&str>, hashtags: bool) -> String {
        rill::render(source, &options(base.map(str::to_owned), hashtags))
            .expect("renders")
            .tree
    }

    #[test]
    fn plugins_follow_their_arguments() {
        let tree = html("hi @ann #news :tada:", Some("/u"), true);
        assert!(tree.contains("\"href\":\"/u/ann\""), "{tree}");
        assert!(tree.contains("\"data-tag\":\"news\""), "{tree}");
        assert!(tree.contains("🎉"), "{tree}");
        let plain = html("hi @ann #news", None, false);
        assert!(
            !plain.contains("href") && !plain.contains("data-tag"),
            "{plain}"
        );
    }

    #[test]
    fn math_that_could_hold_a_hashtag_is_left_to_javascript() {
        let options = options(None, true);
        assert!(matches!(
            rill::render("$$a #b$$", &options),
            Err(Error::Unsupported)
        ));
        assert!(rill::render("$$a+b$$ #tag", &options).is_ok());
    }
}
