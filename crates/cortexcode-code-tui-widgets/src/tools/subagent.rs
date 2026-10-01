//! `core/tools/subagent.ts` renderers: the `Task` dispatch line and
//! `TaskOutput` (its call, and the result card or roster).

use std::rc::Rc;

use cortexcode_ai_types::Content;
use cortexcode_code_agent_session::format::{format_duration_secs, format_tokens};
use cortexcode_code_subagents::inbox::subagent_inbox;
use cortexcode_code_task_store::task_store;
use cortexcode_code_tui_theme::{agent_color_for, theme};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use super::text;
use crate::tool_execution::{ToolRenderDefinition, ToolResultView};

/// A JS value as `String(value ?? "")` prints it.
fn js_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => super::search::js_number(other),
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(_) => true,
    }
}

/// The color of a `type#n` label's agent, or `fallback` for a raw id.
fn label_color(label: &str, fallback: &'static str) -> &'static str {
    match label.find('#') {
        Some(i) if i > 0 => agent_color_for(&label[..i]),
        _ => fallback,
    }
}

pub fn format_task_call(args: &Value) -> String {
    let t = theme();
    let agent_type = match args.get("subagent_type") {
        None | Some(Value::Null) => "agent".to_string(),
        Some(v) => js_string(Some(v)),
    };
    format!(
        "{}{}",
        t.fg("toolTitle", &t.bold("Agent ")),
        t.fg(agent_color_for(&agent_type), &format!("[{agent_type}]"))
    )
}

pub fn format_task_output_call(args: &Value) -> String {
    let t = theme();
    let target = if truthy(args.get("list")) {
        "list".to_string()
    } else {
        js_string(args.get("task_id"))
    };
    let styled = match target.find('#') {
        Some(i) if i > 0 => t.fg(agent_color_for(&target[..i]), &target),
        _ => t.fg("dim", &target),
    };
    format!(
        "{}{styled}{}",
        t.fg("toolTitle", &t.bold("TaskOutput ")),
        if truthy(args.get("wait")) {
            t.fg("dim", " (wait)")
        } else {
            String::new()
        }
    )
}

static ROSTER_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^- (\S+)\s{2}(running|done \(uncollected\)|collected|failed|stalled|timeout|cancelled)(.*)$")
        .expect("roster line")
});

fn now_ms() -> u64 {
    cortexcode_code_task_store::now_ms()
}

pub fn format_task_output_result(result: &ToolResultView<'_>) -> String {
    let t = theme();
    let text = result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(tc) if !tc.text.is_empty() => Some(tc.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return String::new();
    }

    let task_id = result.details.get("task_id").and_then(Value::as_str);
    let card_status = task_id
        .filter(|id| !id.is_empty())
        .and_then(|_| result.details.get("status").and_then(Value::as_str));
    if let Some(status) = card_status.filter(|s| *s != "list" && *s != "empty") {
        let (glyph, color) = match status {
            "done" => ("✓", "success"),
            "running" => ("◐", "warning"),
            "collected" => ("✓", "muted"),
            "cancelled" => ("⊘", "dim"),
            "unknown" => ("?", "dim"),
            _ => ("✗", "error"),
        };
        let label = task_id.unwrap_or("");
        let color_of_label = label_color(label, "accent");
        let record = subagent_inbox().get(label);
        let elapsed = record.as_ref().map(|r| {
            r.ended_at
                .unwrap_or_else(now_ms)
                .saturating_sub(r.started_at)
        });
        let elapsed_text = match elapsed {
            Some(ms) if ms > 0 => format!(" {}", format_duration_secs(ms as f64 / 1000.0)),
            _ => String::new(),
        };
        let task = record.as_ref().and_then(|r| {
            task_store()
                .list()
                .into_iter()
                .find(|task| task.agent.as_deref() == Some(r.task_id.as_str()))
        });
        let token_text = task
            .and_then(|task| task.usage)
            .map(|u| format!(" {}", format_tokens((u.input + u.output) as u64)))
            .unwrap_or_default();
        let spine = |s: &str| t.fg("borderMuted", s);
        let header = format!(
            "{} {} {} {}{token_text}{elapsed_text}",
            spine("╭"),
            t.fg(color, glyph),
            t.bold(&t.fg(color, status)),
            t.fg(color_of_label, label)
        );
        let body = text
            .split('\n')
            .map(|line| format!("{} {}", spine("│"), t.fg("toolOutput", line)))
            .collect::<Vec<_>>()
            .join("\n");
        return format!("{header}\n{body}\n{}", spine("╰"));
    }

    text.split('\n')
        .map(|line| match ROSTER_LINE.captures(line) {
            None => t.fg("toolOutput", line),
            Some(caps) => {
                let label = &caps[1];
                let status = &caps[2];
                let rest = &caps[3];
                let status_color = if status == "running" {
                    "warning"
                } else if status.starts_with("done") {
                    "success"
                } else if status == "collected" || status == "cancelled" {
                    "muted"
                } else {
                    "error"
                };
                format!(
                    "- {}  {}{}",
                    t.fg(label_color(label, "accent"), label),
                    t.fg(status_color, status),
                    t.fg("dim", rest)
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn task_definition() -> ToolRenderDefinition {
    ToolRenderDefinition {
        render_call: Some(Rc::new(|args, _| Ok(text(format_task_call(args))))),
        render_result: None,
        render_shell: None,
    }
}

pub fn task_output_definition() -> ToolRenderDefinition {
    ToolRenderDefinition {
        render_call: Some(Rc::new(|args, _| Ok(text(format_task_output_call(args))))),
        render_result: Some(Rc::new(|result, _, _| {
            Ok(text(format_task_output_result(result)))
        })),
        render_shell: None,
    }
}
