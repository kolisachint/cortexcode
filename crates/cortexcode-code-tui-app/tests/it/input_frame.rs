//! The `InputFrame` cases of the pin's `test/input-surface-frame.test.ts`
//! (the surface-by-surface cases arrive with the pickers in 11.3).

use std::cell::RefCell;
use std::rc::Rc;

use crate::support::lock;
use cortexcode_code_tui_app::input_frame::*;
use cortexcode_tui_components::{FrameBorderStyle, Text};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{strip_vt_control_characters, visible_width};

fn strip(lines: Vec<String>) -> Vec<String> {
    lines
        .iter()
        .map(|l| strip_vt_control_characters(l))
        .collect()
}

fn body(text: &str) -> Rc<RefCell<Text>> {
    Rc::new(RefCell::new(Text::new(text, 0, 0)))
}

fn frame(title: &str) -> InputFrame {
    set_input_frame_border(FrameBorderStyle::Box);
    InputFrame::new(InputFrameOptions {
        title: Some(title.into()),
        padding_x: None,
    })
}

#[test]
fn keeps_the_hints_as_the_last_row_whatever_order_the_pane_was_built_in() {
    let _g = lock(Some("dark"));
    let mut f = frame("demo");
    f.set_hint("enter submit");
    f.add_child(body("a body row"));
    let lines = strip(f.render(60));
    assert!(lines[1].contains("a body row"));
    assert!(lines[2].contains("enter submit"));
}

#[test]
fn collapses_a_multi_line_title_which_the_border_cannot_hold() {
    let _g = lock(Some("dark"));
    let mut f = frame("Overwrite file?\nThis cannot be undone.");
    f.add_child(body("a body row"));
    let lines = f.render(100);
    assert!(lines.iter().all(|l| !l.contains('\n')));
    assert!(strip(lines)[0].contains("Overwrite file? This cannot be undone."));
}

#[test]
fn drops_a_title_inside_the_frame_rather_than_losing_it_when_the_border_is_too_narrow() {
    let _g = lock(Some("dark"));
    let long = "Overwrite file? This cannot be undone.";
    let mut f = frame(long);
    f.add_child(body("a body row"));
    let wide = strip(f.render(100));
    assert!(wide[0].contains(long));
    assert!(!wide[1..wide.len() - 1].join("\n").contains(long));

    let narrow_styled = f.render(30);
    let narrow = strip(narrow_styled.clone());
    assert!(!narrow[0].contains("Overwrite"));
    assert!(narrow[1..narrow.len() - 1]
        .join(" ")
        .contains("Overwrite file?"));
    for line in &narrow_styled {
        assert_eq!(visible_width(line), 30);
    }
    assert!(strip(f.render(100))[0].contains(long));
}

#[test]
fn replaces_the_hints_rather_than_stacking_them() {
    let _g = lock(Some("dark"));
    let mut f = frame("demo");
    f.set_hint("first");
    f.set_hint("second");
    let lines = strip(f.render(60));
    assert!(!lines.iter().any(|l| l.contains("first")));
    assert_eq!(lines.iter().filter(|l| l.contains("second")).count(), 1);
}

#[test]
fn follows_the_border_setting_on_a_pane_that_is_already_open() {
    let _g = lock(Some("dark"));
    let mut f = frame("demo");
    f.add_child(body("row"));
    assert!(strip(f.render(40))[0].starts_with('┌'));
    set_input_frame_border(FrameBorderStyle::Rule);
    assert!(strip(f.render(40))[0].starts_with('─'));
    set_input_frame_border(FrameBorderStyle::Box);
}
