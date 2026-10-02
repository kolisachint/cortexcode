//! Port of the pin's `test/tui-cell-size-input.test.ts`. The capability and
//! cell-size caches are process globals, so this binary's tests serialize.

use crate::support::*;
use cortexcode_tui_images::{
    get_cell_dimensions, reset_capabilities_cache, set_capabilities, set_cell_dimensions,
    CellDimensions, ImageProtocol, TerminalCapabilities,
};
use cortexcode_tui_render::{Component, Tui, TuiEvent};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;

static GLOBALS: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct InputRecorder(Vec<String>);

impl Component for InputRecorder {
    fn render(&mut self, _width: u16) -> Vec<String> {
        vec![String::new()]
    }
    fn handle_input(&mut self, data: &str) {
        self.0.push(data.to_string());
    }
}

/// TERM_PROGRAM=ghostty in the TS test: a terminal with kitty images.
fn with_image_terminal(f: impl FnOnce()) {
    let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
    set_capabilities(TerminalCapabilities {
        images: Some(ImageProtocol::Kitty),
        true_color: true,
        hyperlinks: true,
    });
    f();
    reset_capabilities_cache();
}

fn setup() -> (Tui, Handle, Rc<RefCell<InputRecorder>>) {
    let (terminal, term) = virtual_terminal(80, 24);
    let mut tui = Tui::new(terminal, None);
    let recorder = Rc::new(RefCell::new(InputRecorder::default()));
    tui.set_focus(Some(recorder.clone()));
    let _events = tui.start();
    (tui, term, recorder)
}

#[test]
fn forwards_bare_escape_even_when_a_cell_size_query_was_sent() {
    with_image_terminal(|| {
        let (mut tui, term, recorder) = setup();
        assert!(term.joined_writes().contains("\x1b[16t"));
        tui.process_event(TuiEvent::Input("\x1b".into()));
        assert_eq!(recorder.borrow().0, ["\x1b"]);
        tui.stop();
    });
}

#[test]
fn consumes_cell_size_responses_and_still_forwards_later_input() {
    with_image_terminal(|| {
        set_cell_dimensions(CellDimensions {
            width_px: 9,
            height_px: 18,
        });
        let (mut tui, _term, recorder) = setup();
        tui.process_event(TuiEvent::Input("\x1b[6;20;10t".into()));
        assert!(recorder.borrow().0.is_empty());
        assert_eq!(
            get_cell_dimensions(),
            CellDimensions {
                width_px: 10,
                height_px: 20
            }
        );
        tui.process_event(TuiEvent::Input("q".into()));
        assert_eq!(recorder.borrow().0, ["q"]);
        tui.stop();
    });
}
