//! Ports of the pin's `test/terminal-image.test.ts` (the `Image` component
//! case lives in cortexcode-tui-components' `tests/image_component.rs`) and
//! `test/bug-regression-isimageline-startswith-bug.test.ts`.
//!
//! Env vars and the capability / cell-size caches are process globals, so the
//! tests that touch them hold `GLOBALS`.

use cortexcode_tui_images::{
    delete_all_kitty_images, delete_kitty_image, detect_capabilities, encode_kitty, hyperlink,
    is_image_line, render_image, reset_capabilities_cache, set_capabilities, set_cell_dimensions,
    CellDimensions, ImageDimensions, ImageProtocol, ImageRenderOptions, KittyEncodeOptions,
    TerminalCapabilities,
};
use std::sync::{Mutex, MutexGuard};

static GLOBALS: Mutex<()> = Mutex::new(());

fn globals() -> MutexGuard<'static, ()> {
    GLOBALS.lock().unwrap_or_else(|e| e.into_inner())
}

// HOOCODE_IMAGE_PROTOCOL is CORTEX_IMAGE_PROTOCOL here (branding).
const ENV_KEYS: &[&str] = &[
    "TERM",
    "TERM_PROGRAM",
    "COLORTERM",
    "TMUX",
    "KITTY_WINDOW_ID",
    "GHOSTTY_RESOURCES_DIR",
    "WEZTERM_PANE",
    "ITERM_SESSION_ID",
    "CMUX_WORKSPACE_ID",
    "WT_SESSION",
    "CORTEX_IMAGE_PROTOCOL",
];

