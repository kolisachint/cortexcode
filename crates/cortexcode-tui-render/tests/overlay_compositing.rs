//! Ports of the pin's `test/overlay-short-content.test.ts` and
//! `test/tui-overlay-style-leak.test.ts`.

mod support;

use cortexcode_tui_render::{OverlayOptions, SizeValue, Tui};
use support::*;

#[test]
fn renders_overlay_when_content_is_shorter_than_terminal() {
    let (terminal, term) = virtual_terminal(80, 24);
    let mut tui = Tui::new(terminal, None);
    tui.add_child(Lines::new(&["Line 1", "Line 2", "Line 3"]));
    tui.show_overlay(
        Lines::new(&["OVERLAY_TOP", "OVERLAY_MID", "OVERLAY_BOT"]),
        None,
    );
    let _events = tui.start();
    let screen = term.screen();
    assert!(
        screen.iter().any(|l| l.contains("OVERLAY")),
        "overlay should be visible:\n{}",
        screen.join("\n")
    );
    tui.stop();
}

fn italic_base_line(width: usize) -> String {
    format!("\x1b[3m{}\x1b[23m", "X".repeat(width))
}

#[test]
fn no_style_leak_when_trailing_reset_is_beyond_last_column() {
    let (terminal, term) = virtual_terminal(20, 6);
    let mut tui = Tui::new(terminal, None);
    tui.add_child(Lines::new(&[&italic_base_line(20), "INPUT"]));
    let _events = tui.start();
    tui.request_render(true);
    assert!(!term.cell_italic(1, 0));
    tui.stop();
}

#[test]
fn no_style_leak_when_overlay_slicing_drops_trailing_resets() {
    let (terminal, term) = virtual_terminal(20, 6);
    let mut tui = Tui::new(terminal, None);
    tui.add_child(Lines::new(&[&italic_base_line(20), "INPUT"]));
    tui.show_overlay(
        Lines::new(&["OVR"]),
        Some(OverlayOptions {
            row: Some(SizeValue::Absolute(0)),
            col: Some(SizeValue::Absolute(5)),
            width: Some(SizeValue::Absolute(3)),
            ..Default::default()
        }),
    );
    let _events = tui.start();
    tui.request_render(true);
    assert!(!term.cell_italic(1, 0));
    tui.stop();
}
