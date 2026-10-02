//! Port of the pin's `test/truncated-text.test.ts`.

use cortexcode_tui_components::TruncatedText;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;

fn strip_sgr(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("\x1b[") {
            let n = after
                .bytes()
                .take_while(|b| b.is_ascii_digit() || *b == b';')
                .count();
            if after[n..].starts_with('m') {
                rest = &after[n + 1..];
                continue;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn red(s: &str) -> String {
    format!("\x1b[31m{s}\x1b[39m")
}

#[test]
fn pads_output_lines_to_exactly_match_width() {
    let lines = TruncatedText::new("Hello world", 1, 0).render(50);
    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 50);
}

#[test]
fn pads_vertical_padding_lines_to_width() {
    let lines = TruncatedText::new("Hello", 0, 2).render(40);
    assert_eq!(lines.len(), 5);
    for line in &lines {
        assert_eq!(visible_width(line), 40);
    }
}

#[test]
fn truncates_long_text_and_pads_to_width() {
    let long = "This is a very long piece of text that will definitely exceed the available width";
    let lines = TruncatedText::new(long, 1, 0).render(30);
    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 30);
    assert!(strip_sgr(&lines[0]).contains("..."));
}

#[test]
fn preserves_ansi_codes_and_pads_correctly() {
    let styled = format!("{} \x1b[34mworld\x1b[39m", red("Hello"));
    let lines = TruncatedText::new(styled, 1, 0).render(40);
    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 40);
    assert!(lines[0].contains("\x1b["));
}

#[test]
fn truncates_styled_text_and_resets_before_ellipsis() {
    let lines = TruncatedText::new(
        red("This is a very long red text that will be truncated"),
        1,
        0,
    )
    .render(20);
    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 20);
    assert!(lines[0].contains("\x1b[0m..."));
}

#[test]
fn handles_text_that_fits_exactly() {
    let lines = TruncatedText::new("Hello world", 1, 0).render(30);
    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 30);
    assert!(!strip_sgr(&lines[0]).contains("..."));
}

#[test]
fn handles_empty_text() {
    let lines = TruncatedText::new("", 1, 0).render(30);
    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 30);
}

#[test]
fn stops_at_newline_and_only_shows_first_line() {
    let lines = TruncatedText::new("First line\nSecond line\nThird line", 1, 0).render(40);
    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 40);
    let s = strip_sgr(&lines[0]);
    assert!(s.contains("First line"));
    assert!(!s.contains("Second line") && !s.contains("Third line"));
}

#[test]
fn truncates_first_line_even_with_newlines() {
    let text = "This is a very long first line that needs truncation\nSecond line";
    let lines = TruncatedText::new(text, 1, 0).render(25);
    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 25);
    let s = strip_sgr(&lines[0]);
    assert!(s.contains("...") && !s.contains("Second line"));
}
