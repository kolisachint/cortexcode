//! Port of the pin's `test/tui-flatcache-stress.test.ts`: the normal
//! differential path must paint exactly what the image path (full flatten +
//! full diff) paints, over 150 seeded random mutations.

use crate::support::*;
use cortexcode_tui_render::{Component, ComponentHandle, Container, Tui, CURSOR_MARKER};
use std::cell::RefCell;
use std::rc::Rc;

const JS_FIRST_THREE: [f64; 3] = [0.021141508361324668, 0.6661099966149777, 0.7799714196007699];

fn mulberry32(seed: u32) -> impl FnMut() -> f64 {
    let mut a = seed as i32;
    move || {
        a = a.wrapping_add(0x6d2b79f5u32 as i32);
        let mut t = (a ^ ((a as u32) >> 15) as i32).wrapping_mul(1 | a);
        t = (t.wrapping_add((t ^ ((t as u32) >> 7) as i32).wrapping_mul(61 | t))) ^ t;
        ((t ^ ((t as u32) >> 14) as i32) as u32) as f64 / 4294967296.0
    }
}

#[test]
fn mulberry32_matches_the_js_sequence() {
    let mut r = mulberry32(0xc0ffee);
    let got: Vec<f64> = (0..3).map(|_| r()).collect();
    assert_eq!(got, JS_FIRST_THREE);
}

/// `Text(text, 0, 0)`: one row per line.
struct Text(String);

impl Component for Text {
    fn render(&mut self, _width: u16) -> Vec<String> {
        self.0.split('\n').map(str::to_string).collect()
    }
}

fn text(s: &str) -> Rc<RefCell<Text>> {
    Rc::new(RefCell::new(Text(s.to_string())))
}

struct App {
    term: Handle,
    tui: Tui,
    chat: Rc<RefCell<Container>>,
    items: Vec<Rc<RefCell<Text>>>,
    spinner: Rc<RefCell<Text>>,
    editor: Rc<RefCell<Text>>,
}

fn build(oracle: bool) -> App {
    let (terminal, term) = virtual_terminal(60, 12);
    let mut tui = Tui::new(terminal, None);
    if oracle {
        tui.force_image_path_for_tests();
    }
    let chat = Rc::new(RefCell::new(Container::new()));
    let items: Vec<_> = (0..3).map(|i| text(&format!("message {i} line"))).collect();
    for t in &items {
        chat.borrow_mut().add_child(t.clone());
    }
    let spinner = text("* working");
    let editor = text(&format!("> input{CURSOR_MARKER}"));
    tui.add_child(chat.clone());
    tui.add_child(spinner.clone());
    tui.add_child(editor.clone());
    let _events = tui.start();
    App {
        term,
        tui,
        chat,
        items,
        spinner,
        editor,
    }
}

#[test]
fn differential_rendering_matches_the_full_diff_path_over_random_mutations() {
    let mut subject = build(false);
    let mut oracle = build(true);
    let mut rand = mulberry32(0xc0ffee);
    let frames = ["*", "+", "x", "o"];
    let mut appended = 0;

    for step in 0..150 {
        let roll = rand();
        let extra = rand();
        for app in [&mut subject, &mut oracle] {
            if roll < 0.3 {
                app.spinner.borrow_mut().0 = format!("{} working {step}", frames[step % 4]);
            } else if roll < 0.55 {
                let t = text(&format!("appended message {appended} with some text"));
                app.items.push(t.clone());
                app.chat.borrow_mut().add_child(t);
            } else if roll < 0.7 {
                let last = app.items.last().unwrap();
                let grown = format!("{}\nmore {step}", last.borrow().0);
                last.borrow_mut().0 = grown;
            } else if roll < 0.8 && app.items.len() > 2 {
                let idx = 1 + (extra * (app.items.len() - 2) as f64).floor() as usize;
                let removed = app.items.remove(idx);
                let handle: ComponentHandle = removed;
                app.chat.borrow_mut().remove_child(&handle);
            } else if roll < 0.92 {
                // Also the TS path for 0.7 <= roll < 0.8 with too few items.
                app.editor.borrow_mut().0 = format!("> input {step}{CURSOR_MARKER} tail");
            } else {
                let idx = (extra * app.items.len() as f64).floor() as usize;
                app.items[idx].borrow_mut().0 = format!("edited {step} message {idx}");
            }
        }
        if roll < 0.55 {
            appended += 1;
        }
        subject.tui.request_render(false);
        oracle.tui.request_render(false);
        let got = subject.term.screen();
        let want = oracle.term.screen();
        assert_eq!(
            got,
            want,
            "viewport mismatch at step {step} (roll={roll:.3})\nsubject:\n{}\n---\noracle:\n{}",
            got.join("\n"),
            want.join("\n")
        );
    }
}
