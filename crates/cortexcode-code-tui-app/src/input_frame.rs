//! The frame every surface that asks the user for something draws
//! (`components/input-frame.ts`): the prompt's border style, the title in the
//! top border (or on the first row when the border has no room), and the key
//! hints as the last row inside.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cortexcode_code_tui_theme::theme;
use cortexcode_tui_components::{Frame, FrameBorderStyle, FrameLabel, FrameOptions, Text};
use cortexcode_tui_render::{Component, ComponentHandle};

thread_local! {
    /// The border style the prompt draws, so its stand-ins draw the same one.
    static INPUT_BORDER_STYLE: Cell<FrameBorderStyle> = const { Cell::new(FrameBorderStyle::Box) };
}

/// Follow the user's `editorBorder` setting (interactive mode is the caller).
pub fn set_input_frame_border(style: FrameBorderStyle) {
    INPUT_BORDER_STYLE.with(|s| s.set(style));
}

pub fn get_input_frame_border() -> FrameBorderStyle {
    INPUT_BORDER_STYLE.with(Cell::get)
}

/// `InputFrameOptions`.
#[derive(Default)]
pub struct InputFrameOptions {
    pub title: Option<String>,
    pub padding_x: Option<usize>,
}

/// A `Frame` in the app's border colour and the current border style,
/// resolved per render so theme switches and settings edits repaint it.
pub struct InputFrame {
    frame: Frame,
    hint_row: Option<Rc<RefCell<Text>>>,
    title_text: String,
    title_row: Option<Rc<RefCell<Text>>>,
}

fn handle(text: &Rc<RefCell<Text>>) -> ComponentHandle {
    text.clone()
}

impl InputFrame {
    pub fn new(options: InputFrameOptions) -> Self {
        let mut frame = Self {
            frame: Frame::new(FrameOptions {
                border: Some(get_input_frame_border()),
                padding_x: Some(options.padding_x.unwrap_or(1)),
                border_chars: None,
                color: Some(Box::new(|s: &str| theme().fg("border", s))),
            }),
            hint_row: None,
            title_text: String::new(),
            title_row: None,
        };
        if let Some(title) = options.title {
            frame.set_title(&title);
        }
        frame
    }

    /// Name this surface, collapsed to one line (the border is one line).
    pub fn set_title(&mut self, title: &str) {
        self.title_text = title.split_whitespace().collect::<Vec<_>>().join(" ");
    }

    fn lay_out_title(&mut self, width: usize) {
        let plain = if self.title_text.is_empty() {
            String::new()
        } else {
            format!(" {} ", self.title_text)
        };
        let in_border = self.frame.label_fits(&plain, width);
        self.frame.set_label(in_border.then(|| {
            let t = theme();
            FrameLabel {
                styled: t.fg("accent", &t.bold(&plain)),
                plain: plain.clone(),
            }
        }));
        if in_border || self.title_text.is_empty() {
            if let Some(row) = self.title_row.take() {
                self.frame.remove_child(&handle(&row));
            }
            return;
        }
        let row = match &self.title_row {
            Some(row) => row.clone(),
            None => {
                let row = Rc::new(RefCell::new(Text::new("", 0, 0)));
                self.frame.container.children.insert(0, handle(&row));
                self.title_row = Some(row.clone());
                row
            }
        };
        let t = theme();
        row.borrow_mut()
            .set_text(t.fg("accent", &t.bold(&self.title_text)));
    }

    /// The key hints, always the last row inside.
    pub fn set_hint(&mut self, hint: &str) {
        if let Some(row) = &self.hint_row {
            row.borrow_mut().set_text(hint);
            return;
        }
        let row = Rc::new(RefCell::new(Text::new(hint, 0, 0)));
        self.frame.add_child(handle(&row));
        self.hint_row = Some(row);
    }

    /// Add a row above the hints.
    pub fn add_child(&mut self, component: ComponentHandle) {
        match self.hint_row.clone() {
            None => self.frame.add_child(component),
            Some(hint) => {
                self.frame.remove_child(&handle(&hint));
                self.frame.add_child(component);
                self.frame.add_child(handle(&hint));
            }
        }
    }

    pub fn remove_child(&mut self, component: &ComponentHandle) {
        self.frame.remove_child(component);
    }
}

impl Component for InputFrame {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.set_border(get_input_frame_border());
        self.lay_out_title(width as usize);
        self.frame.render(width)
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }
}
