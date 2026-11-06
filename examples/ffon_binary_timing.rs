//! Times the binary FFON codec, the one every plugin reply crosses the pipe in.
//!
//! `cargo run --release --example ffon_binary_timing`

use sicompass_sdk::ffon::{FfonElement, FfonObject, deserialize_binary, serialize_binary};
use std::hint::black_box;
use std::time::{Duration, Instant};

/// A folder listing: one list, many short leaves.
fn flat_folder(entries: usize) -> Vec<FfonElement> {
    let mut folder = FfonObject::new("Documents");
    for i in 0..entries {
        folder.push(FfonElement::new_str(format!("- report-{i:05}.pdf")));
    }
    vec![FfonElement::Obj(folder)]
}

/// A web page: sections nested `depth` deep, `fanout` children each.
fn nested_page(depth: usize, fanout: usize) -> Vec<FfonElement> {
    fn build(depth: usize, fanout: usize, label: &str) -> FfonElement {
        if depth == 0 {
            return FfonElement::new_str(format!("<p>paragraph {label}</p>"));
        }
        let mut obj = FfonObject::new(format!("section {label}"));
        for i in 0..fanout {
            obj.push(build(depth - 1, fanout, &format!("{label}.{i}")));
        }
        FfonElement::Obj(obj)
    }
    (0..fanout)
        .map(|i| build(depth - 1, fanout, &i.to_string()))
        .collect()
}

/// A mailbox or a kanban board: many small sections of a few rows.
fn small_sections(sections: usize) -> Vec<FfonElement> {
    (0..sections)
        .map(|i| {
            let mut obj = FfonObject::new(format!("Message {i}"));
            obj.push(FfonElement::new_str(format!("From: sender{i}@example.org")));
            obj.push(FfonElement::new_str("<input>Subject line</input>"));
            obj.push(FfonElement::new_str("Body text of a short message."));
            FfonElement::Obj(obj)
        })
        .collect()
}

fn count(elems: &[FfonElement]) -> usize {
    elems
        .iter()
        .map(|e| match e {
            FfonElement::Str(_) => 1,
            FfonElement::Obj(o) => 1 + count(&o.children),
        })
        .sum()
}

/// The fastest of `runs` timings, which is the least disturbed one.
fn fastest(runs: usize, mut f: impl FnMut()) -> Duration {
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .min()
        .unwrap()
}

fn main() {
    let cases = [
        ("flat folder", flat_folder(50_000)),
        ("nested page", nested_page(6, 7)),
        ("small sections", small_sections(25_000)),
    ];
    println!(
        "{:<16} {:>8} {:>10} {:>12} {:>12}",
        "tree", "nodes", "bytes", "serialize", "deserialize"
    );
    for (name, tree) in &cases {
        let data = serialize_binary(tree);
        assert_eq!(&deserialize_binary(&data), tree);
        let ser = fastest(20, || {
            black_box(serialize_binary(black_box(tree)));
        });
        let de = fastest(20, || {
            black_box(deserialize_binary(black_box(&data)));
        });
        println!(
            "{:<16} {:>8} {:>10} {:>12.2?} {:>12.2?}",
            name,
            count(tree),
            data.len(),
            ser,
            de
        );
    }
}
