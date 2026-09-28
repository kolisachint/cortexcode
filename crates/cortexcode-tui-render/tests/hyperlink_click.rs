//! The click half of the pin's `test/hyperlink-click.test.ts`.

mod support;

use cortexcode_tui_render::{InputListenerResult, Tui, TuiEvent};
use std::cell::RefCell;
use std::rc::Rc;
use support::*;

const HEIGHT: usize = 10;

fn link(url: &str, label: &str) -> String {
    format!("\x1b]8;;{url}\x07{label}\x1b]8;;\x07")
}

fn page(link_row: usize) -> Vec<String> {
    (0..HEIGHT)
        .map(|i| {
            if i == link_row {
                format!("go {}", link("https://example.com", "here"))
            } else {
                format!("row {i}")
            }
        })
        .collect()
}

fn setup_rows(rows: Vec<String>) -> (Tui, Rc<RefCell<Vec<String>>>) {
    let (terminal, _term) = virtual_terminal(40, HEIGHT as u16);
    let mut tui = Tui::new(terminal, None);
    let opened = Rc::new(RefCell::new(Vec::new()));
    let sink = opened.clone();
    tui.on_hyperlink = Some(Box::new(move |url: &str| {
        sink.borrow_mut().push(url.to_string())
    }));
    tui.add_child(Rc::new(RefCell::new(Lines(rows))));
    let _events = tui.start();
    (tui, opened)
}

fn send(tui: &mut Tui, data: &str) {
    tui.process_event(TuiEvent::Input(data.to_string()));
}

fn click(tui: &mut Tui, row: usize, column: usize) {
    send(tui, &format!("\x1b[<0;{column};{row}M"));
    send(tui, &format!("\x1b[<0;{column};{row}m"));
}

#[test]
fn opens_the_url_under_the_pointer() {
    let (mut tui, opened) = setup_rows(page(4));
    click(&mut tui, 5, 5);
    assert_eq!(*opened.borrow(), ["https://example.com"]);
}

#[test]
fn opens_a_url_printed_as_plain_text() {
    let rows = (0..HEIGHT)
        .map(|i| {
            if i == 3 {
                "at http://localhost:8080/x.".to_string()
            } else {
                format!("row {i}")
            }
        })
        .collect();
    let (mut tui, opened) = setup_rows(rows);
    click(&mut tui, 4, 2);
    click(&mut tui, 4, 10);
    assert_eq!(*opened.borrow(), ["http://localhost:8080/x"]);
}

#[test]
fn nothing_beside_the_link_no_drag_no_right_button() {
    let (mut tui, opened) = setup_rows(page(4));
    click(&mut tui, 5, 1);
    click(&mut tui, 4, 5);
    send(&mut tui, "\x1b[<0;5;5M");
    send(&mut tui, "\x1b[<0;8;5m");
    send(&mut tui, "\x1b[<2;5;5M");
    send(&mut tui, "\x1b[<2;5;5m");
    assert!(opened.borrow().is_empty());
}

#[test]
fn leaves_nothing_of_the_report_in_the_input_stream() {
    let (terminal, _term) = virtual_terminal(40, HEIGHT as u16);
    let mut tui = Tui::new(terminal, None);
    let typed = Rc::new(RefCell::new(Vec::<String>::new()));
    let sink = typed.clone();
    tui.add_child(Lines::new(&["x"]));
    tui.add_input_listener(Box::new(move |data: &str| -> Option<InputListenerResult> {
        sink.borrow_mut().push(data.to_string());
        None
    }));
    let _events = tui.start();
    click(&mut tui, 5, 5);
    assert!(typed.borrow().is_empty());
}
