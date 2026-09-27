//! Port of the pin's `test/paper-sheet.test.ts`.

use cortexcode_tui_components::{BoxComponent, PaperSheet, Text};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;
use std::cell::RefCell;
use std::rc::Rc;

const INSET: usize = 3;
const WIDTH: u16 = 40;

fn boxed(inset: Option<usize>) -> BoxComponent {
    let mut b = BoxComponent::new(1, 1, Some(Box::new(|t: &str| format!("<b>{t}</b>"))));
    b.set_paper(Some(Box::new(move || {
        Some(PaperSheet {
            shadow: Some(Box::new(|t: &str| format!("<s>{t}</s>"))),
            inset,
        })
    })));
    b
}

fn sheet(rows: usize) -> Vec<String> {
    let mut b = boxed(Some(INSET));
    for i in 0..rows {
        b.add_child(Rc::new(RefCell::new(Text::new(format!("row {i}"), 0, 0))));
    }
    b.render(WIDTH)
}

fn plain(line: &str) -> String {
    line.replace("<b>", "")
        .replace("</b>", "")
        .replace("<s>", "")
        .replace("</s>", "")
}

fn shadow_segment(line: &str) -> &str {
    match line.rfind("</b>") {
        Some(end) => &line[end + 4..],
        None => line,
    }
}

fn char_index(s: &str, needle: char) -> usize {
    s.chars().position(|c| c == needle).unwrap()
}

#[test]
fn closes_the_bottom_corner_flush_with_its_column() {
    let lines = sheet(3);
    let run = &lines[lines.len() - 1];
    assert!(!run.contains('▏'));
    assert_eq!(
        visible_width(&plain(run)),
        char_index(&plain(&lines[lines.len() - 2]), '▏')
    );
}

#[test]
fn draws_one_hairline_beside_the_sheet() {
    let lines = sheet(24);
    for line in &lines[1..lines.len() - 1] {
        assert_eq!(shadow_segment(line), "<s>▏</s>");
    }
}

#[test]
fn leaves_the_top_right_corner_whole() {
    let lines = sheet(24);
    assert_eq!(visible_width(&plain(&lines[0])), WIDTH as usize - INSET);
    assert_eq!(shadow_segment(&lines[0]), "");
}

#[test]
fn holds_every_shadowed_row_to_the_same_width() {
    let lines = sheet(24);
    let edge = WIDTH as usize - INSET + 1;
    for line in &lines[1..lines.len() - 1] {
        assert_eq!(visible_width(&plain(line)), edge);
    }
    assert_eq!(visible_width(&plain(&lines[lines.len() - 1])), edge - 1);
}

#[test]
fn gives_the_treatment_up_when_the_band_is_too_narrow() {
    for width in 1..=4u16 {
        let mut b = boxed(Some(INSET));
        b.add_child(Rc::new(RefCell::new(Text::new("hello there", 0, 0))));
        for line in b.render(width) {
            assert!(
                visible_width(&plain(&line)) <= width as usize,
                "@{width}: {line:?}"
            );
            assert!(!line.contains(['▏', '▔']), "@{width}");
        }
    }
}

#[test]
fn keeps_every_row_inside_the_terminal() {
    for width in [5u16, 6, 8, 12, 20, 40, 120] {
        let mut b = boxed(Some(INSET));
        for i in 0..12 {
            b.add_child(Rc::new(RefCell::new(Text::new(
                format!("a row of text {i}"),
                0,
                0,
            ))));
        }
        for line in b.render(width) {
            assert!(
                visible_width(&plain(&line)) <= width as usize,
                "@{width}: {line:?}"
            );
        }
    }
}

#[test]
fn stops_one_cell_short_of_the_margin_without_a_gutter() {
    let mut b = boxed(None);
    b.add_child(Rc::new(RefCell::new(Text::new("hello", 0, 0))));
    let lines = b.render(WIDTH);
    let run = &lines[lines.len() - 1];
    assert!(!run.contains('▏'));
    assert_eq!(run.matches('▔').count(), WIDTH as usize - 1);
}
