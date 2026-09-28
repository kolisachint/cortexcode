//! `core/tools/read.ts` renderers: the call line (compact for skills,
//! resources and the app's own docs until expanded) and the numbered result.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use cortexcode_code_tool_api::{
    format_size, resolve_read_path, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
use cortexcode_code_tui_keybindings::{key_hint, key_text};
use cortexcode_code_tui_theme::{get_language_from_path, highlight_code, message_label, theme};
use serde_json::Value;

use super::search::js_number;
use super::{text, trim_trailing_empty_lines};
use crate::read_output::render_read_output;
use crate::render_utils::{
    arg_or, get_text_output, invalid_arg_text, replace_tabs, shorten_path, str_arg,
};
use crate::tool_execution::{ToolRenderDefinition, ToolRenderResultOptions, ToolResultView};
use crate::tool_output_view::PEEK_LINES;

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompactKind {
    Docs,
    Resource,
    Skill,
}

const COMPACT_RESOURCE_FILE_NAMES: [&str; 4] = ["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

fn raw_path(args: &Value) -> Option<String> {
    str_arg(arg_or(args, "file_path", "path"))
}

fn format_line_range(args: &Value) -> String {
    // `undefined` checks: an explicit `null` counts as given, as in the pin.
    let (offset, limit) = (args.get("offset"), args.get("limit"));
    if offset.is_none() && limit.is_none() {
        return String::new();
    }
    let start = match offset {
        Some(Value::Null) | None => serde_json::json!(1),
        Some(v) => v.clone(),
    };
    let end = limit.map(|l| {
        let n = start.as_f64().unwrap_or(f64::NAN) + l.as_f64().unwrap_or(0.0) - 1.0;
        if n == 0.0 || n.is_nan() {
            String::new()
        } else {
            js_number(&serde_json::json!(n))
        }
    });
    let end = end.filter(|e| !e.is_empty());
    theme().fg(
        "warning",
        &format!(
            ":{}{}",
            js_number(&start),
            end.map(|e| format!("-{e}")).unwrap_or_default()
        ),
    )
}

fn format_read_call(args: &Value) -> String {
    let t = theme();
    let path = raw_path(args).map(|p| shorten_path(&p));
    let display = match path {
        None => invalid_arg_text(),
        Some(p) if !p.is_empty() => t.fg("accent", &p),
        Some(_) => t.fg("toolOutput", "..."),
    };
    format!(
        "{} {display}{}",
        t.fg("toolTitle", &t.bold("read")),
        format_line_range(args)
    )
}

/// `path.relative` for two absolute, normalized paths.
fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for c in &to[common..] {
        out.push(c.as_os_str());
    }
    out
}

/// `getReadmePath`: the package's README.
pub fn readme_path() -> PathBuf {
    cortexcode_code_paths::package_dir().join("README.md")
}

fn docs_classification(absolute: &Path) -> Option<(CompactKind, String)> {
    let package_root = std::path::absolute(readme_path())
        .ok()?
        .parent()?
        .to_path_buf();
    let absolute = std::path::absolute(absolute).ok()?;
    let rel = relative(&package_root, &absolute);
    let rel_str = rel
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    if rel_str.is_empty() || rel_str == ".." || rel_str.starts_with("../") || rel.is_absolute() {
        return None;
    }
    if rel_str == "README.md" || rel_str.starts_with("docs/") || rel_str.starts_with("examples/") {
        return Some((CompactKind::Docs, rel_str));
    }
    None
}

fn compact_classification(args: &Value, cwd: &str) -> Option<(CompactKind, String)> {
    let raw = raw_path(args).filter(|p| !p.is_empty())?;
    let absolute = resolve_read_path(&raw, Path::new(cwd));
    let file_name = absolute
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if file_name == "SKILL.md" {
        let dir = absolute
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or(file_name);
        return Some((CompactKind::Skill, dir));
    }
    if let Some(docs) = docs_classification(&absolute) {
        return Some(docs);
    }
    if COMPACT_RESOURCE_FILE_NAMES.contains(&file_name.as_str()) {
        return Some((
            CompactKind::Resource,
            cortexcode_code_paths::format_path_relative_to_cwd_or_absolute(
                &absolute,
                Path::new(cwd),
            ),
        ));
    }
    None
}

