//! Case-for-case port of the pin's `test/overlay-non-capturing.test.ts`.
//! `OverlayHandle.focus()/unfocus()/hide()/setHidden()/isFocused()` are
//! `Tui::focus_overlay/unfocus_overlay/remove_overlay/set_overlay_hidden/
//! is_overlay_focused` here. The TS "microtask-deferred sub-overlay" case has
//! no microtasks to defer through in Rust; it runs the same sequence inline.

use crate::support::*;
use cortexcode_tui_render::{Component, OverlayHandle, OverlayOptions, SizeValue, Tui, TuiEvent};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Focusable {
    lines: Vec<String>,
    focused: bool,
    inputs: Vec<String>,
}

impl Component for Focusable {
    fn render(&mut self, _width: u16) -> Vec<String> {
        self.lines.clone()
    }
    fn handle_input(&mut self, data: &str) {
        self.inputs.push(data.to_string());
    }
    fn is_focusable(&self) -> bool {
        true
    }
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}

type F = Rc<RefCell<Focusable>>;

fn focusable(line: &str) -> F {
    Rc::new(RefCell::new(Focusable {
        lines: vec![line.to_string()],
        ..Default::default()
    }))
}

fn focused(c: &F) -> bool {
    c.borrow().focused
}

fn inputs(c: &F) -> Vec<String> {
    c.borrow().inputs.clone()
}

fn nc() -> Option<OverlayOptions> {
    Some(OverlayOptions {
        non_capturing: true,
        ..Default::default()
    })
}

/// 80x24 with empty content, `editor` focused (when given), started.
fn setup(editor: Option<&F>) -> (Tui, Handle) {
    setup_size(80, 24, editor)
}

fn setup_size(cols: u16, rows: u16, editor: Option<&F>) -> (Tui, Handle) {
    let (terminal, term) = virtual_terminal(cols, rows);
    let mut tui = Tui::new(terminal, None);
    tui.add_child(Lines::new(&[]));
    if let Some(e) = editor {
        tui.set_focus(Some(e.clone()));
    }
    let _events = tui.start();
    (tui, term)
}

fn flush(tui: &mut Tui) {
    tui.request_render(true);
}

fn send(tui: &mut Tui, data: &str) {
    tui.process_event(TuiEvent::Input(data.to_string()));
}

// --- focus management

#[test]
fn non_capturing_overlay_preserves_focus_on_creation() {
    let (editor, overlay) = (focusable("EDITOR"), focusable("OVERLAY"));
    let (mut tui, _t) = setup(Some(&editor));
    tui.show_overlay(overlay.clone(), nc());
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!focused(&overlay));
    tui.stop();
}

#[test]
fn focus_transfers_focus_to_the_overlay() {
    let (editor, overlay) = (focusable("EDITOR"), focusable("OVERLAY"));
    let (mut tui, _t) = setup(Some(&editor));
    let h = tui.show_overlay(overlay.clone(), nc());
    tui.focus_overlay(h);
    flush(&mut tui);
    assert!(!focused(&editor));
    assert!(focused(&overlay));
    assert!(tui.is_overlay_focused(h));
    tui.stop();
}

#[test]
fn unfocus_restores_previous_focus() {
    let (editor, overlay) = (focusable("EDITOR"), focusable("OVERLAY"));
    let (mut tui, _t) = setup(Some(&editor));
    let h = tui.show_overlay(overlay.clone(), nc());
    tui.focus_overlay(h);
    tui.unfocus_overlay(h);
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!focused(&overlay));
    assert!(!tui.is_overlay_focused(h));
    tui.stop();
}

#[test]
fn unhiding_a_non_capturing_overlay_does_not_auto_focus() {
    let (editor, overlay) = (focusable("EDITOR"), focusable("OVERLAY"));
    let (mut tui, _t) = setup(Some(&editor));
    let h = tui.show_overlay(overlay.clone(), nc());
    tui.set_overlay_hidden(h, true);
    tui.set_overlay_hidden(h, false);
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!focused(&overlay));
    tui.stop();
}

