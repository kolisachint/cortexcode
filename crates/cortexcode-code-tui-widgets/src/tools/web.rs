//! `core/tools/webfetch.ts` and `websearch.ts` renderers.

use std::rc::Rc;

use cortexcode_code_tui_keybindings::key_hint;
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_components::markdown::js_trim;
use serde_json::Value;

use super::search::js_number;
use super::text;
use crate::render_utils::{get_text_output, invalid_arg_text, str_arg};
use crate::tool_execution::{ToolRenderDefinition, ToolRenderResultOptions, ToolResultView};
use crate::tool_output_view::PEEK_LINES;

/// The first lines of the output, peek-trimmed, as both web tools print it.
fn peek_output(
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
            text.push_str(&format!(
                "{} {})",
                t.fg(
                    "muted",
                    &format!("\n... ({} more lines,", lines.len() - max)
                ),
                key_hint("app.tools.expand", "to expand")
            ));
        }
    }
    text
}

fn present(details: &Value, key: &str) -> Option<Value> {
    details.get(key).filter(|v| !v.is_null()).cloned()
}

pub fn format_webfetch_call(args: &Value) -> String {
    let t = theme();
    let url = match str_arg(args.get("url")) {
        None => invalid_arg_text(),
        Some(u) if !u.is_empty() => u,
        Some(_) => t.fg("toolOutput", "..."),
    };
    let format = if args.get("output").and_then(Value::as_str) == Some("markdown") {
        t.fg("muted", " (markdown)")
    } else {
        String::new()
    };
    format!(
        "{}{}{format}",
        t.fg("toolTitle", &t.bold("webfetch ")),
        t.fg("accent", &url)
    )
}

pub fn format_webfetch_result(
    result: &ToolResultView<'_>,
    options: ToolRenderResultOptions,
    show_images: bool,
) -> String {
    let t = theme();
    let mut text = peek_output(result, options, show_images);
    if let Some(estimate) = present(result.details, "tokenEstimate") {
        let truncated = result
            .details
            .get("truncated")
            .is_some_and(|v| v.as_bool().unwrap_or(!v.is_null()));
        let cut = if truncated {
            let at = present(result.details, "maxTokens").unwrap_or_else(|| estimate.clone());
            t.fg("warning", &format!(" (truncated at {})", js_number(&at)))
        } else {
            String::new()
        };
        let view = if let Some(sections) = present(result.details, "sectionCount") {
            t.fg(
                "muted",
                &format!(" · outline, {} sections", js_number(&sections)),
            )
        } else if let Some(hits) = present(result.details, "matchCount") {
            let n = js_number(&hits);
            let plural = if hits.as_f64() == Some(1.0) { "" } else { "es" };
            t.fg("muted", &format!(" · {n} match{plural}"))
        } else {
            String::new()
        };
        text.push_str(&format!(
            "\n{}{cut}{view}",
            t.fg("muted", &format!("~{} tokens", js_number(&estimate)))
        ));
    }
    text
}

pub fn format_websearch_call(args: &Value) -> String {
    let t = theme();
    let query = match str_arg(args.get("query")) {
        None => invalid_arg_text(),
        Some(q) if !q.is_empty() => format!("\"{q}\""),
        Some(_) => t.fg("toolOutput", "..."),
    };
    let mut text = format!(
        "{}{}",
        t.fg("toolTitle", &t.bold("websearch ")),
        t.fg("accent", &query)
    );
    if let Some(limit) = args.get("maxResults") {
        text.push_str(&t.fg("muted", &format!(" ({})", js_number(limit))));
    }
    text
}

pub fn format_websearch_result(
    result: &ToolResultView<'_>,
    options: ToolRenderResultOptions,
    show_images: bool,
) -> String {
    let mut text = peek_output(result, options, show_images);
    if let Some(estimate) = present(result.details, "tokenEstimate") {
        text.push_str(&format!(
            "\n{}",
            theme().fg("muted", &format!("~{} tokens", js_number(&estimate)))
        ));
    }
    text
}

pub fn webfetch_definition() -> ToolRenderDefinition {
    ToolRenderDefinition {
        render_call: Some(Rc::new(|args, _| Ok(text(format_webfetch_call(args))))),
        render_result: Some(Rc::new(|result, options, ctx| {
            Ok(text(format_webfetch_result(
                result,
                options,
                ctx.show_images,
            )))
        })),
        render_shell: None,
    }
}

pub fn websearch_definition() -> ToolRenderDefinition {
    ToolRenderDefinition {
        render_call: Some(Rc::new(|args, _| Ok(text(format_websearch_call(args))))),
        render_result: Some(Rc::new(|result, options, ctx| {
            Ok(text(format_websearch_result(
                result,
                options,
                ctx.show_images,
            )))
        })),
        render_shell: None,
    }
}
