//! `components/extension-editor.ts`: a multi-line editor in the prompt's
//! frame, for dialogs that ask for free text (the custom branch-summary
//! instructions, extensions' `ctx.ui.editor`).
//!
//! Not ported: the `app.editor.external` hand-off to `$VISUAL`/`$EDITOR`
//! (the main prompt does not have it yet either), so its hint is not shown.

use cortexcode_code_tui_keybindings::key_hint;
use cortexcode_code_tui_theme::get_editor_theme;
use cortexcode_tui_components::{Editor, EditorHost, EditorOptions, FrameBorderStyle};
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::Component;

use crate::input_frame::{InputFrame, InputFrameOptions};
use std::cell::RefCell;
use std::rc::Rc;

/// How the dialog closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorOutcome {
    Submitted(String),
    Cancelled,
}

/// `ExtensionEditorComponent`.
pub struct ExtensionEditorComponent {
    frame: InputFrame,
    editor: Rc<RefCell<Editor>>,
    on_done: Box<dyn FnMut(EditorOutcome)>,
    submitted: Rc<RefCell<Option<String>>>,
}

impl ExtensionEditorComponent {
    pub fn new(
        host: EditorHost,
        title: &str,
        prefill: Option<&str>,
        on_done: Box<dyn FnMut(EditorOutcome)>,
    ) -> Self {
        let mut frame = InputFrame::new(InputFrameOptions {
            title: Some(title.to_string()),
            ..Default::default()
        });
        // The frame already rules the pane, so the editor draws no border.
        let mut editor = Editor::new(
            host,
            get_editor_theme(),
            EditorOptions {
                border: Some(FrameBorderStyle::None),
                ..Default::default()
            },
        );
        if let Some(text) = prefill.filter(|t| !t.is_empty()) {
            editor.set_text(text);
        }
        let submitted: Rc<RefCell<Option<String>>> = Rc::default();
        let sink = submitted.clone();
        editor.on_submit = Some(Box::new(move |text: &str| {
            *sink.borrow_mut() = Some(text.to_string());
        }));
        let editor = Rc::new(RefCell::new(editor));
        frame.add_child(editor.clone());
        frame.set_hint(&format!(
            "{}  {}  {}",
            key_hint("tui.select.confirm", "submit"),
            key_hint("tui.input.newLine", "newline"),
            key_hint("tui.select.cancel", "cancel")
        ));
        Self {
            frame,
            editor,
            on_done,
            submitted,
        }
    }
}

impl Component for ExtensionEditorComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        if get_keybindings().matches(data, "tui.select.cancel") {
            (self.on_done)(EditorOutcome::Cancelled);
            return;
        }
        self.editor.borrow_mut().handle_input(data);
        let submitted = self.submitted.borrow_mut().take();
        if let Some(text) = submitted {
            (self.on_done)(EditorOutcome::Submitted(text));
        }
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.editor.borrow_mut().set_focused(focused);
    }
}
