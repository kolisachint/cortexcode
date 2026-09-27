//! The built-in tools' renderers (`renderCall` / `renderResult` /
//! `renderShell` of `core/tools/*.ts`), keyed by tool name.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_tui_components::Text;
use cortexcode_tui_render::ComponentHandle;

use crate::tool_execution::{RenderShell, ToolRenderDefinition};

pub mod plugins;
pub mod read;
pub mod search;
pub mod subagent;
pub mod web;
pub mod write;

/// `createAllToolDefinitions(cwd)[name]`, rendering half. Tools whose
/// renderers are not ported yet still count as built-in (the block uses its
/// fallbacks), which keeps the shell choice and slot inheritance right.
pub fn builtin_tool_definition(name: &str, _cwd: &str) -> Option<ToolRenderDefinition> {
    match name {
        "read" => Some(read::definition()),
        "write" => Some(write::definition()),
        "SearchCodebase" => Some(search::definition()),
        "webfetch" => Some(web::webfetch_definition()),
        "websearch" => Some(web::websearch_definition()),
        // Renderers ported with the diff and bash work (11.2d).
        "bash" => Some(ToolRenderDefinition::default()),
        "edit" => Some(ToolRenderDefinition {
            render_shell: Some(RenderShell::SelfRendered),
            ..Default::default()
        }),
        _ => None,
    }
}

/// The rendering half of a registered (non built-in) tool's definition: the
/// subagent and plugin tools bring renderers; any other registered tool has
/// none, which still makes the block draw its dot and fallbacks.
pub fn registered_tool_definition(name: &str) -> ToolRenderDefinition {
    match name {
        "Task" => subagent::task_definition(),
        "TaskOutput" => subagent::task_output_definition(),
        n if plugins::RENDERED_PLUGIN_TOOLS.contains(&n) => plugins::definition(),
        _ => ToolRenderDefinition::default(),
    }
}

/// A `Text(text, 0, 0)` component.
pub(crate) fn text(content: String) -> ComponentHandle {
    Rc::new(RefCell::new(Text::new(content, 0, 0)))
}

/// `trimTrailingEmptyLines`.
pub(crate) fn trim_trailing_empty_lines(mut lines: Vec<String>) -> Vec<String> {
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}
