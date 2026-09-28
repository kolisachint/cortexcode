//! `core/tools/bash.ts` renderers: `$ command` and the output preview (the
//! last peek-budget visual lines), truncation notes and the `Took`/`Elapsed`
//! line.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tool_api::{format_size, DEFAULT_MAX_BYTES};
use cortexcode_code_tui_keybindings::key_hint;
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_components::markdown::js_trim;
use cortexcode_tui_components::Text;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::truncate_to_width;
use serde_json::{json, Value};

use super::search::js_number;
use super::text;
use crate::render_utils::{get_text_output, invalid_arg_text, str_arg};
use crate::tool_execution::{ToolRenderDefinition, ToolRenderResultOptions, ToolResultView};
use crate::tool_output_view::PEEK_LINES;
use crate::visual_truncate::truncate_to_visual_lines;

/// State key: the block needs re-rendering every second (a live `Elapsed`).
pub const TICKING_KEY: &str = "interval";

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// `formatDuration`: seconds with one decimal.
fn format_duration(ms: f64) -> String {
    cortexcode_code_agent_session::format::js_to_fixed(ms / 1000.0, 1) + "s"
}

pub fn format_bash_call(args: &Value) -> String {
    let t = theme();
    let command = str_arg(args.get("command"));
    let timeout = args.get("timeout").filter(|v| match v {
        Value::Null => false,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        _ => true,
    });
    let suffix = timeout
        .map(|v| t.fg("muted", &format!(" (timeout {}s)", js_number(v))))
        .unwrap_or_default();
    let display = match command {
        None => invalid_arg_text(),
        Some(c) if !c.is_empty() => c,
        Some(_) => t.fg("toolOutput", "..."),
    };
    format!(
        "{}{suffix}",
        t.fg("toolTitle", &t.bold(&format!("$ {display}")))
    )
}

/// The result body (`BashResultRenderComponent`), laid out at render width.
pub struct BashResult {
    styled_output: Option<String>,
    expanded: bool,
    warning: Option<String>,
    timing: Option<String>,
}

impl Component for BashResult {
    fn render(&mut self, width: u16) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(output) = &self.styled_output {
            if self.expanded {
                lines.extend(Text::new(format!("\n{output}"), 0, 0).render(width));
            } else {
                let preview = truncate_to_visual_lines(output, PEEK_LINES, width, 0);
                lines.push(String::new());
                if preview.skipped_count > 0 {
                    let hint = format!(
                        "{} {})",
                        theme().fg(
                            "muted",
                            &format!("... ({} earlier lines,", preview.skipped_count)
                        ),
                        key_hint("app.tools.expand", "to expand")
                    );
                    lines.push(truncate_to_width(&hint, width as usize, "...", false));
                }
                lines.extend(preview.visual_lines);
            }
        }
        for extra in [&self.warning, &self.timing].into_iter().flatten() {
            lines.extend(Text::new(extra.clone(), 0, 0).render(width));
        }
        lines
    }
}

fn build_result(
    result: &ToolResultView<'_>,
    options: ToolRenderResultOptions,
    show_images: bool,
    started_at: Option<f64>,
    ended_at: Option<f64>,
) -> BashResult {
    let t = theme();
    let output = get_text_output(Some(result.content), show_images);
    let output = js_trim(&output);
    let styled_output = (!output.is_empty()).then(|| {
        output
            .split('\n')
            .map(|l| t.fg("toolOutput", l))
            .collect::<Vec<_>>()
            .join("\n")
    });

    let truncation = result.details.get("truncation").filter(|v| !v.is_null());
    let truncated = truncation.is_some_and(|tr| tr.get("truncated") == Some(&Value::Bool(true)));
    let full_output_path = result
        .details
        .get("fullOutputPath")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty());
    let warning = (truncated || full_output_path.is_some()).then(|| {
        let mut warnings = Vec::new();
        if let Some(path) = full_output_path {
            warnings.push(format!("Full output: {path}"));
        }
        if let Some(tr) = truncation.filter(|_| truncated) {
            let num = |k: &str| {
                tr.get(k)
                    .map(js_number)
                    .unwrap_or_else(|| "undefined".into())
            };
            if tr.get("truncatedBy").and_then(Value::as_str) == Some("lines") {
                warnings.push(format!(
                    "Truncated: showing {} of {} lines",
                    num("outputLines"),
                    num("totalLines")
                ));
            } else {
                let max_bytes = tr
                    .get("maxBytes")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                    .unwrap_or(DEFAULT_MAX_BYTES);
                warnings.push(format!(
                    "Truncated: {} lines shown ({} limit)",
                    num("outputLines"),
                    format_size(max_bytes)
                ));
            }
        }
        format!(
            "\n{}",
            t.fg("warning", &format!("[{}]", warnings.join(". ")))
        )
    });

    let timing = started_at.map(|start| {
        let label = if options.is_partial {
            "Elapsed"
        } else {
            "Took"
        };
        let end = ended_at.unwrap_or_else(now_ms);
        format!(
            "\n{}",
            t.fg(
                "muted",
                &format!("{label} {}", format_duration(end - start))
            )
        )
    });

    BashResult {
        styled_output,
        expanded: options.expanded,
        warning,
        timing,
    }
}

pub fn definition() -> ToolRenderDefinition {
    ToolRenderDefinition {
        render_call: Some(Rc::new(|args, ctx| {
            if ctx.execution_started && ctx.state.get("startedAt").is_none() {
                ctx.state.insert("startedAt".into(), json!(now_ms()));
                ctx.state.remove("endedAt");
            }
            Ok(text(format_bash_call(args)))
        })),
        render_result: Some(Rc::new(|result, options, ctx| {
            let started = ctx.state.get("startedAt").and_then(Value::as_f64);
            if started.is_some() && options.is_partial && !ctx.state.contains_key(TICKING_KEY) {
                ctx.state.insert(TICKING_KEY.into(), json!(true));
            }
            if !options.is_partial || ctx.is_error {
                if ctx.state.get("endedAt").is_none() {
                    ctx.state.insert("endedAt".into(), json!(now_ms()));
                }
                ctx.state.remove(TICKING_KEY);
            }
            let ended = ctx.state.get("endedAt").and_then(Value::as_f64);
            let body = build_result(result, options, ctx.show_images, started, ended);
            Ok(Rc::new(RefCell::new(body)))
        })),
        render_shell: None,
    }
}
