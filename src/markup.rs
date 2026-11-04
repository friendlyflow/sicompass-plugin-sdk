//! The HTML-named tags a row can hold: `<div>`, `<p>`, `<h1>`, `<b>`, `<span>` and so
//! on, each with a `class`.
//!
//! These are for looks, and live beside the older tags in [`crate::tags`] (`<input>`,
//! `<link>`, `<image>`), which are for what a row does. [`plain`] takes them out again,
//! leaving a string the rest of the app reads as it always did.
//!
//! **Only read this for text known to be markup.** Plenty of rows hold text nobody
//! wrote as markup: a file's name, a line of a text file, what a terminal printed. A
//! `<b>` there is two letters and a bracket. That is why [`crate::tags::strip_display`]
//! knows nothing of these tags, and why a provider has to opt in
//! ([`crate::provider::Provider::markup`]) and then escape what it did not write
//! ([`crate::tags::escape`]).
//!
//! Only the tags in [`TAGS`] are recognised. Anything else stays text.

use crate::tags::{self, Target, TargetCounts, TargetKind};

/// The tags this module recognises.
pub const TAGS: &[&str] = &["div", "p", "h1", "h2", "h3", "header", "span", "b", "video"];

/// A tag and what is inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub tag: &'static str,
    pub classes: Vec<String>,
    /// A `<video>`'s still image, shown until it plays.
    pub poster: Option<String>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// Text, still holding any of the older tags.
    Text(String),
    Element(Element),
}

/// An opening or closing tag at the start of `text`, and how long it is.
enum TagToken<'a> {
    Open {
        tag: &'static str,
        attributes: Attributes,
        len: usize,
    },
    Close {
        tag: &'a str,
        len: usize,
    },
}

/// The attributes this module reads.
#[derive(Default)]
struct Attributes {
    classes: Vec<String>,
    poster: Option<String>,
}

/// A tag's attributes, or `None` if they do not read as attributes. Others are allowed
/// and mean nothing here.
fn parse_attributes(mut attrs: &str) -> Option<Attributes> {
    let mut found = Attributes::default();
    loop {
        attrs = attrs.trim_start();
        if attrs.is_empty() {
            return Some(found);
        }
        let name_end = attrs
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(attrs.len());
        let (name, rest) = attrs.split_at(name_end);
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return None;
        }
        let Some(rest) = rest.strip_prefix('=') else {
            attrs = rest;
            continue;
        };
        let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let rest = &rest[1..];
        let end = rest.find(quote)?;
        if name.eq_ignore_ascii_case("class") {
            found
                .classes
                .extend(rest[..end].split_whitespace().map(str::to_owned));
        } else if name.eq_ignore_ascii_case("poster") {
            found.poster = Some(rest[..end].to_owned());
        }
        attrs = &rest[end + 1..];
    }
}

fn read_tag(text: &str) -> Option<TagToken<'_>> {
    let inner_end = text.find('>')?;
    let inner = text.get(1..inner_end)?;
    let len = inner_end + 1;
    if let Some(name) = inner.strip_prefix('/') {
        let name = name.trim_end();
        let tag = *TAGS.iter().find(|t| t.eq_ignore_ascii_case(name))?;
        return Some(TagToken::Close { tag, len });
    }
    let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
    let tag = *TAGS
        .iter()
        .find(|t| t.eq_ignore_ascii_case(&inner[..name_end]))?;
    let attributes = parse_attributes(&inner[name_end..])?;
    Some(TagToken::Open {
        tag,
        attributes,
        len,
    })
}

