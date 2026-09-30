//! Ports of the pin's `test/regression-regional-indicator-width.test.ts` and
//! `test/truncate-to-width.test.ts`.

use cortexcode_tui_util::{
    normalize_terminal_output, truncate_to_width, visible_width, wrap_text_with_ansi,
};

// --- regression-regional-indicator-width.test.ts

#[test]
fn partial_flag_grapheme_is_full_width() {
    assert_eq!(visible_width("🇨"), 2);
    assert_eq!(visible_width("      - 🇨"), 10);
}

#[test]
fn wraps_intermediate_partial_flag_list_line_before_overflow() {
    let wrapped = wrap_text_with_ansi("      - 🇨", 9);
    assert_eq!(wrapped.len(), 2);
    assert_eq!(visible_width(&wrapped[0]), 7);
    assert_eq!(visible_width(&wrapped[1]), 2);
}

#[test]
fn all_regional_indicator_singletons_are_width_2() {
    for cp in 0x1f1e6..=0x1f1ff {
        let s = char::from_u32(cp).unwrap().to_string();
        assert_eq!(visible_width(&s), 2, "U+{cp:X}");
    }
}

#[test]
fn full_flag_pairs_are_width_2() {
    for flag in ["🇯🇵", "🇺🇸", "🇬🇧", "🇨🇳", "🇩🇪", "🇫🇷"] {
        assert_eq!(visible_width(flag), 2, "{flag}");
    }
}

#[test]
fn common_streaming_emoji_intermediates_are_width_2() {
    for s in ["👍", "👍🏻", "✅", "⚡", "⚡️", "👨", "👨‍💻", "🏳️‍🌈"] {
        assert_eq!(visible_width(s), 2, "{s}");
    }
}

// --- truncate-to-width.test.ts

#[test]
fn keeps_output_within_width_for_very_large_unicode_input() {
    let text = "🙂界".repeat(100_000);
    let t = truncate_to_width(&text, 40, "…", false);
    assert!(visible_width(&t) <= 40);
    assert!(t.ends_with("…\x1b[0m"));
}

#[test]
fn preserves_ansi_styling_and_resets_around_ellipsis() {
    let text = format!("\x1b[31m{}\x1b[0m", "hello ".repeat(1000));
    let t = truncate_to_width(&text, 20, "…", false);
    assert!(visible_width(&t) <= 20);
    assert!(t.contains("\x1b[31m"));
    assert!(t.ends_with("\x1b[0m…\x1b[0m"));
}

#[test]
fn handles_malformed_ansi_escape_prefixes_without_hanging() {
    let text = format!("abc\x1bnot-ansi {}", "🙂".repeat(1000));
    assert!(visible_width(&truncate_to_width(&text, 20, "…", false)) <= 20);
}

#[test]
fn clips_wide_ellipsis_safely_and_brackets_it_with_resets() {
    assert_eq!(truncate_to_width("abcdef", 1, "🙂", false), "");
    assert_eq!(
        truncate_to_width("abcdef", 2, "🙂", false),
        "\x1b[0m🙂\x1b[0m"
    );
    assert!(visible_width(&truncate_to_width("abcdef", 2, "🙂", false)) <= 2);
}

#[test]
fn returns_original_text_when_it_fits_even_if_ellipsis_too_wide() {
    assert_eq!(truncate_to_width("a", 2, "🙂", false), "a");
    assert_eq!(truncate_to_width("界", 2, "🙂", false), "界");
}

#[test]
fn pads_truncated_output_to_requested_width() {
    assert_eq!(
        visible_width(&truncate_to_width("🙂界🙂界🙂界", 8, "…", true)),
        8
    );
}

#[test]
fn adds_trailing_reset_when_truncating_without_ellipsis() {
    let t = truncate_to_width(&format!("\x1b[31m{}", "hello".repeat(100)), 10, "", false);
    assert!(visible_width(&t) <= 10);
    assert!(t.ends_with("\x1b[0m"));
}

#[test]
fn keeps_contiguous_prefix_instead_of_skipping_wide_grapheme() {
    let t = truncate_to_width("🙂\t界 \x1b_abc\x07", 7, "…", true);
    assert_eq!(t, "🙂\t\x1b[0m…\x1b[0m ");
}

#[test]
fn visible_width_counts_tabs_inline_and_skips_ansi() {
    assert_eq!(visible_width("\t\x1b[31m界\x1b[0m"), 5);
}

#[test]
fn thai_and_lao_am_clusters_keep_normal_width() {
    assert_eq!(visible_width("ำ"), 1);
    assert_eq!(visible_width("ຳ"), 1);
    assert_eq!(visible_width("กำ"), 2);
    assert_eq!(visible_width("ກຳ"), 2);
}

#[test]
fn normalizes_thai_and_lao_am_only_for_terminal_output() {
    assert_eq!(normalize_terminal_output("ำ"), "ํา");
    assert_eq!(normalize_terminal_output("ຳ"), "ໍາ");
    assert_eq!(
        visible_width(&normalize_terminal_output("ำabc")),
        visible_width("ำabc")
    );
    assert_eq!(
        visible_width(&normalize_terminal_output("ຳabc")),
        visible_width("ຳabc")
    );
}

// --- coding-agent/test/truncate-to-width.test.ts (default "..." ellipsis)

fn truncate(text: &str, width: usize) -> String {
    truncate_to_width(text, width, "...", false)
}

#[test]
fn coding_agent_truncates_unicode_messages_within_width() {
    for (message, width) in [
        (
            "✔ script to run › dev $ concurrently \"vite\" \"node --import tsx ./",
            67,
        ),
        (
            "🎉 Celebration! 🚀 Launch 📦 Package ready for deployment now",
            40,
        ),
        ("Hello 世界 Test 你好 More text here that is long", 30),
    ] {
        let max = width - 2;
        assert!(visible_width(&truncate(message, max)) <= max, "{message}");
    }
}

#[test]
fn coding_agent_does_not_truncate_messages_that_fit() {
    assert_eq!(truncate("Short message", 48), "Short message");
}

#[test]
fn coding_agent_adds_ellipsis_when_truncating() {
    let t = truncate("This is a very long message that needs to be truncated", 28);
    assert!(t.contains("..."));
    assert!(visible_width(&t) <= 28);
}

#[test]
fn coding_agent_exact_crash_case_from_issue_report() {
    let message = "✔ script to run › dev $ concurrently \"vite\" \"node --import tsx ./server.ts\"";
    assert!(visible_width(&truncate(message, 65)) + 2 <= 67);
}
