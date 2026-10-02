//! Port of the pin's `test/streaming-segmentation.test.ts`.

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tui_theme::get_markdown_theme;
use cortexcode_code_tui_widgets::segment_streaming_markdown;
use cortexcode_tui_components::{Markdown, Spacer};
use cortexcode_tui_render::{Component, Container};
use cortexcode_tui_util::strip_vt_control_characters;
use support::lock;

fn normalize(lines: &[String]) -> String {
    let joined = lines
        .iter()
        .map(|l| strip_vt_control_characters(l).trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    joined.trim_matches('\n').to_string()
}

/// Render text as one Markdown and as segments joined by `Spacer(1)`.
fn render_both_ways(text: &str) -> (String, String) {
    let width = 100;
    let single = normalize(&Markdown::new(text, 1, 0, get_markdown_theme(), None).render(width));
    let chunks = segment_streaming_markdown(text);
    let mut container = Container::new();
    for (k, chunk) in chunks.iter().enumerate() {
        if k > 0 {
            container.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        }
        container.add_child(Rc::new(RefCell::new(Markdown::new(
            chunk.as_str(),
            1,
            0,
            get_markdown_theme(),
            None,
        ))));
    }
    (single, normalize(&container.render(width)))
}

const CASES: &[(&str, &str)] = &[
    (
        "paragraphs",
        "First paragraph with some text.\n\nSecond paragraph here.\n\nThird one closes it out.",
    ),
    (
        "heading + code + text",
        "## The plan\n\nSome intro prose about the change.\n\n```ts\nconst x = 1;\n\nconst y = 2; // blank line above must stay inside the fence\n```\n\nClosing remarks after the code block.",
    ),
    (
        "loose list stays whole",
        "Intro line.\n\n- first item\n\n- second item of the same loose list\n\n- third item\n\nOutro line.",
    ),
    (
        "ordered list then paragraph",
        "1. one\n2. two\n3. three\n\nA paragraph after the list.",
    ),
    (
        "blockquote and hr",
        "> quoted wisdom\n> more of it\n\n---\n\nAfter the rule.",
    ),
    (
        "table stays attached",
        "Before table.\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nAfter table.",
    ),
    (
        "indented continuation binds up",
        "- item with body\n\n  continued body of the item\n\nPlain after.",
    ),
];

#[test]
fn segmented_render_matches_single_render() {
    let _g = lock();
    for (name, text) in CASES {
        let (single, segmented) = render_both_ways(text);
        assert_eq!(segmented, single, "{name}");
    }
}

#[test]
fn never_splits_inside_a_code_fence() {
    let text = "```\na\n\nb\n```";
    assert_eq!(segment_streaming_markdown(text), vec![text]);
}

#[test]
fn does_not_segment_when_link_reference_definitions_are_present() {
    let text = "See [the docs][ref].\n\nMore text.\n\n[ref]: https://example.com";
    assert_eq!(segment_streaming_markdown(text), vec![text]);
}

#[test]
fn prefix_stability_appending_text_never_changes_earlier_chunks() {
    let parts = [
        "First paragraph of the answer.",
        "Some more prose in a second block.",
        "```ts\nconst z = 42;\n```",
        "- alpha\n- beta",
        "A closing paragraph.",
    ];
    let mut text = String::new();
    let mut previous: Vec<String> = Vec::new();
    for part in parts {
        text = if text.is_empty() {
            part.to_string()
        } else {
            format!("{text}\n\n{part}")
        };
        let chunks = segment_streaming_markdown(&text);
        for i in 0..previous.len().saturating_sub(1) {
            assert_eq!(chunks[i], previous[i]);
        }
        previous = chunks;
    }
    assert!(previous.len() > 1);
}

#[test]
fn multibyte_line_starts_do_not_panic() {
    // Regression: a thinking block rendered as `✻ ...` crashed the list-item
    // check by slicing inside a multi-byte char.
    let text = "✻ first paragraph\n\n✻ second\n\n- item\n\n日本語\n\n  é\n\n[é]: x";
    let _ = segment_streaming_markdown(text);
    let chunks = segment_streaming_markdown("✻ a\n\n✻ b\n\n— c");
    assert_eq!(chunks.len(), 3);
}