#[test]
fn hide_when_not_focused_does_not_change_focus() {
    let (editor, overlay) = (focusable("EDITOR"), focusable("OVERLAY"));
    let (mut tui, _t) = setup(Some(&editor));
    let h = tui.show_overlay(overlay, nc());
    tui.remove_overlay(h);
    flush(&mut tui);
    assert!(focused(&editor));
    tui.stop();
}

#[test]
fn hide_when_focused_restores_focus() {
    let (editor, overlay) = (focusable("EDITOR"), focusable("OVERLAY"));
    let (mut tui, _t) = setup(Some(&editor));
    let h = tui.show_overlay(overlay.clone(), nc());
    tui.focus_overlay(h);
    tui.remove_overlay(h);
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!focused(&overlay));
    tui.stop();
}

#[test]
fn removing_capturing_overlay_above_non_capturing_restores_editor() {
    let (editor, nonc, cap) = (focusable("EDITOR"), focusable("NC"), focusable("CAP"));
    let (mut tui, _t) = setup(Some(&editor));
    tui.show_overlay(nonc.clone(), nc());
    let h = tui.show_overlay(cap.clone(), None);
    assert!(focused(&cap));
    tui.remove_overlay(h);
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!focused(&nonc));
    tui.stop();
}

/// Timer (non-capturing) then controller (capturing); cleanup hides the
/// timer by handle, then the topmost overlay.
fn sub_overlay_cleanup_restores_editor() {
    let (editor, timer, ctrl) = (focusable("EDITOR"), focusable("TIMER"), focusable("CTRL"));
    let (mut tui, _t) = setup(Some(&editor));
    let timer_handle = tui.show_overlay(timer.clone(), nc());
    tui.show_overlay(ctrl.clone(), None);
    flush(&mut tui);
    assert!(focused(&ctrl));
    assert!(!focused(&editor));
    tui.remove_overlay(timer_handle);
    tui.hide_overlay();
    flush(&mut tui);
    assert!(focused(&editor), "editor should regain focus");
    assert!(!focused(&ctrl));
    assert!(!focused(&timer));
    send(&mut tui, "x");
    flush(&mut tui);
    assert_eq!(inputs(&editor), ["x"]);
    assert!(inputs(&ctrl).is_empty());
    assert!(inputs(&timer).is_empty());
    tui.stop();
}

#[test]
fn sub_overlay_cleanup_then_hide_overlay_restores_focus_and_input() {
    sub_overlay_cleanup_restores_editor();
}

#[test]
fn microtask_deferred_sub_overlay_pattern_restores_focus() {
    sub_overlay_cleanup_restores_editor();
}

static PRIMARY_VISIBLE: AtomicBool = AtomicBool::new(true);

#[test]
fn input_redirection_skips_non_capturing_when_focused_overlay_turns_invisible() {
    let editor = focusable("EDITOR");
    let (fallback, nonc, primary) = (focusable("FALLBACK"), focusable("NC"), focusable("PRIMARY"));
    let (mut tui, _t) = setup(Some(&editor));
    tui.show_overlay(fallback.clone(), None);
    tui.show_overlay(nonc.clone(), nc());
    PRIMARY_VISIBLE.store(true, Ordering::SeqCst);
    tui.show_overlay(
        primary.clone(),
        Some(OverlayOptions {
            visible: Some(|_, _| PRIMARY_VISIBLE.load(Ordering::SeqCst)),
            ..Default::default()
        }),
    );
    assert!(focused(&primary));
    PRIMARY_VISIBLE.store(false, Ordering::SeqCst);
    send(&mut tui, "x");
    flush(&mut tui);
    assert!(inputs(&primary).is_empty());
    assert!(inputs(&nonc).is_empty());
    assert_eq!(inputs(&fallback), ["x"]);
    assert!(focused(&fallback));
    tui.stop();
}