/// Read a row's tags into a tree. A tag left open closes at the end, a closing tag
/// closes the nearest one that matches and anything open inside it, and a closing tag
/// that matches nothing stays text.
pub fn parse(text: &str) -> Vec<Node> {
    let mut stack: Vec<Element> = Vec::new();
    let mut top: Vec<Node> = Vec::new();
    let mut pending = String::new();

    fn children<'a>(stack: &'a mut [Element], top: &'a mut Vec<Node>) -> &'a mut Vec<Node> {
        match stack.last_mut() {
            Some(open) => &mut open.children,
            None => top,
        }
    }
    fn flush(pending: &mut String, into: &mut Vec<Node>) {
        if !pending.is_empty() {
            into.push(Node::Text(std::mem::take(pending)));
        }
    }

    let mut rest = text;
    while let Some(at) = rest.find('<') {
        let escaped = rest[..at].ends_with('\\');
        let token = if escaped { None } else { read_tag(&rest[at..]) };
        match token {
            Some(TagToken::Open {
                tag,
                attributes,
                len,
            }) => {
                pending.push_str(&rest[..at]);
                flush(&mut pending, children(&mut stack, &mut top));
                stack.push(Element {
                    tag,
                    classes: attributes.classes,
                    poster: attributes.poster,
                    children: Vec::new(),
                });
                rest = &rest[at + len..];
            }
            Some(TagToken::Close { tag, len }) if stack.iter().any(|e| e.tag == tag) => {
                pending.push_str(&rest[..at]);
                flush(&mut pending, children(&mut stack, &mut top));
                while let Some(open) = stack.pop() {
                    let matched = open.tag == tag;
                    children(&mut stack, &mut top).push(Node::Element(open));
                    if matched {
                        break;
                    }
                }
                rest = &rest[at + len..];
            }
            _ => {
                pending.push_str(&rest[..=at]);
                rest = &rest[at + 1..];
            }
        }
    }
    pending.push_str(rest);
    flush(&mut pending, children(&mut stack, &mut top));
    while let Some(open) = stack.pop() {
        children(&mut stack, &mut top).push(Node::Element(open));
    }
    top
}

fn push_plain(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Element(element) => push_plain(&element.children, out),
        }
    }
}

/// `text` without the tags of this module, which is what the rest of the app reads.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    push_plain(&parse(text), &mut out);
    out
}

fn push_targets(nodes: &[Node], shown: &mut usize, out: &mut Vec<Target>) {
    for node in nodes {
        match node {
            Node::Text(text) => {
                out.extend(tags::targets(text).into_iter().map(|mut target| {
                    target.display_span =
                        target.display_span.start + *shown..target.display_span.end + *shown;
                    target
                }));
                *shown += tags::strip_display(text).chars().count();
            }
            Node::Element(element) if element.tag == "video" => {
                let mut path = String::new();
                push_plain(&element.children, &mut path);
                let path = path.trim().to_owned();
                let len = tags::strip_display(&path).chars().count();
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                out.push(Target {
                    kind: TargetKind::Video,
                    value: path,
                    name,
                    display_span: *shown..*shown + len,
                });
                *shown += len;
            }
            Node::Element(element) => push_targets(&element.children, shown, out),
        }
    }
}

/// Every image, link, button and video in a row of markup, in reading order, with
/// spans in what [`plain`] shows. [`tags::targets`] does this for a row with no markup.
pub fn targets(text: &str) -> Vec<Target> {
    let mut out = Vec::new();
    push_targets(&parse(text), &mut 0, &mut out);
    out
}

/// How many of each a row of markup holds, cheap enough to ask every frame.
pub fn target_counts(text: &str) -> TargetCounts {
    let mut counts = tags::target_counts(text);
    let mut rest = text;
    while let Some(at) = rest.find("<video") {
        let escaped = rest[..at].ends_with('\\');
        let tag_end = &rest[at + "<video".len()..];
        if !escaped && tag_end.starts_with(['>', ' ', '\t', '\n']) {
            counts.videos += 1;
        }
        rest = tag_end;
    }
    counts
}

/// Whether `text` holds any tag of this module.
pub fn has_markup(text: &str) -> bool {
    parse(text).iter().any(|n| matches!(n, Node::Element(_)))
}

/// The stylesheet in a `<style>` row, `None` if the row is anything else.
pub fn style_sheet(row: &str) -> Option<&str> {
    row.trim().strip_prefix("<style>")?.strip_suffix("</style>")
}

