//! The tool renderers against the pin's own `renderCall`/`renderResult`
//! output (`fixtures/tool-renderers-gold.json`, from
//! `migration/tools/goldens/tool-renderers.mjs`).

mod support;

use cortexcode_ai_types::Content;
use cortexcode_code_tui_widgets::tool_execution::{
    ToolRenderContext, ToolRenderDefinition, ToolRenderResultOptions, ToolResultView,
};
use cortexcode_code_tui_widgets::tools::{builtin_tool_definition, registered_tool_definition};
use serde_json::Value;
use support::lock;

const CWD: &str = "/work/project";

/// Result bodies the pin highlights (the path has a language) need the
/// syntax highlighter, which task 11.2d installs; it removes this check.
fn needs_highlighter(args: &Value) -> bool {
    let path = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .and_then(Value::as_str)
        .unwrap_or("");
    cortexcode_code_tui_theme::get_language_from_path(path).is_some()
}

fn definition(tool: &str) -> ToolRenderDefinition {
    builtin_tool_definition(tool, CWD).unwrap_or_else(|| registered_tool_definition(tool))
}

fn content(result: &Value) -> Vec<Content> {
    serde_json::from_value(result["content"].clone()).unwrap()
}

#[test]
fn renderers_match_the_pin() {
    let _g = lock();
    let gold: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/tool-renderers-gold.json")).unwrap();
    let mut failures = Vec::new();
    for case in &gold {
        let tool = case["tool"].as_str().unwrap();
        let args = &case["args"];
        let expanded = case["expanded"].as_bool().unwrap();
        let is_error = case["isError"].as_bool().unwrap();
        let def = definition(tool);
        let mut state = serde_json::Map::new();
        let mut ctx = ToolRenderContext {
            args,
            tool_call_id: "t",
            last_component: None,
            state: &mut state,
            cwd: CWD,
            execution_started: true,
            args_complete: true,
            is_partial: false,
            expanded,
            show_images: false,
            is_error,
        };
        let call = def
            .render_call
            .as_ref()
            .map(|f| f(args, &mut ctx).unwrap().borrow_mut().render(120));
        let want_call: Option<Vec<String>> = serde_json::from_value(case["call"].clone()).unwrap();
        if call != want_call {
            failures.push(format!(
                "{tool} {args} call (expanded={expanded})\n  want {want_call:?}\n  got  {call:?}"
            ));
        }
        let result = &case["result"];
        let res = match (&def.render_result, result.is_null()) {
            (Some(f), false) => {
                let content = content(result);
                let details = result.get("details").cloned().unwrap_or(Value::Null);
                let view = ToolResultView {
                    content: &content,
                    details: &details,
                };
                let options = ToolRenderResultOptions {
                    expanded,
                    is_partial: false,
                };
                Some(
                    f(&view, options, &mut ctx)
                        .unwrap()
                        .borrow_mut()
                        .render(120),
                )
            }
            _ => None,
        };
        let want_res: Option<Vec<String>> = serde_json::from_value(case["res"].clone()).unwrap();
        if res != want_res && !(tool == "read" && needs_highlighter(args)) {
            failures.push(format!(
                "{tool} {args} result (expanded={expanded})\n  want {want_res:?}\n  got  {res:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
