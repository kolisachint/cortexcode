//! `core/tools/edit.ts` renderers: a framed call (header band tinted by the
//! preview's outcome) carrying the diff the edit would make, and the result
//! (only what the preview did not already show).

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use cortexcode_code_tools_fs::edit_diff::{compute_edits_diff, Edit};
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_components::{BoxComponent, Spacer, Text};
use cortexcode_tui_render::{Component, ComponentHandle, Container};
use serde_json::{json, Value};

use crate::diff::render_diff;
use crate::render_utils::{arg_or, invalid_arg_text, shorten_path, str_arg};
use crate::tool_execution::ToolRenderDefinition;

/// `EditPreview`: the diff, or why the edit cannot apply.
#[derive(Debug, Clone, PartialEq)]
enum Preview {
    Diff {
        diff: String,
        first_changed_line: Option<usize>,
    },
    Error(String),
}

/// `EditCallRenderComponent`'s state, shared by both slots.
#[derive(Default)]
struct CallState {
    args: Value,
    preview: Option<Preview>,
    preview_args_key: Option<String>,
    settled_error: bool,
}

const CALL_KEY: &str = "edit.callComponent";

/// `getRenderablePreviewInput`: the path and complete edits, when present.
fn preview_input(args: &Value) -> Option<(String, Vec<Edit>, Value)> {
    let path = match (args.get("path"), args.get("file_path")) {
        (Some(Value::String(p)), _) => p.clone(),
        (_, Some(Value::String(p))) => p.clone(),
        _ => return None,
    };
    if path.is_empty() {
        return None;
    }
    let edit = |e: &Value| -> Option<Edit> {
        Some(Edit {
            old_text: e.get("oldText")?.as_str()?.to_string(),
            new_text: e.get("newText")?.as_str()?.to_string(),
            replace_all: e
                .get("replaceAll")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    };
    if let Some(list) = args.get("edits").and_then(Value::as_array) {
        if !list.is_empty() {
            let edits: Option<Vec<Edit>> = list.iter().map(edit).collect();
            if let Some(edits) = edits {
                return Some((path, edits, Value::Array(list.clone())));
            }
        }
    }
    if let (Some(Value::String(old)), Some(Value::String(new))) =
        (args.get("oldText"), args.get("newText"))
    {
        return Some((
            path,
            vec![Edit {
                old_text: old.clone(),
                new_text: new.clone(),
                replace_all: false,
            }],
            json!([{ "oldText": old, "newText": new }]),
        ));
    }
    None
}

/// `JSON.stringify({ path, edits })`, the preview's cache key.
fn args_key(path: &str, edits: &Value) -> String {
    json!({ "path": path, "edits": edits }).to_string()
}

fn format_edit_call(args: &Value) -> String {
    let t = theme();
    let path = str_arg(arg_or(args, "file_path", "path")).map(|p| shorten_path(&p));
    let display = match path {
        None => invalid_arg_text(),
        Some(p) if !p.is_empty() => t.fg("accent", &p),
        Some(_) => t.fg("toolOutput", "..."),
    };
    format!("{} {display}", t.fg("toolTitle", &t.bold("edit")))
}

/// The call component: rebuilt from the shared state at render time, so a
/// preview the result slot settles shows without re-running the call slot.
struct EditCall {
    state: Rc<RefCell<CallState>>,
}

impl Component for EditCall {
    fn render(&mut self, width: u16) -> Vec<String> {
        let state = self.state.borrow();
        let banded = state.preview.is_some() || state.settled_error;
        let bg: Box<dyn Fn(&str) -> String> = match (&state.preview, state.settled_error) {
            (Some(Preview::Error(_)), _) | (None, true) => {
                Box::new(|s: &str| theme().bg("toolErrorBg", s))
            }
            (Some(Preview::Diff { .. }), _) => Box::new(|s: &str| theme().bg("toolSuccessBg", s)),
            (None, false) => Box::new(|s: &str| theme().bg("toolPendingBg", s)),
        };
        let mut b = BoxComponent::new(if banded { 1 } else { 0 }, 0, Some(bg));
        b.add_child(Rc::new(RefCell::new(Text::new(
            format_edit_call(&state.args),
            0,
            0,
        ))));
        if let Some(preview) = &state.preview {
            let body = match preview {
                Preview::Error(e) => theme().fg("error", e),
                Preview::Diff { diff, .. } => render_diff(diff),
            };
            b.add_child(Rc::new(RefCell::new(Spacer::new(1))));
            b.add_child(Rc::new(RefCell::new(Text::new(body, 0, 0))));
        }
        b.render(width)
    }
}

fn call_state(
    objects: &mut std::collections::HashMap<String, Rc<dyn std::any::Any>>,
) -> Rc<RefCell<CallState>> {
    if let Some(existing) = objects.get(CALL_KEY) {
        if let Ok(state) = existing.clone().downcast::<RefCell<CallState>>() {
            return state;
        }
    }
    let state = Rc::new(RefCell::new(CallState::default()));
    objects.insert(CALL_KEY.into(), state.clone());
    state
}

pub fn definition() -> ToolRenderDefinition {
    ToolRenderDefinition {
        render_call: Some(Rc::new(|args, ctx| {
            let state = call_state(ctx.objects);
            {
                let mut s = state.borrow_mut();
                s.args = args.clone();
                let input = preview_input(args);
                let key = input.as_ref().map(|(p, _, raw)| args_key(p, raw));
                if s.preview_args_key != key {
                    s.preview = None;
                    s.preview_args_key = key.clone();
                    s.settled_error = false;
                }
                if ctx.args_complete && s.preview.is_none() {
                    if let Some((path, edits, _)) = &input {
                        // The pin computes this in the background and
                        // re-renders when it lands; here it is immediate.
                        s.preview =
                            Some(match compute_edits_diff(path, edits, Path::new(ctx.cwd)) {
                                Ok(d) => Preview::Diff {
                                    diff: d.diff,
                                    first_changed_line: d.first_changed_line,
                                },
                                Err(e) => Preview::Error(e),
                            });
                        s.preview_args_key = key;
                    }
                }
            }
            let call: ComponentHandle = Rc::new(RefCell::new(EditCall { state }));
            Ok(call)
        })),
        render_result: Some(Rc::new(|result, _options, ctx| {
            let existing = ctx.objects.contains_key(CALL_KEY);
            let result_diff = if ctx.is_error {
                None
            } else {
                result
                    .details
                    .get("diff")
                    .and_then(Value::as_str)
                    .map(String::from)
            };
            let mut preview: Option<Preview> = None;
            if existing {
                let state = call_state(ctx.objects);
                let mut s = state.borrow_mut();
                if let Some(diff) = &result_diff {
                    let first = result
                        .details
                        .get("firstChangedLine")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize);
                    s.preview = Some(Preview::Diff {
                        diff: diff.clone(),
                        first_changed_line: first,
                    });
                    s.preview_args_key =
                        preview_input(ctx.args).map(|(p, _, raw)| args_key(&p, &raw));
                }
                s.settled_error = ctx.is_error;
                preview = s.preview.clone();
            }
            // `formatEditResult`: only what the preview does not already show.
            let (preview_diff, preview_error) = match &preview {
                Some(Preview::Diff { diff, .. }) => (Some(diff.clone()), None),
                Some(Preview::Error(e)) => (None, Some(e.clone())),
                None => (None, None),
            };
            let output = if ctx.is_error {
                let error = result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        cortexcode_ai_types::Content::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (!error.is_empty() && Some(&error) != preview_error.as_ref())
                    .then(|| theme().fg("error", &error))
            } else {
                result
                    .details
                    .get("diff")
                    .and_then(Value::as_str)
                    .filter(|d| !d.is_empty() && Some(d.to_string()) != preview_diff)
                    .map(render_diff)
            };
            let mut container = Container::new();
            if let Some(output) = output {
                container.add_child(Rc::new(RefCell::new(Spacer::new(1))));
                container.add_child(Rc::new(RefCell::new(Text::new(output, 1, 0))));
            }
            let handle: ComponentHandle = Rc::new(RefCell::new(container));
            Ok(handle)
        })),
        render_shell: Some(crate::tool_execution::RenderShell::SelfRendered),
    }
}
