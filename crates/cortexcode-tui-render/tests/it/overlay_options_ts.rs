//! Case-for-case port of the pin's `test/overlay-options.test.ts`.

use crate::support::*;
use cortexcode_tui_render::{
    Component, OverlayAnchor, OverlayMargin, OverlayOptions, SizeValue, Tui,
};
use std::cell::RefCell;
use std::rc::Rc;

/// `StaticOverlay`: fixed lines, remembers the width it was rendered at.
struct StaticOverlay {
    lines: Vec<String>,
    requested_width: Option<u16>,
}

impl Component for StaticOverlay {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.requested_width = Some(width);
        self.lines.clone()
    }
}

fn overlay(lines: &[&str]) -> Rc<RefCell<StaticOverlay>> {
    Rc::new(RefCell::new(StaticOverlay {
        lines: lines.iter().map(|s| s.to_string()).collect(),
        requested_width: None,
    }))
}

/// A component rendering `f(width)`.
struct WidthFn(fn(u16) -> Vec<String>);

impl Component for WidthFn {
    fn render(&mut self, width: u16) -> Vec<String> {
        (self.0)(width)
    }
}

fn abs(v: i64) -> Option<SizeValue> {
    Some(SizeValue::Absolute(v))
}

fn pct(v: f64) -> Option<SizeValue> {
    Some(SizeValue::Percent(v))
}

fn width(w: i64) -> OverlayOptions {
    OverlayOptions {
        width: abs(w),
        ..Default::default()
    }
}

fn anchored(anchor: OverlayAnchor, w: i64) -> OverlayOptions {
    OverlayOptions {
        anchor: Some(anchor),
        ..width(w)
    }
}

/// Empty base content, `overlays` shown in order, then start + forced render.
fn run(
    cols: u16,
    rows: u16,
    overlays: Vec<(Rc<RefCell<StaticOverlay>>, OverlayOptions)>,
) -> (Tui, Handle) {
    run_on(cols, rows, Lines::new(&[]), overlays)
}

fn run_on(
    cols: u16,
    rows: u16,
    base: Rc<RefCell<dyn Component>>,
    overlays: Vec<(Rc<RefCell<StaticOverlay>>, OverlayOptions)>,
) -> (Tui, Handle) {
    let (terminal, term) = virtual_terminal(cols, rows);
    let mut tui = Tui::new(terminal, None);
    tui.add_child(base);
    for (o, opts) in overlays {
        tui.show_overlay(o, Some(opts));
    }
    let _events = tui.start();
    tui.request_render(true);
    (tui, term)
}

fn col_of(line: &str, needle: &str) -> Option<usize> {
    line.find(needle).map(|b| line[..b].chars().count())
}

// --- width overflow protection

#[test]
fn truncates_overlay_lines_that_exceed_declared_width() {
    let (mut tui, term) = run(80, 24, vec![(overlay(&[&"X".repeat(100)]), width(20))]);
    let screen = term.screen();
    assert!(screen.iter().all(|l| l.chars().count() <= 80));
    assert!(screen.iter().any(|l| l.contains(&"X".repeat(20))));
    assert!(!screen.iter().any(|l| l.contains(&"X".repeat(21))));
    tui.stop();
}

#[test]
fn handles_overlay_with_complex_ansi_sequences() {
    let complex = format!(
        "\x1b[48;2;40;50;40m \x1b[38;2;128;128;128mSome styled content\x1b[39m\x1b[49m\x1b]8;;http://example.com\x07link\x1b]8;;\x07{}",
        " more content ".repeat(10)
    );
    let (mut tui, term) = run(
        80,
        24,
        vec![(overlay(&[&complex, &complex, &complex]), width(60))],
    );
    assert!(!term.screen().is_empty());
    tui.stop();
}

#[test]
fn composites_overlay_on_styled_base_content() {
    let base = Rc::new(RefCell::new(WidthFn(|w| {
        let line = format!("\x1b[1m\x1b[38;2;255;0;0m{}\x1b[0m", "X".repeat(w as usize));
        vec![line.clone(), line.clone(), line]
    })));
    let (mut tui, term) = run_on(
        80,
        24,
        base,
        vec![(overlay(&["OVERLAY"]), anchored(OverlayAnchor::Center, 20))],
    );
    assert!(term.screen().iter().any(|l| l.contains("OVERLAY")));
    tui.stop();
}

#[test]
fn handles_wide_characters_at_overlay_boundary() {
    let (mut tui, term) = run(
        80,
        24,
        vec![(overlay(&["中文日本語한글テスト漢字"]), width(15))],
    );
    assert!(!term.screen().is_empty());
    tui.stop();
}

