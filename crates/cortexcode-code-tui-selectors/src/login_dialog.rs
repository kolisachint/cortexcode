//! The login dialog that replaces the prompt during `/login`, hoocode
//! `components/login-dialog.ts`.
//!
//! Adaptations: the original's `showPrompt`/`showManualInput` return a
//! promise the flow awaits; here they arm the input and the answer comes out
//! as a [`LoginDialogEvent`] from [`LoginDialogComponent::take_events`].
//! `showAuth` does not open the browser itself; the owner calls the URL
//! opener (`utils/open-url.ts`) next to it. The provider's display name is
//! resolved by the caller.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_ai_types::AbortSignal;
use cortexcode_code_tui_keybindings::key_hint;
use cortexcode_code_tui_theme::{style_input, theme};
use cortexcode_code_tui_widgets::input_frame::{InputFrame, InputFrameOptions};
use cortexcode_tui_components::{Input, Spacer, Text};
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::{Component, ComponentHandle, Container};

/// What the dialog hands back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginDialogEvent {
    /// The armed input was submitted.
    Submitted(String),
    /// Escape: the login is cancelled and the signal aborted.
    Cancelled,
}

/// `LoginDialogComponent`.
pub struct LoginDialogComponent {
    frame: InputFrame,
    content: Rc<RefCell<Container>>,
    input: Rc<RefCell<Input>>,
    placeholder: Option<ComponentHandle>,
    signal: AbortSignal,
    /// An answer is being waited for (`inputResolver`).
    awaiting_input: bool,
    submitted: Rc<RefCell<Option<String>>>,
    events: Vec<LoginDialogEvent>,
}

fn text(content: String, padding_x: usize) -> ComponentHandle {
    Rc::new(RefCell::new(Text::new(content, padding_x, 0)))
}

impl LoginDialogComponent {
    /// Titled `Login to <provider>` unless `title_override` names it.
    pub fn new(provider_name: &str, title_override: Option<&str>) -> Self {
        let title = title_override
            .map(String::from)
            .unwrap_or_else(|| format!("Login to {provider_name}"));
        let mut frame = InputFrame::new(InputFrameOptions {
            title: Some(title),
            ..Default::default()
        });
        let content: Rc<RefCell<Container>> = Rc::default();
        frame.add_child(content.clone());
        // Something to show until the flow's first step arrives.
        let placeholder = text(theme().fg("dim", "Starting…"), 0);
        content.borrow_mut().add_child(placeholder.clone());
        let mut input = Input::new();
        style_input(&mut input);
        let submitted: Rc<RefCell<Option<String>>> = Rc::default();
        let sink = submitted.clone();
        input.on_submit = Some(Box::new(move |value: &str| {
            *sink.borrow_mut() = Some(value.to_string())
        }));
        Self {
            frame,
            content,
            input: Rc::new(RefCell::new(input)),
            placeholder: Some(placeholder),
            signal: AbortSignal::new(),
            awaiting_input: false,
            submitted,
            events: Vec::new(),
        }
    }

    /// Aborted when the user cancels.
    pub fn signal(&self) -> AbortSignal {
        self.signal.clone()
    }

    /// Events since the last call.
    pub fn take_events(&mut self) -> Vec<LoginDialogEvent> {
        std::mem::take(&mut self.events)
    }

    fn add(&self, component: ComponentHandle) {
        self.content.borrow_mut().add_child(component);
    }

    /// A blank row before the next step, unless nothing is shown yet.
    fn separate(&self) {
        if !self.content.borrow().children.is_empty() {
            self.add(Rc::new(RefCell::new(Spacer::new(1))));
        }
    }

    fn drop_placeholder(&mut self) {
        if let Some(placeholder) = self.placeholder.take() {
            self.content
                .borrow_mut()
                .children
                .retain(|c| !Rc::ptr_eq(c, &placeholder));
        }
    }

    fn cancel(&mut self) {
        self.signal.abort();
        self.awaiting_input = false;
        self.events.push(LoginDialogEvent::Cancelled);
    }

    /// `showAuth`: the URL (a link) and any instructions.
    pub fn show_auth(&mut self, url: &str, instructions: Option<&str>) {
        self.drop_placeholder();
        self.content.borrow_mut().clear();
        let t = theme();
        let linked = format!("\x1b]8;;{url}\x07{url}\x1b]8;;\x07");
        self.add(text(t.fg("accent", &linked), 0));
        let click = format!("\x1b]8;;{url}\x07click to open\x1b]8;;\x07");
        self.add(text(t.fg("dim", &click), 0));
        if let Some(instructions) = instructions.filter(|i| !i.is_empty()) {
            self.add(Rc::new(RefCell::new(Spacer::new(1))));
            self.add(text(t.fg("warning", instructions), 0));
        }
    }

    /// `showManualInput`: an input for a pasted code or redirect URL.
    pub fn show_manual_input(&mut self, prompt: &str) {
        self.drop_placeholder();
        self.separate();
        self.add(text(theme().fg("dim", prompt), 0));
        self.add(self.input.clone());
        self.add(text(
            format!("({})", key_hint("tui.select.cancel", "to cancel")),
            0,
        ));
        self.awaiting_input = true;
    }

    /// `showPrompt`: a question under what is already shown.
    pub fn show_prompt(&mut self, message: &str, placeholder: Option<&str>) {
        self.drop_placeholder();
        self.separate();
        let t = theme();
        self.add(text(t.fg("text", message), 0));
        if let Some(example) = placeholder.filter(|p| !p.is_empty()) {
            self.add(text(t.fg("dim", &format!("e.g., {example}")), 0));
        }
        self.add(self.input.clone());
        self.add(text(
            format!(
                "({} {})",
                key_hint("tui.select.cancel", "to cancel,"),
                key_hint("tui.select.confirm", "to submit")
            ),
            1,
        ));
        self.input.borrow_mut().set_value("");
        self.awaiting_input = true;
    }

    /// `showInfo`: lines to read, nothing to answer.
    pub fn show_info(&mut self, lines: &[String]) {
        self.drop_placeholder();
        self.content.borrow_mut().clear();
        for line in lines {
            self.add(text(line.clone(), 0));
        }
        self.add(Rc::new(RefCell::new(Spacer::new(1))));
        self.add(text(
            format!("({})", key_hint("tui.select.cancel", "to close")),
            0,
        ));
    }

    /// `showWaiting`: a polling flow's wait.
    pub fn show_waiting(&mut self, message: &str) {
        self.drop_placeholder();
        self.separate();
        self.add(text(theme().fg("dim", message), 0));
        self.add(text(
            format!("({})", key_hint("tui.select.cancel", "to cancel")),
            0,
        ));
    }

    /// `showProgress`.
    pub fn show_progress(&mut self, message: &str) {
        self.drop_placeholder();
        self.add(text(theme().fg("dim", message), 0));
    }
}

impl Component for LoginDialogComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        if kb.matches(data, "tui.select.cancel") {
            self.cancel();
            return;
        }
        self.input.borrow_mut().handle_input_with(data, &kb);
        let submitted = self.submitted.borrow_mut().take();
        if let Some(value) = submitted {
            // Only an answer something is waiting for counts.
            if std::mem::take(&mut self.awaiting_input) {
                self.events.push(LoginDialogEvent::Submitted(value));
            }
        }
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.input.borrow_mut().set_focused(focused);
    }
}
