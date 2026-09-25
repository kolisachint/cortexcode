//! Shared plumbing for the built-in tools of the cortex coding agent.
//!
//! Ports `truncate.ts`, `path-utils.ts` and `tool-definition-wrapper.ts` from
//! hoocode's `packages/coding-agent/src/core/tools/`.

pub mod definition;
pub mod fs_error;
pub mod path_utils;
pub mod truncate;

pub use definition::{
    tool_definition_from_agent_tool, wrap_tool_definition, wrap_tool_definitions,
    DefinitionExecuteFn, SessionBranch, ToolContext, ToolContextFactory, ToolDefinition, ToolError,
};
pub use fs_error::{node_fs_error, NodeFsError};
pub use path_utils::{expand_path, resolve_read_path, resolve_to_cwd};
pub use truncate::{
    format_size, truncate_head, truncate_line, truncate_tail, TruncatedBy, TruncationOptions,
    TruncationResult, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, GREP_MAX_LINE_LENGTH,
};

/// Format a number the way JS `String(n)` does for the values tools print
/// (integers without a fraction; other values in shortest round-trip form).
pub fn js_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
    } else if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i128)
    } else {
        format!("{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::js_number;

    #[test]
    fn js_number_formats_like_string_of_number() {
        assert_eq!(js_number(100.0), "100");
        assert_eq!(js_number(-0.0), "0");
        assert_eq!(js_number(2.5), "2.5");
        assert_eq!(js_number(f64::INFINITY), "Infinity");
    }
}