#[test]
fn handles_overlay_positioned_at_terminal_edge() {
    let opts = OverlayOptions {
        col: abs(60),
        ..width(20)
    };
    let (mut tui, term) = run(80, 24, vec![(overlay(&[&"X".repeat(50)]), opts)]);
    assert!(!term.screen().is_empty());
    tui.stop();
}

#[test]
fn handles_overlay_on_base_content_with_osc_sequences() {
    let base = Rc::new(RefCell::new(WidthFn(|w| {
        let link = "\x1b]8;;file:///path/to/file.ts\x07file.ts\x1b]8;;\x07";
        let line = format!("See {link} for details {}", "X".repeat(w as usize - 30));
        vec![line.clone(), line.clone(), line]
    })));
    let (mut tui, term) = run_on(
        80,
        24,
        base,
        vec![(
            overlay(&["OVERLAY-TEXT"]),
            anchored(OverlayAnchor::Center, 20),
        )],
    );
    assert!(!term.screen().is_empty());
    tui.stop();
}

// --- width percentage

#[test]
fn renders_overlay_at_percentage_of_terminal_width() {
    let o = overlay(&["test"]);
    let opts = OverlayOptions {
        width: pct(50.0),
        ..Default::default()
    };
    let (mut tui, _term) = run(100, 24, vec![(o.clone(), opts)]);
    assert_eq!(o.borrow().requested_width, Some(50));
    tui.stop();
}

#[test]
fn respects_min_width_over_small_percentage() {
    let o = overlay(&["test"]);
    let opts = OverlayOptions {
        width: pct(10.0),
        min_width: Some(30),
        ..Default::default()
    };
    let (mut tui, _term) = run(100, 24, vec![(o.clone(), opts)]);
    assert_eq!(o.borrow().requested_width, Some(30));
    tui.stop();
}

// --- anchor positioning

#[test]
fn positions_overlay_at_top_left() {
    let (mut tui, term) = run(
        80,
        24,
        vec![(overlay(&["TOP-LEFT"]), anchored(OverlayAnchor::TopLeft, 10))],
    );
    let screen = term.screen();
    assert!(screen[0].starts_with("TOP-LEFT"), "{:?}", screen[0]);
    tui.stop();
}

#[test]
fn positions_overlay_at_bottom_right() {
    let (mut tui, term) = run(
        80,
        24,
        vec![(
            overlay(&["BTM-RIGHT"]),
            anchored(OverlayAnchor::BottomRight, 10),
        )],
    );
    let last = &term.screen()[23];
    assert!(last.trim_end().ends_with("BTM-RIGHT"), "{last:?}");
    tui.stop();
}

#[test]
fn positions_overlay_at_top_center() {
    let (mut tui, term) = run(
        80,
        24,
        vec![(
            overlay(&["CENTERED"]),
            anchored(OverlayAnchor::TopCenter, 10),
        )],
    );
    let col = col_of(&term.screen()[0], "CENTERED").expect("CENTERED on row 0");
    assert!((30..=40).contains(&col), "col {col}");
    tui.stop();
}

// --- margin

#[test]
fn clamps_negative_margins_to_zero() {
    let opts = OverlayOptions {
        margin: Some(OverlayMargin {
            top: -5,
            left: -10,
            right: 0,
            bottom: 0,
        }),
        ..anchored(OverlayAnchor::TopLeft, 12)
    };
    let (mut tui, term) = run(80, 24, vec![(overlay(&["NEG-MARGIN"]), opts)]);
    assert!(term.screen()[0].starts_with("NEG-MARGIN"));
    tui.stop();
}

#[test]
fn respects_margin_as_number() {
    let opts = OverlayOptions {
        margin: Some(OverlayMargin::all(5)),
        ..anchored(OverlayAnchor::TopLeft, 10)
    };
    let (mut tui, term) = run(80, 24, vec![(overlay(&["MARGIN"]), opts)]);
    let screen = term.screen();
    assert!(!screen[0].contains("MARGIN"));
    assert!(!screen[4].contains("MARGIN"));
    assert_eq!(col_of(&screen[5], "MARGIN"), Some(5));
    tui.stop();
}

#[test]
fn respects_margin_object() {
    let opts = OverlayOptions {
        margin: Some(OverlayMargin {
            top: 2,
            left: 3,
            right: 0,
            bottom: 0,
        }),
        ..anchored(OverlayAnchor::TopLeft, 10)
    };
    let (mut tui, term) = run(80, 24, vec![(overlay(&["MARGIN"]), opts)]);
    assert_eq!(col_of(&term.screen()[2], "MARGIN"), Some(3));
    tui.stop();
}

// --- offset

