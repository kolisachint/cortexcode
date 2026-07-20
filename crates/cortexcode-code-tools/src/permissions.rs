//! Permission policy for dangerous operations.

/// Policy controlling whether dangerous tools require explicit approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionPolicy {
    /// Always require user approval for dangerous tools.
    #[default]
    Ask,
    /// Auto-approve dangerous tools.
    Auto,
    /// Reject dangerous tools entirely.
    Deny,
}

/// Whether a tool name is considered dangerous.
pub fn is_dangerous(tool_name: &str) -> bool {
    matches!(tool_name, "bash" | "write" | "edit")
}