fn with_env(overrides: &[(&str, &str)], f: impl FnOnce()) {
    let _g = globals();
    let saved: Vec<_> = ENV_KEYS
        .iter()
        .map(|k| (*k, std::env::var(k).ok()))
        .collect();
    for k in ENV_KEYS {
        std::env::remove_var(k);
    }
    for (k, v) in overrides {
        std::env::set_var(k, v);
    }
    f();
    for (k, v) in saved {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
}

// --- isImageLine

#[test]
fn detects_iterm2_sequences_anywhere_in_the_line() {
    for line in [
        "\x1b]1337;File=size=100,100;inline=1:base64encodeddata==\x07",
        "Some text \x1b]1337;File=size=100,100;inline=1:base64data==\x07 more text",
        "Text before image...\x1b]1337;File=inline=1:verylongbase64data==...text after",
        "Regular text ending with \x1b]1337;File=inline=1:base64data==\x07",
        "\x1b]1337;File=:\x07",
    ] {
        assert!(is_image_line(line), "{line:?}");
    }
}

#[test]
fn detects_kitty_sequences() {
    for line in [
        "\x1b_Ga=T,f=100,t=f,d=base64data...\x1b\\\x1b_Gm=i=1;\x1b\\",
        "Output: \x1b_Ga=T,f=100;data...\x1b\\\x1b_Gm=i=1;\x1b\\",
        "  \x1b_Ga=T,f=100...\x1b\\\x1b_Gm=i=1;\x1b\\  ",
    ] {
        assert!(is_image_line(line), "{line:?}");
    }
}

#[test]
fn detects_image_sequences_in_very_long_lines() {
    let long = format!(
        "Text prefix \x1b]1337;File=size=800,600;inline=1:{} suffix",
        "A".repeat(100).repeat(3000)
    );
    assert!(long.len() > 300_000);
    assert!(is_image_line(&long));
}

#[test]
fn detects_image_sequences_regardless_of_terminal_support_and_ansi() {
    for line in [
        "Read image file [image/jpeg]\x1b]1337;File=inline=1:base64data==\x07",
        "\x1b[31mError output \x1b]1337;File=inline=1:image==\x07",
        "\x1b_Ga=T,f=100:data...\x1b\\\x1b_Gm=i=1;\x1b\\\x1b[0m reset",
    ] {
        assert!(is_image_line(line), "{line:?}");
    }
}

#[test]
fn no_images_in_lines_without_image_sequences() {
    for line in [
        "This is just a regular text line without any escape sequences",
        "\x1b[31mRed text\x1b[0m and \x1b[32mgreen text\x1b[0m",
        "\x1b[1A\x1b[2KLine cleared and moved up",
        "Some text with ]1337;File but missing ESC at start",
        "Some text with _G but missing ESC at start",
        "",
        "\n",
        "\n\n",
        "/path/to/File_1337_backup/image.jpg",
    ] {
        assert!(!is_image_line(line), "{line:?}");
    }
}

#[test]
fn detects_mixed_and_multiple_image_segments() {
    assert!(is_image_line(
        "Kitty: \x1b_Ga=T...\x1b\\\x1b_Gm=i=1;\x1b\\ iTerm2: \x1b]1337;File=inline=1:data==\x07"
    ));
    assert!(is_image_line(
        "Start \x1b]1337;File=img1==\x07 middle \x1b]1337;File=img2==\x07 end"
    ));
}

// --- bug-regression-isimageline-startswith-bug.test.ts

#[test]
fn regression_old_startswith_implementation_missed_the_sequence() {
    // The old `prefix !== null && line.startsWith(prefix)`, with no image support.
    let old_is_image_line =
        |line: &str, prefix: Option<&str>| prefix.is_some_and(|p| line.starts_with(p));
    let line = "Read image file [image/jpeg]\x1b]1337;File=size=800,600;inline=1:base64data...\x07";
    assert!(!old_is_image_line(line, None));
    assert!(is_image_line(line));
}

#[test]
fn regression_detects_kitty_and_iterm2_in_any_position() {
    let long_kitty = format!(
        "Text before \x1b_Ga=T,f=100{} text after",
        "A".repeat(300_000)
    );
    let long_iterm = format!(
        "Text before \x1b]1337;File=size=800,600;inline=1:{} text after",
        "B".repeat(300_000)
    );
    for line in [
        "At start: \x1b_Ga=T,f=100,data...\x1b\\",
        "Prefix \x1b_Ga=T,data...\x1b\\",
        "Suffix text \x1b_Ga=T,data...\x1b\\ suffix",
        "Middle \x1b_Ga=T,data...\x1b\\ more text",
        &long_kitty,
        "At start: \x1b]1337;File=size=100,100:base64...\x07",
        "Prefix \x1b]1337;File=inline=1:data==\x07",
        "Suffix text \x1b]1337;File=inline=1:data==\x07 suffix",
        "Middle \x1b]1337;File=inline=1:data==\x07 more text",
        &long_iterm,
    ] {
        assert!(is_image_line(line), "{:?}", &line[..line.len().min(50)]);
    }
}

#[test]
fn regression_tool_output_and_ansi_prefixed_lines() {
    for line in [
        "Read image file [image/jpeg]\x1b]1337;File=size=800,600;inline=1:base64image...\x07",
        "\x1b_Ga=T,f=100,t=f,d=base64data...\x1b\\\x1b_Gm=i=1;\x1b\\",
        "\x1b[31mError\x1b[0m: \x1b]1337;File=inline=1:base64==\x07",
        "\x1b[33mWarning\x1b[0m: \x1b_Ga=T,data...\x1b\\",
        "\x1b[1mBold\x1b[0m \x1b]1337;File=:base64==\x07\x1b[0m",
    ] {
        assert!(is_image_line(line), "{line:?}");
    }
}

#[test]
fn regression_crash_log_dimensions() {
    let crash = format!(
        "Output: \x1b]1337;File=size=800,600;inline=1:{} end of output",
        "A".repeat(100).repeat(3040)
    );
    assert!(crash.len() > 300_000);
    assert!(is_image_line(&crash));

    let seq = "\x1b_Ga=T,f=100";
    let line = format!("Text{seq}{}End", "A".repeat(58649 - 4 - seq.len() - 3));
    assert_eq!(line.len(), 58649);
    assert!(is_image_line(&line));
}

#[test]
fn regression_no_false_positives() {
    assert!(!is_image_line(&"A".repeat(100_000)));
    for path in [
        "/path/to/1337/image.jpg",
        "/usr/local/bin/File_converter",
        "~/Documents/1337File_backup.png",
        "./_G_test_file.txt",
    ] {
        assert!(!is_image_line(path), "{path}");
    }
}

// --- detectCapabilities

#[test]
fn unknown_terminals_default_to_no_hyperlinks_and_no_images() {
    with_env(&[], || {
        let caps = detect_capabilities();
        assert!(!caps.hyperlinks);
        assert_eq!(caps.images, None);
    });
}

#[test]
fn tmux_and_screen_force_no_hyperlinks_and_no_images() {
    let cases: [&[(&str, &str)]; 3] = [
        &[
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("TERM_PROGRAM", "ghostty"),
        ],
        &[("TERM", "tmux-256color"), ("TERM_PROGRAM", "iterm.app")],
        &[("TERM", "screen-256color")],
    ];
    for env in cases {
        with_env(env, || {
            let caps = detect_capabilities();
            assert!(!caps.hyperlinks, "{env:?}");
            assert_eq!(caps.images, None, "{env:?}");
        });
    }
}

#[test]
fn hyperlink_capable_terminals() {
    for env in [
        ("TERM_PROGRAM", "ghostty"),
        ("KITTY_WINDOW_ID", "1"),
        ("WEZTERM_PANE", "0"),
        ("TERM_PROGRAM", "iterm.app"),
        ("TERM_PROGRAM", "vscode"),
    ] {
        with_env(&[env], || {
            assert!(detect_capabilities().hyperlinks, "{env:?}")
        });
    }
}

#[test]
fn cmux_does_not_disable_ghostty_images() {
    with_env(
        &[
            ("TERM_PROGRAM", "ghostty"),
            ("CMUX_WORKSPACE_ID", "workspace"),
        ],
        || {
            let caps = detect_capabilities();
            assert_eq!(caps.images, Some(ImageProtocol::Kitty));
            assert!(caps.hyperlinks);
        },
    );
}

// --- Kitty image cursor movement

#[test]
fn encode_kitty_can_request_no_cursor_movement() {
    let seq = encode_kitty(
        "AAAA",
        &KittyEncodeOptions {
            columns: Some(2),
            rows: Some(2),
            move_cursor: Some(false),
            ..Default::default()
        },
    );
    assert!(
        seq.starts_with("\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2;"),
        "{seq:?}"
    );
}

#[test]
fn delete_commands_suppress_kitty_replies() {
    assert_eq!(delete_kitty_image(42), "\x1b_Ga=d,d=I,i=42,q=2\x1b\\");
    assert_eq!(delete_all_kitty_images(), "\x1b_Ga=d,d=A,q=2\x1b\\");
}

fn with_kitty_10px(f: impl FnOnce()) {
    let _g = globals();
    set_capabilities(TerminalCapabilities {
        images: Some(ImageProtocol::Kitty),
        true_color: true,
        hyperlinks: true,
    });
    set_cell_dimensions(CellDimensions {
        width_px: 10,
        height_px: 10,
    });
    f();
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

const DIMS: ImageDimensions = ImageDimensions {
    width_px: 20,
    height_px: 20,
};

#[test]
fn render_image_keeps_default_cursor_movement() {
    with_kitty_10px(|| {
        let opts = ImageRenderOptions {
            max_width_cells: Some(2),
            ..Default::default()
        };
        let r = render_image("AAAA", DIMS, &opts).unwrap();
        assert!(!r.sequence.contains(",C=1,"));
        assert_eq!(r.rows, 2);
    });
}

#[test]
fn render_image_can_opt_out_of_cursor_movement() {
    with_kitty_10px(|| {
        let opts = ImageRenderOptions {
            max_width_cells: Some(2),
            move_cursor: Some(false),
            ..Default::default()
        };
        let r = render_image("AAAA", DIMS, &opts).unwrap();
        assert!(r.sequence.contains(",C=1,"));
        assert_eq!(r.rows, 2);
    });
}

// --- hyperlink

#[test]
fn hyperlink_wraps_text_in_osc8() {
    assert_eq!(
        hyperlink("click me", "https://example.com"),
        "\x1b]8;;https://example.com\x1b\\click me\x1b]8;;\x1b\\"
    );
}

#[test]
fn hyperlink_preserves_ansi_styling_inside() {
    let styled = "\x1b[4m\x1b[34mclick me\x1b[0m";
    let r = hyperlink(styled, "https://example.com");
    assert!(r.starts_with("\x1b]8;;https://example.com\x1b\\"));
    assert!(r.contains(styled));
    assert!(r.ends_with("\x1b]8;;\x1b\\"));
}

#[test]
fn hyperlink_with_empty_text() {
    assert_eq!(
        hyperlink("", "https://example.com"),
        "\x1b]8;;https://example.com\x1b\\\x1b]8;;\x1b\\"
    );
}

#[test]
fn hyperlink_with_file_uri() {
    let r = hyperlink("README.md", "file:///home/user/README.md");
    assert!(r.contains("file:///home/user/README.md") && r.contains("README.md"));
}