#[test]
fn hide_overlay_keeps_focus_when_topmost_is_non_capturing() {
    let (editor, cap, nonc) = (focusable("EDITOR"), focusable("CAP"), focusable("NC"));
    let (mut tui, _t) = setup(Some(&editor));
    tui.show_overlay(cap.clone(), None);
    tui.show_overlay(nonc, nc());
    assert!(focused(&cap));
    tui.hide_overlay();
    flush(&mut tui);
    assert!(focused(&cap));
    tui.stop();
}

#[test]
fn mixed_overlays_restore_focus_through_removals() {
    let editor = focusable("EDITOR");
    let (c1, n1, c2, n2) = (
        focusable("C1"),
        focusable("N1"),
        focusable("C2"),
        focusable("N2"),
    );
    let (mut tui, _t) = setup(Some(&editor));
    let c1h = tui.show_overlay(c1.clone(), None);
    tui.show_overlay(n1, nc());
    let c2h = tui.show_overlay(c2.clone(), None);
    tui.show_overlay(n2, nc());
    assert!(focused(&c2));
    tui.remove_overlay(c2h);
    flush(&mut tui);
    assert!(focused(&c1));
    tui.remove_overlay(c1h);
    flush(&mut tui);
    assert!(focused(&editor));
    tui.stop();
}

#[test]
fn unfocus_on_topmost_capturing_overlay_falls_back_to_pre_focus() {
    let (editor, cap) = (focusable("EDITOR"), focusable("CAP"));
    let (mut tui, _t) = setup(Some(&editor));
    let h = tui.show_overlay(cap.clone(), None);
    assert!(focused(&cap));
    tui.unfocus_overlay(h);
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!focused(&cap));
    tui.stop();
}

// --- no-op guards

#[test]
fn focus_on_hidden_overlay_is_a_no_op() {
    let (editor, overlay) = (focusable("EDITOR"), focusable("OVERLAY"));
    let (mut tui, _t) = setup(Some(&editor));
    let h = tui.show_overlay(overlay, nc());
    tui.set_overlay_hidden(h, true);
    tui.focus_overlay(h);
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!tui.is_overlay_focused(h));
    tui.stop();
}

#[test]
fn focus_after_hide_is_a_no_op() {
    let (editor, overlay) = (focusable("EDITOR"), focusable("OVERLAY"));
    let (mut tui, _t) = setup(Some(&editor));
    let h = tui.show_overlay(overlay, nc());
    tui.remove_overlay(h);
    tui.focus_overlay(h);
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!tui.is_overlay_focused(h));
    tui.stop();
}

#[test]
fn unfocus_without_focus_is_a_no_op() {
    let (editor, overlay) = (focusable("EDITOR"), focusable("OVERLAY"));
    let (mut tui, _t) = setup(Some(&editor));
    let h = tui.show_overlay(overlay.clone(), nc());
    tui.unfocus_overlay(h);
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!focused(&overlay));
    tui.stop();
}

#[test]
fn unfocus_with_no_pre_focus_clears_focus_and_input_routing() {
    let overlay = focusable("OVERLAY");
    let (mut tui, _t) = setup(None);
    let h = tui.show_overlay(overlay.clone(), None);
    assert!(focused(&overlay));
    tui.unfocus_overlay(h);
    assert!(!focused(&overlay));
    send(&mut tui, "x");
    flush(&mut tui);
    assert!(inputs(&overlay).is_empty());
    assert!(!tui.is_overlay_focused(h));
    tui.stop();
}

// --- focus cycle prevention

#[test]
fn toggling_focus_between_non_capturing_overlays_then_unfocus_returns_to_editor() {
    let (editor, a, b) = (focusable("EDITOR"), focusable("A"), focusable("B"));
    let (mut tui, _t) = setup(Some(&editor));
    let ah = tui.show_overlay(a.clone(), nc());
    let bh = tui.show_overlay(b.clone(), nc());
    tui.focus_overlay(ah);
    tui.focus_overlay(bh);
    tui.focus_overlay(ah);
    tui.unfocus_overlay(ah);
    flush(&mut tui);
    assert!(focused(&editor));
    assert!(!focused(&a));
    assert!(!focused(&b));
    tui.stop();
}

// --- rendering order

