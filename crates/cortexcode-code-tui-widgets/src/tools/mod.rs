//! The built-in tools' renderers (`renderCall` / `renderResult` /
//! `renderShell` of `core/tools/*.ts`), keyed by tool name.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_tui_components::Text;
use cortexcode_tui_render::ComponentHandle;

use crate::tool_execution::{RenderShell, ToolRenderDefinition};

pub mod read;
pub mod search;
pub mod write;

/// `createAllToolDefinitions(cwd)[name]`, rendering half. Tools whose
/// renderers are not ported yet still count as built-in (the block uses its
/// fallbacks), which keeps the shell choice and slot inheritance right.
pub fn builtin_tool_definition(name: &str, _cwd: &str) -> Option<ToolRenderDefinition> {
    match name {
        "read" => Some(read::definition()),
        "write" => Some(write::definition()),
        "SearchCodebase" => Some(search::definition()),
        // Renderers ported with the diff and bash work (11.2d) and the web
        // tools (11.2c2).
        "bash" | "webfetch" | "websearch" => Some(ToolRenderDefinition::default()),
        "edit" => Some(ToolRenderDefinition {
            render_shell: Some(RenderShell::SelfRendered),
            ..Default::default()
        }),
        _ => None,
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