fn format_compact_read_call(kind: &CompactKind, label: &str, args: &Value) -> String {
    let t = theme();
    let expand_hint = t.fg(
        "dim",
        &format!(" ({} to expand)", key_text("app.tools.expand")),
    );
    if *kind == CompactKind::Skill {
        return format!(
            "{} {}{}{expand_hint}",
            message_label("skill"),
            t.fg("customMessageText", label),
            format_line_range(args)
        );
    }
    let kind = match kind {
        CompactKind::Docs => "docs",
        _ => "resource",
    };
    format!(
        "{} {}{}{expand_hint}",
        t.fg("toolTitle", &t.bold(&format!("read {kind}"))),
        t.fg("accent", label),
        format_line_range(args)
    )
}

fn format_read_result(
    args: &Value,
    result: &ToolResultView<'_>,
    options: ToolRenderResultOptions,
    show_images: bool,
    cwd: &str,
    is_error: bool,
) -> String {
    if !options.expanded && !is_error && compact_classification(args, cwd).is_some() {
        return String::new();
    }
    let t = theme();
    let raw = raw_path(args);
    let output = get_text_output(Some(result.content), show_images);
    let lang = raw.as_deref().and_then(get_language_from_path);
    let rendered: Vec<String> = match lang {
        Some(lang) => highlight_code(&replace_tabs(&output), Some(lang)),
        None => output.split('\n').map(str::to_string).collect(),
    };
    let lines = trim_trailing_empty_lines(rendered);
    let max = if options.expanded {
        lines.len()
    } else {
        PEEK_LINES
    };
    let shown: Vec<String> = lines.iter().take(max).cloned().collect();
    let start_line = args
        .get("offset")
        .and_then(Value::as_f64)
        .map(|n| n as i64)
        .unwrap_or(1);
    let numbered = render_read_output(&shown.join("\n"), start_line);
    let mut text = format!("\n{}", numbered.join("\n"));
    if lines.len() > max {
        let remaining = lines.len() - max;
        text.push_str(&format!(
            "{} {})",
            t.fg("muted", &format!("\n... ({remaining} more lines,")),
            key_hint("app.tools.expand", "to expand")
        ));
    }

    let truncation = result.details.get("truncation");
    if let Some(tr) = truncation.filter(|tr| tr.get("truncated") == Some(&Value::Bool(true))) {
        let num = |key: &str, default: usize| {
            tr.get(key)
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(default)
        };
        let max_bytes = num("maxBytes", DEFAULT_MAX_BYTES);
        if tr.get("firstLineExceedsLimit") == Some(&Value::Bool(true)) {
            text.push_str(&format!(
                "\n{}",
                t.fg(
                    "warning",
                    &format!("[First line exceeds {} limit]", format_size(max_bytes))
                )
            ));
        } else if tr.get("truncatedBy").and_then(Value::as_str) == Some("lines") {
            text.push_str(&format!(
                "\n{}",
                t.fg(
                    "warning",
                    &format!(
                        "[Truncated: showing {} of {} lines ({} line limit)]",
                        num("outputLines", 0),
                        num("totalLines", 0),
                        num("maxLines", DEFAULT_MAX_LINES)
                    )
                )
            ));
        } else {
            text.push_str(&format!(
                "\n{}",
                t.fg(
                    "warning",
                    &format!(
                        "[Truncated: {} lines shown ({} limit)]",
                        num("outputLines", 0),
                        format_size(max_bytes)
                    )
                )
            ));
        }
    }
    text
}

pub fn definition() -> ToolRenderDefinition {
    ToolRenderDefinition {
        render_call: Some(Rc::new(|args, ctx| {
            let compact = if ctx.expanded {
                None
            } else {
                compact_classification(args, ctx.cwd)
            };
            Ok(text(match compact {
                Some((kind, label)) => format_compact_read_call(&kind, &label, args),
                None => format_read_call(args),
            }))
        })),
        render_result: Some(Rc::new(|result, options, ctx| {
            Ok(text(format_read_result(
                ctx.args,
                result,
                options,
                ctx.show_images,
                ctx.cwd,
                ctx.is_error,
            )))
        })),
        render_shell: None,
    }
}