fn cell(label: &str, non_capturing: bool) -> (Rc<RefCell<Lines>>, Option<OverlayOptions>) {
    (
        Lines::new(&[label]),
        Some(OverlayOptions {
            row: Some(SizeValue::Absolute(0)),
            col: Some(SizeValue::Absolute(0)),
            width: Some(SizeValue::Absolute(1)),
            non_capturing,
            ..Default::default()
        }),
    )
}

fn show(tui: &mut Tui, label: &str, non_capturing: bool) -> OverlayHandle {
    let (c, o) = cell(label, non_capturing);
    tui.show_overlay(c, o)
}

fn top_left(tui: &mut Tui, term: &Handle) -> char {
    flush(tui);
    term.screen()[0].chars().next().unwrap_or(' ')
}

#[test]
fn focus_on_already_focused_overlay_bumps_visual_order() {
    let editor = focusable("EDITOR");
    let (mut tui, term) = setup_size(20, 6, Some(&editor));
    let a = show(&mut tui, "A", true);
    show(&mut tui, "B", true);
    tui.focus_overlay(a);
    show(&mut tui, "C", true);
    assert_eq!(top_left(&mut tui, &term), 'C');
    tui.focus_overlay(a);
    assert_eq!(top_left(&mut tui, &term), 'A');
    assert!(tui.is_overlay_focused(a));
    tui.stop();
}

#[test]
fn overlapping_overlays_render_in_creation_order() {
    let (mut tui, term) = setup_size(20, 6, None);
    show(&mut tui, "A", true);
    show(&mut tui, "B", true);
    assert_eq!(top_left(&mut tui, &term), 'B');
    tui.stop();
}

#[test]
fn focus_on_lower_overlay_renders_it_on_top() {
    let (mut tui, term) = setup_size(20, 6, None);
    let lower = show(&mut tui, "A", true);
    show(&mut tui, "B", true);
    assert_eq!(top_left(&mut tui, &term), 'B');
    tui.focus_overlay(lower);
    assert_eq!(top_left(&mut tui, &term), 'A');
    tui.stop();
}

#[test]
fn focusing_middle_overlay_puts_it_on_top_keeping_others_order() {
    let (mut tui, term) = setup_size(20, 6, None);
    show(&mut tui, "A", true);
    let middle = show(&mut tui, "B", true);
    let top = show(&mut tui, "C", true);
    assert_eq!(top_left(&mut tui, &term), 'C');
    tui.focus_overlay(middle);
    assert_eq!(top_left(&mut tui, &term), 'B');
    tui.remove_overlay(middle);
    assert_eq!(top_left(&mut tui, &term), 'C');
    tui.remove_overlay(top);
    assert_eq!(top_left(&mut tui, &term), 'A');
    tui.stop();
}

#[test]
fn capturing_overlay_renders_on_top_after_unhide() {
    let (mut tui, term) = setup_size(20, 6, None);
    show(&mut tui, "A", true);
    let capturing = show(&mut tui, "B", false);
    assert_eq!(top_left(&mut tui, &term), 'B');
    tui.set_overlay_hidden(capturing, true);
    show(&mut tui, "C", true);
    assert_eq!(top_left(&mut tui, &term), 'C');
    tui.set_overlay_hidden(capturing, false);
    assert_eq!(top_left(&mut tui, &term), 'B');
    tui.stop();
}

#[test]
fn unfocus_keeps_visual_order_until_another_overlay_is_focused() {
    let editor = focusable("EDITOR");
    let (mut tui, term) = setup_size(20, 6, Some(&editor));
    let a = show(&mut tui, "A", true);
    let b = show(&mut tui, "B", true);
    assert_eq!(top_left(&mut tui, &term), 'B');
    tui.focus_overlay(a);
    assert_eq!(top_left(&mut tui, &term), 'A');
    tui.unfocus_overlay(a);
    assert_eq!(top_left(&mut tui, &term), 'A');
    tui.focus_overlay(b);
    assert_eq!(top_left(&mut tui, &term), 'B');
    tui.stop();
}
