//! Case-for-case port of the pin's `test/tui-render.test.ts`.
//!
//! `TERMUX_VERSION` is a process global, so the resize tests hold `ENV`.

mod support;

use cortexcode_tui_images::{delete_kitty_image, encode_kitty, KittyEncodeOptions};
use cortexcode_tui_render::{Tui, TuiEvent};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;
use support::*;

static ENV: Mutex<()> = Mutex::new(());

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn numbered(prefix: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{prefix} {i}")).collect()
}

fn setup(cols: u16, rows: u16, initial: Vec<String>) -> (Tui, Handle, Rc<RefCell<Lines>>) {
    let (terminal, term) = virtual_terminal(cols, rows);
    let mut tui = Tui::new(terminal, None);
    let component = Rc::new(RefCell::new(Lines(initial)));
    tui.add_child(component.clone());
    let _events = tui.start();
    (tui, term, component)
}

fn set(tui: &mut Tui, c: &Rc<RefCell<Lines>>, new: Vec<String>) {
    c.borrow_mut().0 = new;
    tui.request_render(false);
}

fn resize(tui: &mut Tui, term: &Handle, cols: u16, rows: u16) {
    term.resize(cols, rows);
    tui.process_event(TuiEvent::Resize);
}

fn kitty(data: &str, rows: u32, id: u32) -> String {
    encode_kitty(
        data,
        &KittyEncodeOptions {
            columns: Some(2),
            rows: Some(rows),
            image_id: Some(id),
            move_cursor: Some(false),
        },
    )
}

// --- Kitty image cleanup

#[test]
fn deletes_changed_image_ids_before_drawing_moved_placements() {
    let old = kitty("AAAA", 2, 42);
    let (mut tui, term, c) = setup(40, 10, vec!["top".into(), old]);
    term.clear_writes();
    let new = kitty("BBBB", 1, 42);
    set(&mut tui, &c, vec![new.clone(), String::new()]);
    let writes = term.joined_writes();
    let del = writes
        .find(&delete_kitty_image(42))
        .expect("old image deleted");
    let draw = writes.find(&new).expect("new image drawn");
    assert!(del < draw);
    tui.stop();
}

#[test]
fn redraws_image_lines_when_an_earlier_reserved_row_changes() {
    let image = kitty("AAAA", 2, 88);
    let (mut tui, term, c) = setup(40, 10, vec![String::new(), image.clone()]);
    term.clear_writes();
    set(&mut tui, &c, vec!["covered".into(), image.clone()]);
    let writes = term.joined_writes();
    let del = writes.find(&delete_kitty_image(88)).expect("image deleted");
    let draw = writes.find(&image).expect("image redrawn");
    assert!(del < draw);
    assert!(!writes.contains("\x1b[2J"), "no full redraw");
    tui.stop();
}

#[test]
fn deletes_previously_rendered_image_ids_during_full_redraws() {
    let (mut tui, term, c) = setup(40, 10, vec![kitty("AAAA", 2, 77)]);
    term.clear_writes();
    c.borrow_mut().0 = lines(&["plain text"]);
    tui.request_render(true);
    let writes = term.joined_writes();
    let del = writes.find(&delete_kitty_image(77)).expect("image deleted");
    let clear = writes.find("\x1b[2J").expect("screen cleared");
    assert!(del < clear);
    tui.stop();
}

// --- resize handling

#[test]
fn height_change_triggers_full_rerender() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("TERMUX_VERSION");
    let (mut tui, term, _c) = setup(40, 10, lines(&["Line 0", "Line 1", "Line 2"]));
    let before = tui.full_redraws();
    resize(&mut tui, &term, 40, 15);
    assert!(tui.full_redraws() > before);
    assert!(term.screen()[0].contains("Line 0"));
    tui.stop();
}

