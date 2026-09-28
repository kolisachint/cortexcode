//! `core/tools/search.ts` renderers (`SearchCodebase`).

use std::rc::Rc;

use cortexcode_code_tui_keybindings::key_hint;
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_components::markdown::js_trim;
use serde_json::Value;

use super::text;
use crate::render_utils::{get_text_output, invalid_arg_text, str_arg};
use crate::tool_execution::{ToolRenderDefinition, ToolRenderResultOptions, ToolResultView};
use crate::tool_output_view::PEEK_LINES;

/// A JS number as `String(n)` prints it.
pub(crate) fn js_number(value: &Value) -> String {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{}", f as i64),
            Some(f) => format!("{f}"),
            None => n.to_string(),
        },
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

fn format_search_call(args: &Value) -> String {
    let t = theme();
    let query_display = match str_arg(args.get("query")) {
        None => invalid_arg_text(),
        Some(q) => format!("\"{q}\""),
    };
    let mut text = format!(
        "{}{}",
        t.fg("toolTitle", &t.bold("SearchCodebase ")),
        t.fg("accent", &query_display)
    );
    let mut extras = Vec::new();
    if let Some(mode) = args.get("mode").and_then(Value::as_str) {
        if !mode.is_empty() && mode != "auto" {
            extras.push(mode.to_string());
        }
    }
    if let Some(limit) = args.get("limit").filter(|v| !v.is_null()) {
        extras.push(js_number(limit));
    }
    if !extras.is_empty() {
        text.push_str(&t.fg("muted", &format!(" ({})", extras.join(", "))));
    }
    text
}

fn format_search_result(
    result: &ToolResultView<'_>,
    options: ToolRenderResultOptions,
    show_images: bool,
) -> String {
    let t = theme();
    let output = get_text_output(Some(result.content), show_images);
    let output = js_trim(&output);
    let mut text = String::new();
    if !output.is_empty() {
        let lines: Vec<&str> = output.split('\n').collect();
        let max = if options.expanded {
            lines.len()
        } else {
            PEEK_LINES
        };
        let shown: Vec<String> = lines
            .iter()
            .take(max)
            .map(|l| t.fg("toolOutput", l))
            .collect();
        text.push_str(&format!("\n{}", shown.join("\n")));
        if lines.len() > max {
            let remaining = lines.len() - max;
            text.push_str(&format!(
                "{} {})",
                t.fg("muted", &format!("\n... ({remaining} more lines,")),
                key_hint("app.tools.expand", "to expand")
            ));
        }
    }
    text
}

pub fn definition() -> ToolRenderDefinition {
    ToolRenderDefinition {
        render_call: Some(Rc::new(|args, _ctx| Ok(text(format_search_call(args))))),
        render_result: Some(Rc::new(|result, options, ctx| {
            Ok(text(format_search_result(result, options, ctx.show_images)))
        })),
        render_shell: None,
    }
}
