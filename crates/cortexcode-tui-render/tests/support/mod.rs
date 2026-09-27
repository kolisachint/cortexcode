//! A terminal emulator for renderer tests: the Rust stand-in for the pin's
//! `test/virtual-terminal.ts` (xterm headless), on the `vt100` crate.

#![allow(dead_code)]

use cortexcode_tui_render::Component;
use cortexcode_tui_terminal::Terminal;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct Handle {
    parser: Arc<Mutex<vt100::Parser>>,
    size: Arc<Mutex<(u16, u16)>>,
    pub writes: Arc<Mutex<Vec<String>>>,
    pub mouse: bool,
}

pub struct VirtualTerminal {
    handle: Handle,
}

pub fn virtual_terminal(cols: u16, rows: u16) -> (Box<dyn Terminal>, Handle) {
    let handle = Handle {
        parser: Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10_000))),
        size: Arc::new(Mutex::new((cols, rows))),
        writes: Arc::default(),
        mouse: true,
    };
    (
        Box::new(VirtualTerminal {
            handle: handle.clone(),
        }),
        handle,
    )
}

impl Handle {
    fn feed(&self, data: &str) {
        self.parser.lock().unwrap().process(data.as_bytes());
    }

    /// The visible rows, trailing spaces trimmed (`getViewport` + trimEnd).
    pub fn screen(&self) -> Vec<String> {
        let parser = self.parser.lock().unwrap();
        let (cols, _) = *self.size.lock().unwrap();
        parser
            .screen()
            .rows(0, cols)
            .map(|r| r.trim_end().to_string())
            .collect()
    }

    /// Scrollback plus screen, oldest first (`getScrollBuffer`).
    pub fn scroll_buffer(&self) -> Vec<String> {
        let mut parser = self.parser.lock().unwrap();
        let (cols, rows) = *self.size.lock().unwrap();
        let mut out = Vec::new();
        // vt100 exposes scrollback by moving the view offset.
        parser.screen_mut().set_scrollback(usize::MAX);
        let depth = parser.screen().scrollback();
        for offset in (1..=depth).rev() {
            parser.screen_mut().set_scrollback(offset);
            if let Some(first) = parser.screen().rows(0, cols).next() {
                out.push(first.trim_end().to_string());
            }
        }
        parser.screen_mut().set_scrollback(0);
        out.extend(
            parser
                .screen()
                .rows(0, cols)
                .take(rows as usize)
                .map(|r| r.trim_end().to_string()),
        );
        out
    }

    /// `(row, col)` of the cursor.
    pub fn cursor(&self) -> (u16, u16) {
        self.parser.lock().unwrap().screen().cursor_position()
    }

    pub fn alternate_screen(&self) -> bool {
        self.parser.lock().unwrap().screen().alternate_screen()
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        *self.size.lock().unwrap() = (cols, rows);
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(rows, cols);
    }

    pub fn clear_writes(&self) {
        self.writes.lock().unwrap().clear();
    }

    pub fn joined_writes(&self) -> String {
        self.writes.lock().unwrap().concat()
    }

    pub fn last_write(&self) -> String {
        self.writes
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }
}

impl Terminal for VirtualTerminal {
    fn start(
        &mut self,
        _on_input: Box<dyn FnMut(&str) + Send>,
        _on_resize: Box<dyn FnMut() + Send>,
    ) {
        self.handle.feed("\x1b[?2004h");
    }
    fn stop(&mut self) {
        self.handle.feed("\x1b[?2004l");
    }
    fn drain_input(&mut self, _max: Duration, _idle: Duration) {}
    fn write(&mut self, data: &str) {
        self.handle.writes.lock().unwrap().push(data.to_string());
        self.handle.feed(data);
    }
    fn columns(&self) -> u16 {
        self.handle.size.lock().unwrap().0
    }
    fn rows(&self) -> u16 {
        self.handle.size.lock().unwrap().1
    }
    fn kitty_protocol_active(&self) -> bool {
        true
    }
    fn move_by(&mut self, lines: i32) {
        if lines > 0 {
            self.handle.feed(&format!("\x1b[{lines}B"));
        } else if lines < 0 {
            self.handle.feed(&format!("\x1b[{}A", -lines));
        }
    }
    fn hide_cursor(&mut self) {
        self.handle.feed("\x1b[?25l");
    }
    fn show_cursor(&mut self) {
        self.handle.feed("\x1b[?25h");
    }
    fn clear_line(&mut self) {
        self.handle.feed("\x1b[K");
    }
    fn clear_from_cursor(&mut self) {
        self.handle.feed("\x1b[J");
    }
    fn clear_screen(&mut self) {
        self.handle.feed("\x1b[2J\x1b[H");
    }
    fn set_title(&mut self, _title: &str) {}
    fn set_progress(&mut self, _active: bool) {}
    fn mouse_reporting(&self) -> bool {
        self.handle.mouse
    }
    fn set_alternate_screen(&mut self, active: bool) {
        self.handle
            .feed(if active { "\x1b[?1049h" } else { "\x1b[?1049l" });
    }
}

/// A component whose lines the test drives.
pub struct Lines(pub Vec<String>);

impl Lines {
    pub fn new(lines: &[&str]) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self(
            lines.iter().map(|s| s.to_string()).collect(),
        )))
    }

    /// `body 1` .. `body n`.
    pub fn body(count: usize) -> Rc<RefCell<Self>> {
        let this = Self::new(&[]);
        this.borrow_mut().set_count(count);
        this
    }

    pub fn set_count(&mut self, count: usize) {
        self.0 = (1..=count).map(|i| format!("body {i}")).collect();
    }
}

impl Component for Lines {
    fn render(&mut self, _width: u16) -> Vec<String> {
        self.0.clone()
    }
}