#[test]
fn termux_skips_full_rerender_on_height_changes() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("TERMUX_VERSION", "1");
    let (mut tui, term, _c) = setup(40, 10, numbered("Line", 20));
    term.clear_writes();
    let before = tui.full_redraws();
    for h in [15, 8, 14, 11] {
        resize(&mut tui, &term, 40, h);
    }
    std::env::remove_var("TERMUX_VERSION");
    assert_eq!(tui.full_redraws(), before);
    let writes = term.joined_writes();
    assert!(!writes.contains("\x1b[2J"));
    assert!(!writes.contains("\x1b[3J"));
    assert!(
        term.screen().join("\n").contains("Line 19"),
        "{:?}",
        term.screen()
    );
    tui.stop();
}

#[test]
fn width_change_triggers_full_rerender() {
    let (mut tui, term, _c) = setup(40, 10, lines(&["Line 0", "Line 1", "Line 2"]));
    let before = tui.full_redraws();
    resize(&mut tui, &term, 60, 10);
    assert!(tui.full_redraws() > before);
    tui.stop();
}

// --- content shrinkage

fn setup_clear_on_shrink(initial: Vec<String>) -> (Tui, Handle, Rc<RefCell<Lines>>) {
    let (terminal, term) = virtual_terminal(40, 10);
    let mut tui = Tui::new(terminal, None);
    tui.set_clear_on_shrink(true);
    let component = Rc::new(RefCell::new(Lines(initial)));
    tui.add_child(component.clone());
    let _events = tui.start();
    (tui, term, component)
}

#[test]
fn clears_empty_rows_when_content_shrinks_significantly() {
    let (mut tui, term, c) = setup_clear_on_shrink(numbered("Line", 6));
    let before = tui.full_redraws();
    set(&mut tui, &c, lines(&["Line 0", "Line 1"]));
    assert!(tui.full_redraws() > before);
    let s = term.screen();
    assert!(s[0].contains("Line 0") && s[1].contains("Line 1"));
    assert_eq!(s[2].trim(), "");
    assert_eq!(s[3].trim(), "");
    tui.stop();
}

#[test]
fn handles_shrink_to_single_line() {
    let (mut tui, term, c) = setup_clear_on_shrink(numbered("Line", 4));
    set(&mut tui, &c, lines(&["Only line"]));
    let s = term.screen();
    assert!(s[0].contains("Only line"));
    assert_eq!(s[1].trim(), "");
    tui.stop();
}

#[test]
fn handles_shrink_to_empty() {
    let (mut tui, term, c) = setup_clear_on_shrink(numbered("Line", 3));
    set(&mut tui, &c, vec![]);
    let s = term.screen();
    assert_eq!(s[0].trim(), "");
    assert_eq!(s[1].trim(), "");
    tui.stop();
}

// --- differential rendering

#[test]
fn tracks_cursor_when_content_shrinks_with_unchanged_remaining_lines() {
    let (mut tui, term, c) = setup(40, 10, numbered("Line", 5));
    set(&mut tui, &c, numbered("Line", 3));
    set(&mut tui, &c, lines(&["Line 0", "CHANGED", "Line 2"]));
    assert!(term.screen()[1].contains("CHANGED"), "{:?}", term.screen());
    tui.stop();
}

#[test]
fn renders_middle_line_changes_spinner_case() {
    let (mut tui, term, c) = setup(40, 10, lines(&["Header", "Working...", "Footer"]));
    for frame in ["|", "/", "-", "\\"] {
        set(
            &mut tui,
            &c,
            lines(&["Header", &format!("Working {frame}"), "Footer"]),
        );
        let s = term.screen();
        assert!(s[0].contains("Header"));
        assert!(s[1].contains(&format!("Working {frame}")));
        assert!(s[2].contains("Footer"));
    }
    tui.stop();
}

#[test]
fn resets_styles_after_each_rendered_line() {
    let (mut tui, term, _c) = setup(20, 6, lines(&["\x1b[3mItalic", "Plain"]));
    assert!(!term.cell_italic(1, 0));
    tui.stop();
}

