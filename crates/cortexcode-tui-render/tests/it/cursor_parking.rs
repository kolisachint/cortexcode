//! Port of the pin's `test/cursor-parking.test.ts`: the cursor move is part
//! of the frame, inside its synchronized-output block.

use crate::support::*;
use cortexcode_tui_render::{Tui, CURSOR_MARKER};
use std::cell::RefCell;
use std::rc::Rc;

const SYNC_END: &str = "\x1b[?2026l";
const WIDTH: usize = 40;

fn padded(text: &str) -> String {
    let visible = text.replace(CURSOR_MARKER, "");
    format!(
        "{text}{}",
        " ".repeat(WIDTH.saturating_sub(visible.chars().count()))
    )
}

fn setup(show_cursor: bool, with_editor: bool) -> (Tui, Handle, Rc<RefCell<Lines>>) {
    let (terminal, term) = virtual_terminal(WIDTH as u16, 10);
    let mut tui = Tui::new(terminal, Some(show_cursor));
    let status = Rc::new(RefCell::new(Lines(vec![padded("O Working...")])));
    tui.add_child(status.clone());
    if with_editor {
        tui.add_child(Rc::new(RefCell::new(Lines(vec![padded(&format!(
            "> hi{CURSOR_MARKER}"
        ))]))));
    }
    let _events = tui.start();
    (tui, term, status)
}

#[test]
fn moves_the_cursor_inside_the_synchronized_block() {
    let (mut tui, term, status) = setup(true, true);
    term.clear_writes();
    status.borrow_mut().0 = vec![padded("* Working...")];
    tui.request_render(false);
    let frames: Vec<String> = term
        .writes
        .lock()
        .unwrap()
        .iter()
        .filter(|w| !w.is_empty())
        .cloned()
        .collect();
    assert_eq!(frames.len(), 1, "{frames:?}");
    let frame = &frames[0];
    let sync_end = frame.find(SYNC_END).expect("synchronized");
    assert!(frame[sync_end + SYNC_END.len()..].trim().is_empty());
    assert!(frame[..sync_end].contains("\x1b[5G"));
}

#[test]
fn leaves_the_cursor_on_the_focused_line() {
    let (mut tui, term, status) = setup(true, true);
    for f in ["* Working...", "O Working...", "* Working..."] {
        status.borrow_mut().0 = vec![padded(f)];
        tui.request_render(false);
        assert_eq!(term.cursor(), (1, 4), "after {f:?}");
    }
}

#[test]
fn parks_the_cursor_at_column_zero_when_nothing_is_focused() {
    let (mut tui, term, status) = setup(false, false);
    status.borrow_mut().0 = vec![padded("* Working...")];
    tui.request_render(false);
    assert_eq!(term.cursor(), (0, 0));
}
