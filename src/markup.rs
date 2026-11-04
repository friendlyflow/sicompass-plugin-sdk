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
pub const TAGS: &[&str] = &[
    "div", "p", "h1", "h2", "h3", "header", "span", "b", "img", "a", "video",
];

/// A tag and what is inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub tag: &'static str,
    pub classes: Vec<String>,
    /// A `<video>`'s still image, shown until it plays.
    pub poster: Option<String>,
    /// Where an `<img>` or `<video>` is.
    pub src: Option<String>,
    /// What an `<img>` is of, which is what a screen reader says.
    pub alt: Option<String>,
    /// Where an `<a>` goes.
    pub href: Option<String>,
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
        /// `<img ...>` and `<x ... />` have no closing tag.
        void: bool,
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
    src: Option<String>,
    alt: Option<String>,
    href: Option<String>,
}

/// Write `value` so it can sit inside a quoted attribute: `"` and the brackets that
/// would end the tag become entities, which reading undoes. For file names and the like.
pub fn escape_attribute(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// The reverse of [`escape_attribute`]. Other `&...;` stay as written.
fn unescape_attribute(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = [
            ("&amp;", '&'),
            ("&quot;", '"'),
            ("&apos;", '\''),
            ("&lt;", '<'),
            ("&gt;", '>'),
        ]
        .into_iter()
        .find(|(entity, _)| rest.starts_with(entity));
        match decoded {
            Some((entity, c)) => {
                out.push(c);
                rest = &rest[entity.len()..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
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
            found.poster = Some(unescape_attribute(&rest[..end]));
        } else if name.eq_ignore_ascii_case("src") {
            found.src = Some(unescape_attribute(&rest[..end]));
        } else if name.eq_ignore_ascii_case("alt") {
            found.alt = Some(unescape_attribute(&rest[..end]));
        } else if name.eq_ignore_ascii_case("href") {
            found.href = Some(unescape_attribute(&rest[..end]));
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
    let (inner, self_closing) = match inner.trim_end().strip_suffix('/') {
        Some(without) => (without, true),
        None => (inner, false),
    };
    let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
    let tag = *TAGS
        .iter()
        .find(|t| t.eq_ignore_ascii_case(&inner[..name_end]))?;
    let attributes = parse_attributes(&inner[name_end..])?;
    Some(TagToken::Open {
        tag,
        attributes,
        void: self_closing || tag == "img",
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
        // Only an opening tag can be escaped by the backslash before it: a literal
        // closing tag is `\</b\>`, which is no tag at all, and `\</b>` is text that ends
        // in a backslash.
        let escaped = rest[..at].ends_with('\\');
        let token =
            read_tag(&rest[at..]).filter(|t| !escaped || matches!(t, TagToken::Close { .. }));
        match token {
            Some(TagToken::Open {
                tag,
                attributes,
                void,
                len,
            }) => {
                pending.push_str(&rest[..at]);
                flush(&mut pending, children(&mut stack, &mut top));
                let element = Element {
                    tag,
                    classes: attributes.classes,
                    poster: attributes.poster,
                    src: attributes.src,
                    alt: attributes.alt,
                    href: attributes.href,
                    children: Vec::new(),
                };
                if void {
                    children(&mut stack, &mut top).push(Node::Element(element));
                } else {
                    stack.push(element);
                }
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

/// What one image or link in the lowered row is called, in the order they appear. The
/// older tags have no name of their own, only the text before them, so they hold `None`.
#[derive(Default)]
struct Names {
    /// For each `<image>`: whether it was a `<video>`, and its name if it has one.
    images: Vec<(bool, Option<String>)>,
    links: Vec<Option<String>>,
}

/// The row written in the older tags, which the rest of the app reads. An `<img>` is
/// its `alt` and an `<image>`, an `<a>` is its text and a `<link>`, and a `<video>` is
/// its path as text, or with `video_as_image` as an `<image>` so a flat read can find
/// it.
fn lower(nodes: &[Node], out: &mut String, names: &mut Names, video_as_image: bool) {
    for node in nodes {
        match node {
            Node::Text(text) => {
                out.push_str(text);
                let counts = tags::target_counts(text);
                names
                    .images
                    .extend((0..counts.images).map(|_| (false, None)));
                names.links.extend((0..counts.links).map(|_| None));
            }
            Node::Element(element) => match element.tag {
                "img" => {
                    let src = element.src.as_deref().unwrap_or_default();
                    let alt = element
                        .alt
                        .as_deref()
                        .map(str::trim)
                        .filter(|a| !a.is_empty());
                    if let Some(alt) = alt {
                        out.push_str(alt);
                        out.push(' ');
                    }
                    out.push_str(&format!("<image>{src}</image>"));
                    names.images.push((false, alt.map(str::to_owned)));
                }
                "a" => {
                    let mut inner = String::new();
                    lower(&element.children, &mut inner, names, video_as_image);
                    let name = tags::strip_display(&inner).trim().to_owned();
                    out.push_str(&inner);
                    if let Some(href) = &element.href {
                        if !name.is_empty() {
                            out.push(' ');
                        }
                        out.push_str(&format!("<link>{href}</link>"));
                        names
                            .links
                            .push(Some(if name.is_empty() { href.clone() } else { name }));
                    }
                }
                "video" => {
                    // `<video src="a.mp4">Trip</video>`, or the older `<video>a.mp4</video>`.
                    let mut inner = String::new();
                    lower(&element.children, &mut inner, names, video_as_image);
                    let inner = tags::strip_display(&inner).trim().to_owned();
                    let (src, caption) = match &element.src {
                        Some(src) => (src.clone(), inner),
                        None => (inner, String::new()),
                    };
                    if !caption.is_empty() {
                        out.push_str(&caption);
                        out.push(' ');
                    }
                    if video_as_image {
                        out.push_str(&format!("<image>{src}</image>"));
                    } else {
                        out.push_str(&src);
                    }
                    let name = if caption.is_empty() {
                        std::path::Path::new(&src)
                            .file_name()
                            .map_or_else(|| src.clone(), |n| n.to_string_lossy().into_owned())
                    } else {
                        caption
                    };
                    if video_as_image {
                        names.images.push((true, Some(name)));
                    }
                }
                _ => lower(&element.children, out, names, video_as_image),
            },
        }
    }
}

/// `text` without the tags of this module, which is what the rest of the app reads.
pub fn plain(text: &str) -> String {
    plain_nodes(&parse(text))
}

/// [`plain`] for a part of a parsed row.
pub fn plain_nodes(nodes: &[Node]) -> String {
    let mut out = String::new();
    lower(nodes, &mut out, &mut Names::default(), false);
    out
}

/// Every image, link, button and video in a row of markup, in reading order, with
/// spans in what [`plain`] shows. [`tags::targets`] does this for a row with no markup.
///
/// Through one flat string, because a link's name is the text before it, and that text
/// can run through tags: `Read <b>this</b> <link>u</link>` is one link called
/// `Read this`. An `<a>` is called by its own text, an `<img>` by its `alt`.
pub fn targets(text: &str) -> Vec<Target> {
    targets_of(&parse(text))
}

/// [`targets`] for a part of a parsed row.
pub fn targets_of(nodes: &[Node]) -> Vec<Target> {
    let mut flat = String::new();
    let mut names = Names::default();
    lower(nodes, &mut flat, &mut names, true);
    let mut images = names.images.into_iter();
    let mut links = names.links.into_iter();
    let mut found = tags::targets(&flat);
    for target in &mut found {
        let given = match target.kind {
            TargetKind::Image => images.next().and_then(|(is_video, name)| {
                if is_video {
                    target.kind = TargetKind::Video;
                }
                name
            }),
            TargetKind::Link => links.next().flatten(),
            _ => None,
        };
        if let Some(name) = given {
            target.name = name;
        }
    }
    found
}

/// How many of each a row of markup holds. A parse and a walk, which is fine for the one
/// row the cursor is on.
pub fn target_counts(text: &str) -> TargetCounts {
    let mut counts = TargetCounts::default();
    for target in targets(text) {
        match target.kind {
            TargetKind::Link => counts.links += 1,
            TargetKind::Image => counts.images += 1,
            TargetKind::Button => counts.buttons += 1,
            TargetKind::Video => counts.videos += 1,
        }
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
            src: None,
            alt: None,
            href: None,
            children,
        })
    }

    #[test]
    fn img_is_an_image_with_its_alt_as_the_name() {
        let row = r#"<img src="/p/cat.jpg" alt="A cat on a sofa">"#;
        assert_eq!(plain(row), "A cat on a sofa <image>/p/cat.jpg</image>");
        let found = targets(row);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, TargetKind::Image);
        assert_eq!(found[0].value, "/p/cat.jpg");
        assert_eq!(found[0].name, "A cat on a sofa");
    }

    #[test]
    fn img_needs_no_closing_tag_and_may_close_itself() {
        for row in [
            r#"<img src="a.jpg">"#,
            r#"<img src="a.jpg"/>"#,
            r#"<img src="a.jpg" />"#,
            r#"<img src="a.jpg"></img>"#,
        ] {
            assert_eq!(targets(row).len(), 1, "{row}");
        }
        assert_eq!(targets(r#"<img src="a.jpg"><img src="b.jpg">"#).len(), 2);
    }

    #[test]
    fn an_img_with_no_alt_is_called_by_its_file_name() {
        let found = targets(r#"<img src="/p/cat.jpg">"#);
        assert_eq!(found[0].name, "cat.jpg");
        assert_eq!(
            plain(r#"<img src="/p/cat.jpg">"#),
            "<image>/p/cat.jpg</image>"
        );
    }

    #[test]
    fn a_is_a_link_called_by_its_own_text() {
        let row = r#"See <a href="https://x.org/home">Home</a> and <a href="https://x.org/about">About us</a>"#;
        assert_eq!(
            plain(row),
            "See Home <link>https://x.org/home</link> and About us <link>https://x.org/about</link>"
        );
        let found = targets(row);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, TargetKind::Link);
        assert_eq!(found[0].value, "https://x.org/home");
        assert_eq!(found[0].name, "Home");
        assert_eq!(found[1].name, "About us");
    }

    #[test]
    fn an_a_with_no_text_is_called_by_its_address() {
        assert_eq!(
            targets(r#"<a href="https://x.org"></a>"#)[0].name,
            "https://x.org"
        );
    }

    #[test]
    fn an_a_with_no_href_is_only_its_text() {
        assert_eq!(plain("<a>just text</a>"), "just text");
        assert!(targets("<a>just text</a>").is_empty());
    }

    /// A picture that is also a link, the usual thumbnail.
    #[test]
    fn an_img_inside_an_a_is_both() {
        let row = r#"<a href="page.html"><img src="t.jpg" alt="Trip"></a>"#;
        let found = targets(row);
        let kinds: Vec<TargetKind> = found.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, [TargetKind::Image, TargetKind::Link]);
        assert_eq!(found[0].name, "Trip");
        assert_eq!(found[1].value, "page.html");
    }

    #[test]
    fn video_takes_its_path_from_src_and_its_name_from_its_text() {
        let row = r#"<video src="/clips/trip.mp4" poster="still.jpg">The trip</video>"#;
        let found = targets(row);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, TargetKind::Video);
        assert_eq!(found[0].value, "/clips/trip.mp4");
        assert_eq!(found[0].name, "The trip");
        assert_eq!(plain(row), "The trip /clips/trip.mp4");
        let nodes = parse(row);
        let Some(Node::Element(video)) = nodes.first() else {
            panic!("{nodes:?}")
        };
        assert_eq!(video.poster.as_deref(), Some("still.jpg"));
        assert_eq!(video.src.as_deref(), Some("/clips/trip.mp4"));
    }

    #[test]
    fn a_video_with_a_src_and_no_text_is_called_by_its_file_name() {
        let found = targets(r#"<video src="/clips/trip.mp4"></video>"#);
        assert_eq!(found[0].name, "trip.mp4");
        assert_eq!(
            plain(r#"<video src="/clips/trip.mp4"/>"#),
            "/clips/trip.mp4"
        );
    }

    /// A file name with a quote or a bracket in it must stay in its attribute.
    #[test]
    fn attribute_values_survive_what_would_end_the_tag() {
        let name = r#"My "best" <trip> & more.jpg"#;
        let row = format!(
            r#"<img src="{}" alt="{}">"#,
            escape_attribute(name),
            escape_attribute(name)
        );
        let found = targets(&row);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].value, name);
        assert_eq!(found[0].name, name);
    }

    #[test]
    fn escape_attribute_round_trips_and_leaves_other_entities() {
        for value in ["plain", "a&b", "&amp;", "<>\"'", "&unknown; & more", ""] {
            assert_eq!(
                unescape_attribute(&escape_attribute(value)),
                value,
                "{value}"
            );
        }
        assert_eq!(unescape_attribute("&copy; &#39;"), "&copy; &#39;");
    }

    #[test]
    fn the_older_tags_and_the_real_ones_can_share_a_row() {
        let row = r#"<img src="a.jpg" alt="One"> <image>b.jpg</image> <a href="u">Go</a> <button>f</button>Run"#;
        let kinds: Vec<TargetKind> = targets(row).iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [
                TargetKind::Image,
                TargetKind::Image,
                TargetKind::Link,
                TargetKind::Button
            ]
        );
        assert_eq!(target_counts(row).images, 2);
    }

    #[test]
    fn spans_stay_right_with_the_real_tags() {
        for row in [
            r#"<p>Look <img src="a.jpg" alt="A"> then <a href="u">go</a></p>"#,
            r#"<div><img src="a.jpg"><img src="b.jpg"></div>"#,
            r#"<video src="v.mp4">Clip</video> and <a href="u">link</a>"#,
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
    fn self_closing_works_on_any_tag() {
        assert_eq!(plain("a<b/>c"), "ac");
        assert_eq!(parse("<span class=\"x\"/>").len(), 1);
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
    fn text_ending_in_a_backslash_still_closes_its_tag() {
        let row = format!("<p>{}</p><b>y</b>", tags::escape(r"x\"));
        let nodes = parse(&row);
        assert_eq!(nodes.len(), 2, "{nodes:?}");
        assert_eq!(tags::strip_display(&plain(&row)), r"x\y");
        let literal = tags::escape("</p>");
        assert_eq!(parse(&format!("<p>{literal}</p>")).len(), 1);
        assert_eq!(
            tags::strip_display(&plain(&format!("<p>{literal}</p>"))),
            "</p>"
        );
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

    /// A link is called by the text before it, even through a tag.
    #[test]
    fn a_links_name_runs_through_tags() {
        let found = targets("<div>One <link>u1</link> <b>Two</b> <link>u2</link></div>");
        assert_eq!(found[0].name, "One");
        assert_eq!(found[1].name, "Two");
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
