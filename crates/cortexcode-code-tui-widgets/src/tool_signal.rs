//! `components/tool-signal.ts`: the radar view's one-line row per call.
//!
//! ```text
//! ● bash            npm run check                             412 lines
//! ● read            packages/tui/src/keys.ts                 1451 lines
//! ● edit            src/core/keybindings.ts                         ok
//! ```

use std::path::Path;

use cortexcode_ai_types::Content;
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_components::markdown::js_trim;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{truncate_to_width, visible_width};
use serde_json::Value;

use crate::render_utils::get_text_output;

/// Width of the tool-name column (`SearchCodebase` is the longest built-in).
const VERB_WIDTH: usize = 14;
/// Gap between the verb column and the subject.
const VERB_GAP: usize = 2;
/// Minimum space kept for the flush-right signal before the subject is trimmed.
const MIN_SIGNAL_GAP: usize = 2;

/// Argument names that carry a call's subject, most specific first.
const SUBJECT_KEYS: [&str; 12] = [
    "command",
    "file_path",
    // Before `path`: for a search the pattern identifies the call.
    "pattern",
    "query",
    "url",
    "path",
    "prompt",
    "description",
    "task_id",
    "subagent_type",
    "name",
    "id",
];

fn non_blank_string(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !js_trim(s).is_empty())
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `toolSubject`: the single most identifying argument, on one line.
pub fn tool_subject(args: &Value, cwd: &str) -> String {
    let Some(record) = args.as_object() else {
        return String::new();
    };
    let subject = SUBJECT_KEYS
        .iter()
        .find_map(|key| record.get(*key).and_then(non_blank_string))
        .or_else(|| record.values().find_map(non_blank_string));
    let Some(subject) = subject else {
        return String::new();
    };
    // Paths are the most common subject and the least readable in absolute form.
    let relative = cortexcode_code_paths::cwd_relative_path(Path::new(subject), Path::new(cwd))
        .map(|p| p.to_string_lossy().into_owned());
    let display = match &relative {
        Some(r) if utf16_len(r) < utf16_len(subject) => r.as_str(),
        _ => subject,
    };
    // Multi-line commands collapse to one line.
    let mut out = String::new();
    let mut in_space = false;
    for c in display.chars() {
        if crate::is_js_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    js_trim(&out).to_string()
}

/// A finished (or partial) tool result, as the tool blocks see it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolResult {
    pub content: Vec<Content>,
    pub details: Value,
    pub is_error: bool,
}

/// `ToolSignalInput`.
#[derive(Debug, Clone, Default)]
pub struct ToolSignalInput {
    pub tool_name: String,
    pub args: Value,
    pub cwd: String,
    pub result: Option<ToolResult>,
    pub is_partial: bool,
    pub show_images: bool,
    /// The newest call in the transcript. Themes that set `activeToolBg` mark it.
    pub is_latest: bool,
}

/// `toolSignal`: how much output came back, or why none did.
pub fn tool_signal(input: &ToolSignalInput) -> (String, &'static str) {
    let Some(result) = &input.result else {
        return (
            if input.is_partial {
                "running".into()
            } else {
                String::new()
            },
            "warning",
        );
    };
    if result.is_error {
        return ("error".into(), "error");
    }
    let output = get_text_output(Some(&result.content), input.show_images);
    if js_trim(&output).is_empty() {
        return ("ok".into(), "success");
    }
    let lines = output.split('\n').count();
    (
        format!("{lines} {}", if lines == 1 { "line" } else { "lines" }),
        "success",
    )
}

/// `renderToolSignalLine`: one radar row, without the status dot.
pub fn render_tool_signal_line(input: &ToolSignalInput, width: usize) -> String {
    let name: String = input.tool_name.chars().take(VERB_WIDTH).collect();
    let verb = format!("{name:<w$}", w = VERB_WIDTH + VERB_GAP);
    let (signal, color) = tool_signal(input);
    let subject = tool_subject(&input.args, &input.cwd);

    let available = width;
    let signal_width = if signal.is_empty() {
        0
    } else {
        visible_width(&signal) + MIN_SIGNAL_GAP
    };
    let subject_width = available.saturating_sub(visible_width(&verb) + signal_width);
    let shown_subject = if subject_width > 0 {
        truncate_to_width(&subject, subject_width, "...", false)
    } else {
        String::new()
    };
    let left_width = visible_width(&verb) + visible_width(&shown_subject);
    let min_pad = if signal.is_empty() { 0 } else { MIN_SIGNAL_GAP };
    let pad = min_pad.max(
        (available as isize - left_width as isize - visible_width(&signal) as isize).max(0)
            as usize,
    );

    let t = theme();
    let stroke = input.is_latest && t.has_bg("activeToolBg");
    let content = format!(
        "{}{}",
        t.fg("toolTitle", &t.bold(&verb)),
        t.fg("toolOutput", &shown_subject)
    );
    let content = if stroke {
        t.bg("activeToolBg", &content)
    } else {
        content
    };
    format!("{content}{}{}", " ".repeat(pad), t.fg(color, &signal))
}

/// `ToolSignalComponent`.
pub struct ToolSignalComponent {
    input: ToolSignalInput,
}

impl ToolSignalComponent {
    pub fn new(input: ToolSignalInput) -> Self {
        Self { input }
    }

    pub fn set_input(&mut self, input: ToolSignalInput) {
        self.input = input;
    }
}

impl Component for ToolSignalComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        vec![render_tool_signal_line(&self.input, width as usize)]
    }
}
