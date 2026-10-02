//! Port of the pin's `test/wrap-ansi.test.ts`.

use cortexcode_tui_util::{visible_width, wrap_text_with_ansi};

const UL_ON: &str = "\x1b[4m";
const UL_OFF: &str = "\x1b[24m";
const RESET: &str = "\x1b[0m";

/// Drops OSC 8 hyperlink sequences (ST-terminated) and SGR codes.
fn strip_osc8_and_sgr(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("\x1b]8;;") {
            let end = after.find("\x1b\\").map(|i| i + 2).unwrap_or(after.len());
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix("\x1b[") {
            let n = after
                .bytes()
                .take_while(|b| b.is_ascii_digit() || *b == b';')
                .count();
            if after[n..].starts_with('m') {
                rest = &after[n + 1..];
            } else {
                out.push(c);
                rest = &rest[1..];
            }
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

#[test]
fn underline_not_applied_before_the_styled_text() {
    let url = "https://example.com/very/long/path/that/will/wrap";
    let wrapped = wrap_text_with_ansi(&format!("read this thread {UL_ON}{url}{UL_OFF}"), 40);
    assert_eq!(wrapped[0], "read this thread");
    assert!(wrapped[1].starts_with(UL_ON));
    assert!(wrapped[1].contains("https://"));
}

#[test]
fn no_whitespace_before_underline_reset_code() {
    let wrapped = wrap_text_with_ansi(&format!("{UL_ON}underlined text here {UL_OFF}more"), 18);
    assert!(!wrapped[0].contains(&format!(" {UL_OFF}")));
}

#[test]
fn underline_does_not_bleed_to_padding() {
    let url = "https://example.com/very/long/path/that/will/definitely/wrap";
    let wrapped = wrap_text_with_ansi(&format!("prefix {UL_ON}{url}{UL_OFF} suffix"), 30);
    for line in &wrapped[1..wrapped.len() - 1] {
        if line.contains(UL_ON) {
            assert!(line.ends_with(UL_OFF));
            assert!(!line.ends_with(RESET));
        }
    }
}

#[test]
fn preserves_background_across_wrapped_lines_without_full_reset() {
    let bg = "\x1b[44m";
    let wrapped = wrap_text_with_ansi(
        &format!("{bg}hello world this is blue background text{RESET}"),
        15,
    );
    for line in &wrapped {
        assert!(line.contains(bg));
    }
    for line in &wrapped[..wrapped.len() - 1] {
        assert!(!line.ends_with(RESET));
    }
}

#[test]
fn resets_underline_but_preserves_background_inside_background() {
    let text = format!("\x1b[41mprefix {UL_ON}UNDERLINED_CONTENT_THAT_WRAPS{UL_OFF} suffix{RESET}");
    let wrapped = wrap_text_with_ansi(&text, 20);
    for line in &wrapped {
        assert!(line.contains("[41m") || line.contains(";41m") || line.contains("[41;"));
    }
    for line in &wrapped[..wrapped.len() - 1] {
        let has_ul = line.contains("[4m") || line.contains("[4;") || line.contains(";4m");
        if has_ul && !line.contains(UL_OFF) {
            assert!(line.ends_with(UL_OFF));
            assert!(!line.ends_with(RESET));
        }
    }
}

#[test]
fn wraps_plain_text() {
    let wrapped = wrap_text_with_ansi("hello world this is a test", 10);
    assert!(wrapped.len() > 1);
    for line in &wrapped {
        assert!(visible_width(line) <= 10);
    }
}

#[test]
fn ignores_osc_133_markers_in_visible_width() {
    assert_eq!(visible_width("\x1b]133;A\x07hello\x1b]133;B\x07"), 5);
}

#[test]
fn ignores_st_terminated_osc_in_visible_width() {
    assert_eq!(visible_width("\x1b]133;A\x1b\\hello\x1b]133;B\x1b\\"), 5);
}

#[test]
fn isolated_regional_indicators_are_width_2() {
    assert_eq!(visible_width("🇨"), 2);
    assert_eq!(visible_width("🇨🇳"), 2);
}

#[test]
fn truncates_trailing_whitespace_that_exceeds_width() {
    assert!(visible_width(&wrap_text_with_ansi("  ", 1)[0]) <= 1);
}

#[test]
fn preserves_color_codes_across_wraps() {
    let red = "\x1b[31m";
    let wrapped = wrap_text_with_ansi(&format!("{red}hello world this is red{RESET}"), 10);
    for line in &wrapped[1..] {
        assert!(line.starts_with(red));
    }
    for line in &wrapped[..wrapped.len() - 1] {
        assert!(!line.ends_with(RESET));
    }
}

const URL: &str = "https://example.com";

fn link_input() -> String {
    format!("\x1b]8;;{URL}\x1b\\0123456789\x1b]8;;\x1b\\")
}

#[test]
fn reemits_osc8_open_at_start_of_continuation_lines() {
    let open = format!("\x1b]8;;{URL}\x1b\\");
    for line in wrap_text_with_ansi(&link_input(), 6) {
        if !strip_osc8_and_sgr(&line).trim().is_empty() {
            assert!(
                line.contains(&open),
                "{line:?} has visible text but no OSC 8 re-open"
            );
        }
    }
}

#[test]
fn closes_osc8_before_each_line_break() {
    let open = format!("\x1b]8;;{URL}\x1b\\");
    let lines = wrap_text_with_ansi(&link_input(), 6);
    for line in &lines[..lines.len() - 1] {
        if line.contains(&open) {
            assert!(
                line.ends_with("\x1b]8;;\x1b\\"),
                "{line:?} does not close the link"
            );
        }
    }
}

#[test]
fn preserves_bel_terminators_when_wrapping_oauth_style_links() {
    let url = format!("https://example.com/oauth/{}", "a".repeat(32));
    let lines = wrap_text_with_ansi(&format!("\x1b]8;;{url}\x07{url}\x1b]8;;\x07"), 20);
    assert!(lines.len() > 1);
    for line in &lines {
        assert!(line.contains(&format!("\x1b]8;;{url}\x07")), "{line:?}");
        assert!(!line.contains(&format!("\x1b]8;;{url}\x1b\\")), "{line:?}");
    }
    for line in &lines[..lines.len() - 1] {
        assert!(line.ends_with("\x1b]8;;\x07"), "{line:?}");
    }
}

#[test]
fn no_osc8_on_lines_outside_the_link() {
    let lines = wrap_text_with_ansi(
        &format!("before \x1b]8;;{URL}\x1b\\link\x1b]8;;\x1b\\ after"),
        80,
    );
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].matches("\x1b]8;;https:").count(), 1);
    assert_eq!(lines[0].matches("\x1b]8;;\x1b\\").count(), 1);
}
