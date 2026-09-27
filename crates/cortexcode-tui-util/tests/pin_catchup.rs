//! Ports of the pin's `test/background-fill.test.ts` and the `hyperlinkAt` /
//! `bareUrlAt` half of `test/hyperlink-click.test.ts` (the click dispatch half
//! lives with the renderer).

use cortexcode_tui_util::{apply_background_to_line, bare_url_at, hyperlink_at, visible_width};

fn band(text: &str) -> String {
    format!("\x1b[48;5;224m{text}\x1b[49m")
}

/// The cells a terminal would paint in a background.
fn painted_cells(rendered: &str, width: usize) -> Vec<bool> {
    let mut cells = Vec::new();
    let mut painted = false;
    let mut rest = rendered;
    while !rest.is_empty() && cells.len() < width {
        if let Some(after) = rest.strip_prefix("\x1b[") {
            let n = after
                .bytes()
                .take_while(|b| b.is_ascii_digit() || *b == b';')
                .count();
            if after[n..].starts_with('m') {
                let params = if n == 0 { "0" } else { &after[..n] };
                for p in params.split(';') {
                    let code: u32 = if p.is_empty() { 0 } else { p.parse().unwrap() };
                    if code == 0 || code == 49 {
                        painted = false;
                    }
                    if code == 48 {
                        painted = true;
                    }
                }
                rest = &after[n + 1..];
                continue;
            }
        }
        cells.push(painted);
        let c = rest.chars().next().unwrap();
        rest = &rest[c.len_utf8()..];
    }
    cells
}

const WIDTH: usize = 20;

#[test]
fn paints_every_cell_of_a_plain_line() {
    assert_eq!(
        painted_cells(&apply_background_to_line("hello", WIDTH, band), WIDTH),
        vec![true; WIDTH]
    );
}

#[test]
fn keeps_the_fill_across_every_reset_spelling() {
    for reset in ["\x1b[49m", "\x1b[0m", "\x1b[m", "\x1b[0;32m", "\x1b[;32m"] {
        let rendered = apply_background_to_line(&format!("ab{reset}cd"), WIDTH, band);
        assert_eq!(
            painted_cells(&rendered, WIDTH),
            vec![true; WIDTH],
            "{reset:?}"
        );
    }
}

#[test]
fn keeps_the_fill_when_a_child_closes_at_the_end() {
    let rendered = apply_background_to_line("abcd\x1b[0m", WIDTH, band);
    assert_eq!(painted_cells(&rendered, WIDTH), vec![true; WIDTH]);
}

#[test]
fn leaves_a_childs_own_fill_alone() {
    let rendered = apply_background_to_line("a\x1b[48;5;33mchip\x1b[49mb", WIDTH, band);
    assert!(rendered.contains("\x1b[48;5;33mchip"));
    assert_eq!(painted_cells(&rendered, WIDTH), vec![true; WIDTH]);
}

#[test]
fn adds_no_width() {
    for line in ["plain", "ab\x1b[0mcd", "ab\x1b[49mcd"] {
        assert_eq!(
            visible_width(&apply_background_to_line(line, WIDTH, band)),
            WIDTH
        );
    }
}

#[test]
fn is_a_no_op_for_a_bg_fn_that_paints_nothing() {
    let rendered = apply_background_to_line("ab\x1b[0mcd", WIDTH, |t| t.to_string());
    assert_eq!(rendered, format!("ab\x1b[0mcd{}", " ".repeat(WIDTH - 4)));
}

fn link(url: &str, label: &str) -> String {
    format!("\x1b]8;;{url}\x07{label}\x1b]8;;\x07")
}

#[test]
fn hyperlink_at_finds_the_link_under_a_column_only() {
    let line = format!("see {} now", link("https://example.com", "docs"));
    let at = |c| hyperlink_at(&line, c);
    assert_eq!(at(0), None);
    assert_eq!(at(3), None);
    assert_eq!(at(4).as_deref(), Some("https://example.com"));
    assert_eq!(at(7).as_deref(), Some("https://example.com"));
    assert_eq!(at(8), None);
    assert_eq!(at(99), None);
    assert_eq!(at(-1), None);
}

#[test]
fn hyperlink_at_counts_cells() {
    let line = format!("🙂🙂{}", link("https://example.com", "x"));
    assert_eq!(hyperlink_at(&line, 3), None);
    assert_eq!(
        hyperlink_at(&line, 4).as_deref(),
        Some("https://example.com")
    );
}

#[test]
fn hyperlink_at_keeps_two_links_apart() {
    let line = format!(
        "{}--{}",
        link("https://a.test", "aa"),
        link("https://b.test", "bb")
    );
    assert_eq!(hyperlink_at(&line, 0).as_deref(), Some("https://a.test"));
    assert_eq!(hyperlink_at(&line, 2), None);
    assert_eq!(hyperlink_at(&line, 4).as_deref(), Some("https://b.test"));
}

#[test]
fn hyperlink_at_declines_lines_without_links() {
    assert_eq!(hyperlink_at("plain text", 2), None);
    assert_eq!(hyperlink_at("\x1b[31mred\x1b[0m", 1), None);
}

#[test]
fn bare_url_at_finds_plain_text_urls_only_under_them() {
    let line = "open http://127.0.0.1:4321/?t=abc now";
    assert_eq!(bare_url_at(line, 4), None);
    assert_eq!(
        bare_url_at(line, 5).as_deref(),
        Some("http://127.0.0.1:4321/?t=abc")
    );
    assert_eq!(
        bare_url_at(line, 32).as_deref(),
        Some("http://127.0.0.1:4321/?t=abc")
    );
    assert_eq!(bare_url_at(line, 33), None);
}

#[test]
fn bare_url_at_steps_over_styling_and_counts_cells() {
    let line = "🙂 \x1b[36mhttps://\x1b[1mexample.com\x1b[0m/x";
    assert_eq!(bare_url_at(line, 2), None);
    assert_eq!(
        bare_url_at(line, 3).as_deref(),
        Some("https://example.com/x")
    );
    assert_eq!(
        bare_url_at(line, 23).as_deref(),
        Some("https://example.com/x")
    );
}

#[test]
fn bare_url_at_leaves_sentence_punctuation_out() {
    assert_eq!(
        bare_url_at("see https://example.com/a.", 6).as_deref(),
        Some("https://example.com/a")
    );
    assert_eq!(
        bare_url_at("(see https://example.com/a)", 6).as_deref(),
        Some("https://example.com/a")
    );
    assert_eq!(
        bare_url_at("https://en.wikipedia.org/wiki/Foo_(bar)", 0).as_deref(),
        Some("https://en.wikipedia.org/wiki/Foo_(bar)")
    );
}

#[test]
fn bare_url_at_ignores_other_schemes_and_plain_text() {
    assert_eq!(bare_url_at("file:///etc/passwd", 3), None);
    assert_eq!(bare_url_at("plain text", 2), None);
    // `\b`: a scheme glued to a word is not a URL start.
    assert_eq!(bare_url_at("xhttps://a.b", 3), None);
}
