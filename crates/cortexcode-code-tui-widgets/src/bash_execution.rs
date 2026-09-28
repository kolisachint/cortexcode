//! `components/bash-execution.ts`: a user `!` command with streaming output,
//! a spinner while it runs, and a status line once it ends.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tool_api::{
    truncate_tail, TruncationOptions, TruncationResult, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
use cortexcode_code_tool_bash::shell::strip_ansi;
use cortexcode_code_tui_keybindings::key_hint;
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_components::{Loader, Text};
use cortexcode_tui_render::{Component, ComponentHandle};

use crate::visual_truncate::truncate_to_visual_lines;

/// Preview line limit when not expanded.
const PREVIEW_LINES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Running,
    Complete,
    Cancelled,
    Error,
}

/// The collapsed preview: the last visual lines at the render width.
struct Preview {
    styled_input: String,
}

impl Component for Preview {
    fn render(&mut self, width: u16) -> Vec<String> {
        truncate_to_visual_lines(&self.styled_input, PREVIEW_LINES, width, 1).visual_lines
    }
}

pub struct BashExecutionComponent {
    command: String,
    output_lines: Vec<String>,
    status: Status,
    exit_code: Option<i32>,
    loader: Rc<RefCell<Loader>>,
    truncation: Option<TruncationResult>,
    full_output_path: Option<String>,
    expanded: bool,
    children: Vec<ComponentHandle>,
}

impl BashExecutionComponent {
    /// `excludeFromContext` (`!!`) draws the spinner dim instead of in the
    /// bash-mode colour.
    pub fn new(command: &str, exclude_from_context: bool) -> Self {
        let color_key: &'static str = if exclude_from_context {
            "dim"
        } else {
            "bashMode"
        };
        let mut loader = Loader::new(
            Box::new(move |s: &str| theme().fg(color_key, s)),
            Box::new(|s: &str| theme().fg("muted", s)),
            "",
            None,
        );
        loader.start();
        let mut this = Self {
            command: command.to_string(),
            output_lines: Vec::new(),
            status: Status::Running,
            exit_code: None,
            loader: Rc::new(RefCell::new(loader)),
            truncation: None,
            full_output_path: None,
            expanded: false,
            children: Vec::new(),
        };
        // Until the first update the header uses the spinner's colour (dim for
        // `!!`), as the pin's constructor draws it.
        let t = theme();
        let header: ComponentHandle = Rc::new(RefCell::new(Text::new(
            format!(
                "{}{}",
                t.fg("warning", "● "),
                t.fg(color_key, &t.bold(&format!("$ {command}")))
            ),
            1,
            0,
        )));
        this.children = vec![header, this.loader.clone()];
        this
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.update_display();
    }

    /// Advance the spinner; true when it changed.
    pub fn tick(&mut self) -> bool {
        self.status == Status::Running && self.loader.borrow_mut().tick()
    }

    pub fn append_output(&mut self, chunk: &str) {
        let clean = strip_ansi(chunk).replace("\r\n", "\n").replace('\r', "\n");
        let mut new_lines = clean.split('\n').map(String::from);
        if let Some(last) = self.output_lines.last_mut() {
            if let Some(first) = new_lines.next() {
                last.push_str(&first);
            }
        }
        self.output_lines.extend(new_lines);
        self.update_display();
    }

    pub fn set_complete(
        &mut self,
        exit_code: Option<i32>,
        cancelled: bool,
        truncation: Option<TruncationResult>,
        full_output_path: Option<String>,
    ) {
        self.exit_code = exit_code;
        self.status = if cancelled {
            Status::Cancelled
        } else if exit_code.is_some_and(|c| c != 0) {
            Status::Error
        } else {
            Status::Complete
        };
        self.truncation = truncation;
        self.full_output_path = full_output_path;
        self.loader.borrow_mut().stop();
        self.update_display();
    }

    fn update_display(&mut self) {
        let t = theme();
        let full = self.output_lines.join("\n");
        let context = truncate_tail(
            &full,
            TruncationOptions {
                max_lines: Some(DEFAULT_MAX_LINES),
                max_bytes: Some(DEFAULT_MAX_BYTES),
            },
        );
        let available: Vec<&str> = if context.content.is_empty() {
            Vec::new()
        } else {
            context.content.split('\n').collect()
        };
        let preview_start = available.len().saturating_sub(PREVIEW_LINES);
        let hidden = preview_start;

        let mut children: Vec<ComponentHandle> = Vec::new();
        let dot_color = match self.status {
            Status::Error => "error",
            Status::Running => "warning",
            _ => "success",
        };
        children.push(Rc::new(RefCell::new(Text::new(
            format!(
                "{}{}",
                t.fg(dot_color, "● "),
                t.fg("bashMode", &t.bold(&format!("$ {}", self.command)))
            ),
            1,
            0,
        ))));

        if !available.is_empty() {
            if self.expanded {
                let text = available
                    .iter()
                    .map(|l| t.fg("muted", l))
                    .collect::<Vec<_>>()
                    .join("\n");
                children.push(Rc::new(RefCell::new(Text::new(format!("\n{text}"), 1, 0))));
            } else {
                let styled = available[preview_start..]
                    .iter()
                    .map(|l| t.fg("muted", l))
                    .collect::<Vec<_>>()
                    .join("\n");
                children.push(Rc::new(RefCell::new(Preview {
                    styled_input: format!("\n{styled}"),
                })));
            }
        }

        if self.status == Status::Running {
            children.push(self.loader.clone());
        } else {
            let mut parts = Vec::new();
            if hidden > 0 {
                if self.expanded {
                    parts.push(format!("({})", key_hint("app.tools.expand", "to collapse")));
                } else {
                    parts.push(format!(
                        "{} ({})",
                        t.fg("muted", &format!("... {hidden} more lines")),
                        key_hint("app.tools.expand", "to expand")
                    ));
                }
            }
            match self.status {
                Status::Cancelled => parts.push(t.fg("warning", "(cancelled)")),
                Status::Error => parts.push(t.fg(
                    "error",
                    &format!(
                        "(exit {})",
                        self.exit_code.map_or("undefined".to_string(), |c| c.to_string())
                    ),
                )),
                _ => {}
            }
            let truncated =
                self.truncation.as_ref().is_some_and(|tr| tr.truncated) || context.truncated;
            if let (true, Some(path)) = (truncated, &self.full_output_path) {
                parts.push(t.fg("warning", &format!("Output truncated. Full output: {path}")));
            }
            if !parts.is_empty() {
                children.push(Rc::new(RefCell::new(Text::new(
                    format!("\n{}", parts.join("\n")),
                    1,
                    0,
                ))));
            }
        }
        self.children = children;
    }

    /// The raw output, for the `BashExecutionMessage`.
    pub fn output(&self) -> String {
        self.output_lines.join("\n")
    }

    pub fn command(&self) -> &str {
        &self.command
    }
}

impl Component for BashExecutionComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &self.children {
            lines.extend(child.borrow_mut().render(width));
        }
        lines
    }

    fn invalidate(&mut self) {
        for child in &self.children {
            child.borrow_mut().invalidate();
        }
        self.update_display();
    }
}