#[test]
fn renders_first_line_change() {
    let (mut tui, term, c) = setup(40, 10, numbered("Line", 4));
    set(
        &mut tui,
        &c,
        lines(&["CHANGED", "Line 1", "Line 2", "Line 3"]),
    );
    assert_eq!(
        term.screen()[..4],
        ["CHANGED", "Line 1", "Line 2", "Line 3"]
    );
    tui.stop();
}

#[test]
fn renders_last_line_change() {
    let (mut tui, term, c) = setup(40, 10, numbered("Line", 4));
    set(
        &mut tui,
        &c,
        lines(&["Line 0", "Line 1", "Line 2", "CHANGED"]),
    );
    assert_eq!(
        term.screen()[..4],
        ["Line 0", "Line 1", "Line 2", "CHANGED"]
    );
    tui.stop();
}

#[test]
fn renders_multiple_non_adjacent_line_changes() {
    let (mut tui, term, c) = setup(40, 10, numbered("Line", 5));
    set(
        &mut tui,
        &c,
        lines(&["Line 0", "CHANGED 1", "Line 2", "CHANGED 3", "Line 4"]),
    );
    assert_eq!(
        term.screen()[..5],
        ["Line 0", "CHANGED 1", "Line 2", "CHANGED 3", "Line 4"]
    );
    tui.stop();
}

#[test]
fn handles_content_to_empty_and_back() {
    let (mut tui, term, c) = setup(40, 10, numbered("Line", 3));
    assert!(term.screen()[0].contains("Line 0"));
    set(&mut tui, &c, vec![]);
    set(&mut tui, &c, lines(&["New Line 0", "New Line 1"]));
    let s = term.screen();
    assert!(s[0].contains("New Line 0"), "{s:?}");
    assert!(s[1].contains("New Line 1"), "{s:?}");
    tui.stop();
}

#[test]
fn full_rerenders_when_deleted_lines_move_the_viewport_upward() {
    let (mut tui, term, c) = setup(20, 5, numbered("Line", 12));
    let before = tui.full_redraws();
    set(&mut tui, &c, numbered("Line", 7));
    assert!(tui.full_redraws() > before);
    assert_eq!(
        term.screen(),
        ["Line 2", "Line 3", "Line 4", "Line 5", "Line 6"]
    );
    tui.stop();
}

#[test]
fn appends_after_a_shrink_without_another_full_redraw() {
    let (mut tui, term, c) = setup(20, 5, numbered("Line", 8));
    let before = tui.full_redraws();
    set(&mut tui, &c, numbered("Line", 2));
    assert!(tui.full_redraws() > before);
    let after_shrink = tui.full_redraws();
    set(&mut tui, &c, numbered("Line", 3));
    assert_eq!(tui.full_redraws(), after_shrink);
    assert_eq!(term.screen(), ["Line 0", "Line 1", "Line 2", "", ""]);
    tui.stop();
}

#[test]
fn clears_stale_content_after_a_transient_component_inflated_max_lines() {
    let (terminal, term) = virtual_terminal(40, 10);
    let mut tui = Tui::new(terminal, None);
    let chat = Rc::new(RefCell::new(Lines(numbered("Chat", 15))));
    let editor_lines = lines(&["Editor 0", "Editor 1", "Editor 2"]);
    let editor = Rc::new(RefCell::new(Lines(editor_lines.clone())));
    tui.add_child(chat.clone());
    tui.add_child(editor.clone());
    let _events = tui.start();

    set(&mut tui, &editor, numbered("Selector", 8));
    set(&mut tui, &editor, editor_lines);
    let before = tui.full_redraws();
    set(&mut tui, &chat, numbered("Chat", 12));
    assert!(tui.full_redraws() > before);

    let s = term.screen();
    for (i, line) in s.iter().enumerate() {
        for stale in ["Chat 12", "Chat 13", "Chat 14"] {
            assert!(!line.contains(stale), "stale {stale:?} at row {i}");
        }
    }
    assert_eq!(
        s,
        [
            "Chat 5", "Chat 6", "Chat 7", "Chat 8", "Chat 9", "Chat 10", "Chat 11", "Editor 0",
            "Editor 1", "Editor 2"
        ]
    );
    tui.stop();
}