/// The one element a whole row is, if it is one: `<h1 class="x">Title</h1>`. Space
/// around it does not count.
pub fn whole_row_element(nodes: &[Node]) -> Option<&Element> {
    let mut found = None;
    for node in nodes {
        match node {
            Node::Text(text) if text.trim().is_empty() => {}
            Node::Element(element) if found.is_none() => found = Some(element),
            _ => return None,
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(tag: &'static str, classes: &[&str], children: Vec<Node>) -> Node {
        Node::Element(Element {
            tag,
            classes: classes.iter().map(|c| c.to_string()).collect(),
            poster: None,
            children,
        })
    }

    fn text(s: &str) -> Node {
        Node::Text(s.to_owned())
    }

    #[test]
    fn plain_text_is_one_text_node() {
        assert_eq!(parse("just words"), [text("just words")]);
        assert_eq!(parse(""), []);
    }

    #[test]
    fn tags_with_classes_become_elements() {
        assert_eq!(
            parse(r#"<p class="a b">Hi</p>"#),
            [element("p", &["a", "b"], vec![text("Hi")])]
        );
    }

    #[test]
    fn tags_nest() {
        assert_eq!(
            parse("<div>one <b>two</b> three</div>"),
            [element(
                "div",
                &[],
                vec![
                    text("one "),
                    element("b", &[], vec![text("two")]),
                    text(" three")
                ]
            )]
        );
    }

    /// The user's grid example.
    #[test]
    fn the_grid_example_reads() {
        let row = r#"<div class="grid-container">
  <div class="grid-item">1</div>
  <div class="grid-item">2</div>
</div>"#;
        let nodes = parse(row);
        let Some(Node::Element(grid)) = nodes.first() else {
            panic!("{nodes:?}")
        };
        assert_eq!(grid.classes, ["grid-container"]);
        let items: Vec<_> = grid
            .children
            .iter()
            .filter_map(|n| match n {
                Node::Element(e) => Some(e),
                Node::Text(_) => None,
            })
            .collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].classes, ["grid-item"]);
        assert_eq!(items[1].children, [text("2")]);
    }

    #[test]
    fn the_older_tags_stay_in_the_text() {
        let row = "<b>Go <link>u</link></b> <input>v</input>";
        assert_eq!(plain(row), "Go <link>u</link> <input>v</input>");
        assert_eq!(
            crate::tags::targets(&plain("<div>A <link>u</link></div>"))[0].name,
            "A"
        );
    }

    /// Only the tags this module names: everything else is what the row says.
    #[test]
    fn an_unknown_tag_stays_text() {
        assert_eq!(plain("a <blink>b</blink> c"), "a <blink>b</blink> c");
        assert_eq!(plain("1 < 2 and 3 > 2"), "1 < 2 and 3 > 2");
        assert_eq!(plain("a <"), "a <");
        assert!(!has_markup("<blink>b</blink>"));
    }

    #[test]
    fn an_escaped_tag_stays_text() {
        assert_eq!(plain(r"\<b\>x\</b\>"), r"\<b\>x\</b\>");
        assert!(!has_markup(r"\<b\>x\</b\>"));
        assert_eq!(plain(&crate::tags::escape("<b>x</b>")), r"\<b\>x\</b\>");
    }

    #[test]
    fn a_tag_left_open_closes_at_the_end() {
        assert_eq!(parse("<b>x"), [element("b", &[], vec![text("x")])]);
    }

    #[test]
    fn a_closing_tag_closes_what_is_open_inside_it() {
        assert_eq!(
            parse("<div><b>x</div>y"),
            [
                element("div", &[], vec![element("b", &[], vec![text("x")])]),
                text("y")
            ]
        );
    }

    #[test]
    fn a_closing_tag_that_matches_nothing_stays_text() {
        assert_eq!(plain("x</b>y"), "x</b>y");
    }

    #[test]
    fn other_attributes_mean_nothing_and_broken_ones_are_not_a_tag() {
        assert_eq!(
            parse(r#"<p id="x" hidden class='a'>y</p>"#),
            [element("p", &["a"], vec![text("y")])]
        );
        assert_eq!(plain(r#"<p class="a>y"#), r#"<p class="a>y"#);
        assert_eq!(plain("<p class=a>y"), "<p class=a>y");
    }

    #[test]
    fn tag_names_ignore_case() {
        assert_eq!(parse("<B>x</b>"), [element("b", &[], vec![text("x")])]);
    }

    #[test]
    fn multibyte_text_survives() {
        assert_eq!(plain("<p>café → thé</p> é"), "café → thé é");
        assert_eq!(plain("é < è"), "é < è");
    }

    #[test]
    fn a_style_row_is_only_a_whole_row() {
        assert_eq!(
            style_sheet("<style>li { color: red }</style>"),
            Some("li { color: red }")
        );
        assert_eq!(style_sheet("  <style>x</style>\n"), Some("x"));
        assert_eq!(style_sheet("see <style>x</style>"), None);
        assert_eq!(style_sheet("<style>x"), None);
        assert_eq!(style_sheet(r"\<style\>x\</style\>"), None);
    }

    #[test]
    fn a_video_is_a_target_with_its_poster() {
        let row = r#"<video poster="still.jpg">/clips/trip.mp4</video>"#;
        let nodes = parse(row);
        let Some(Node::Element(video)) = nodes.first() else {
            panic!("{nodes:?}")
        };
        assert_eq!(video.tag, "video");
        assert_eq!(video.poster.as_deref(), Some("still.jpg"));
        let found = targets(row);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, TargetKind::Video);
        assert_eq!(found[0].value, "/clips/trip.mp4");
        assert_eq!(found[0].name, "trip.mp4");
    }

    #[test]
    fn targets_of_all_kinds_come_in_reading_order_through_the_tags() {
        let row = r#"<div class="g"><div><image>a.jpg</image></div><div><video>b.mp4</video></div> See <b>Home</b> <link>u</link></div>"#;
        let found = targets(row);
        let kinds: Vec<TargetKind> = found.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [TargetKind::Image, TargetKind::Video, TargetKind::Link]
        );
        assert_eq!(found[1].value, "b.mp4");
    }

    /// Spans index what the row shows, which is [`plain`] through `strip_display`.
    #[test]
    fn target_spans_agree_with_what_the_row_shows() {
        for row in [
            "<p>One <link>u1</link></p> <b>Two</b> <link>u2</link>",
            "<div><div><image>a.jpg</image></div><div><image>b.jpg</image></div></div>",
            "<span class=\"x\"><button>f</button>Go</span> <b><button>g</button>Stop</b>",
            "<video>a.mp4</video> and <image>b.jpg</image>",
        ] {
            let shown: Vec<char> = tags::strip_display(&plain(row)).chars().collect();
            for target in targets(row) {
                let text: String = shown[target.display_span.clone()].iter().collect();
                assert!(
                    text.contains(&target.value) || text.contains(&target.name),
                    "{row}: {target:?} shows {text:?}"
                );
            }
        }
    }

    #[test]
    fn counts_agree_with_the_targets() {
        for row in [
            "plain",
            r#"<video poster="p">a</video> <video>b</video> <image>c</image>"#,
            r"\<video>not one",
            "<videos>not one</videos>",
            "<button>f</button>Go <link>u</link>",
        ] {
            let found = targets(row);
            let by = |k| found.iter().filter(|t| t.kind == k).count();
            assert_eq!(
                target_counts(row),
                TargetCounts {
                    links: by(TargetKind::Link),
                    images: by(TargetKind::Image),
                    buttons: by(TargetKind::Button),
                    videos: by(TargetKind::Video),
                },
                "{row}"
            );
        }
    }

    #[test]
    fn a_whole_row_element_is_found_with_space_around_it() {
        let nodes = parse("  <h1>T</h1>\n");
        assert_eq!(whole_row_element(&nodes).map(|e| e.tag), Some("h1"));
        assert!(whole_row_element(&parse("a <b>x</b>")).is_none());
        assert!(whole_row_element(&parse("<b>x</b><b>y</b>")).is_none());
        assert!(whole_row_element(&parse("plain")).is_none());
    }
}