#[test]
fn applies_offset_x_and_offset_y_from_anchor() {
    let opts = OverlayOptions {
        offset_x: Some(10),
        offset_y: Some(5),
        ..anchored(OverlayAnchor::TopLeft, 10)
    };
    let (mut tui, term) = run(80, 24, vec![(overlay(&["OFFSET"]), opts)]);
    assert_eq!(col_of(&term.screen()[5], "OFFSET"), Some(10));
    tui.stop();
}

// --- percentage positioning

#[test]
fn positions_with_row_and_col_percent() {
    let opts = OverlayOptions {
        row: pct(50.0),
        col: pct(50.0),
        ..width(10)
    };
    let (mut tui, term) = run(80, 24, vec![(overlay(&["PCT"]), opts)]);
    let row = term.screen().iter().position(|l| l.contains("PCT"));
    assert!(matches!(row, Some(10..=13)), "{row:?}");
    tui.stop();
}

#[test]
fn row_percent_0_positions_at_top() {
    let opts = OverlayOptions {
        row: pct(0.0),
        ..width(10)
    };
    let (mut tui, term) = run(80, 24, vec![(overlay(&["TOP"]), opts)]);
    assert!(term.screen()[0].contains("TOP"));
    tui.stop();
}

#[test]
fn row_percent_100_positions_at_bottom() {
    let opts = OverlayOptions {
        row: pct(100.0),
        ..width(10)
    };
    let (mut tui, term) = run(80, 24, vec![(overlay(&["BOTTOM"]), opts)]);
    assert!(term.screen()[23].contains("BOTTOM"));
    tui.stop();
}

// --- maxHeight

#[test]
fn truncates_overlay_to_max_height() {
    let opts = OverlayOptions {
        max_height: abs(3),
        ..Default::default()
    };
    let (mut tui, term) = run(
        80,
        24,
        vec![(
            overlay(&["Line 1", "Line 2", "Line 3", "Line 4", "Line 5"]),
            opts,
        )],
    );
    let content = term.screen().join("\n");
    for l in ["Line 1", "Line 2", "Line 3"] {
        assert!(content.contains(l), "{l}");
    }
    assert!(!content.contains("Line 4") && !content.contains("Line 5"));
    tui.stop();
}

#[test]
fn truncates_overlay_to_max_height_percent() {
    let opts = OverlayOptions {
        max_height: pct(50.0),
        ..Default::default()
    };
    let lines = ["L1", "L2", "L3", "L4", "L5", "L6", "L7", "L8", "L9", "L10"];
    let (mut tui, term) = run(80, 10, vec![(overlay(&lines), opts)]);
    let content = term.screen().join("\n");
    assert!(content.contains("L1") && content.contains("L5"));
    assert!(!content.contains("L6"));
    tui.stop();
}

// --- absolute positioning

#[test]
fn row_and_col_override_anchor() {
    let opts = OverlayOptions {
        row: abs(3),
        col: abs(5),
        ..anchored(OverlayAnchor::BottomRight, 10)
    };
    let (mut tui, term) = run(80, 24, vec![(overlay(&["ABSOLUTE"]), opts)]);
    assert_eq!(col_of(&term.screen()[3], "ABSOLUTE"), Some(5));
    tui.stop();
}

// --- stacked overlays

#[test]
fn later_overlays_render_on_top() {
    let (mut tui, term) = run(
        80,
        24,
        vec![
            (
                overlay(&["FIRST-OVERLAY"]),
                anchored(OverlayAnchor::TopLeft, 20),
            ),
            (overlay(&["SECOND"]), anchored(OverlayAnchor::TopLeft, 10)),
        ],
    );
    assert!(term.screen()[0].contains("SECOND"));
    tui.stop();
}

#[test]
fn overlays_at_different_positions_do_not_interfere() {
    let (mut tui, term) = run(
        80,
        24,
        vec![
            (overlay(&["TOP-LEFT"]), anchored(OverlayAnchor::TopLeft, 15)),
            (
                overlay(&["BTM-RIGHT"]),
                anchored(OverlayAnchor::BottomRight, 15),
            ),
        ],
    );
    let screen = term.screen();
    assert!(screen[0].contains("TOP-LEFT"));
    assert!(screen[23].contains("BTM-RIGHT"));
    tui.stop();
}

#[test]
fn hides_overlays_in_stack_order() {
    let (mut tui, term) = run(
        80,
        24,
        vec![
            (overlay(&["FIRST"]), anchored(OverlayAnchor::TopLeft, 10)),
            (overlay(&["SECOND"]), anchored(OverlayAnchor::TopLeft, 10)),
        ],
    );
    assert!(term.screen()[0].contains("SECOND"));
    tui.hide_overlay();
    tui.request_render(true);
    assert!(term.screen()[0].contains("FIRST"));
    tui.stop();
}
