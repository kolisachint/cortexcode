//! `core/subagent-depth.ts`: subagent nesting depth and tree-wide bounds.
//!
//! Nesting is governed by environment variables so every process in a
//! delegation tree agrees without cross-process coordination:
//!
//! - `<prefix>SUBAGENT_DEPTH`: this process's depth (root unset/0, children 1).
//! - `<prefix>SUBAGENT_MAX_DEPTH`: the tree-wide cap, seeded once by the root
//!   from its `maxSubagentDepth` setting and inherited by every descendant.
//!
//! `<prefix>` is any of [`cortexcode_code_paths::ENV_PREFIXES`] (hoocode's
//! `HOOCODE_` included), most specific first. The default cap is 1: a subagent
//! may not spawn further subagents. Pools at depth >= 1 run with the nested
//! concurrency cap, so the worst-case live process count is a fixed function of
//! depth and the per-level caps.

use std::collections::HashMap;

/// Current process depth (suffix of `HOOCODE_SUBAGENT_DEPTH`).
pub const SUBAGENT_DEPTH_ENV: &str = "SUBAGENT_DEPTH";
/// Tree-wide max depth (suffix of `HOOCODE_SUBAGENT_MAX_DEPTH`).
pub const SUBAGENT_MAX_DEPTH_ENV: &str = "SUBAGENT_MAX_DEPTH";
/// Nested pool concurrency (suffix of `HOOCODE_NESTED_SUBAGENT_CONCURRENCY`).
pub const NESTED_CONCURRENCY_ENV: &str = "NESTED_SUBAGENT_CONCURRENCY";
/// Comma-separated subagent types this process may delegate to.
pub const DELEGATE_ALLOW_ENV: &str = "DELEGATE_ALLOW";
/// Set by the parent pool when a child's tool allowlist has no MCP tools: the
/// child skips connecting MCP servers at startup.
pub const SUBAGENT_SKIP_MCP_ENV: &str = "SKIP_MCP";
/// "1": the MCP loader defers tool schemas (names only, resolved on demand).
/// Set on the top-level agent, cleared for subagent children.
pub const DEFER_MCP_SCHEMAS_ENV: &str = "DEFER_MCP_SCHEMAS";

/// Default tree-wide cap: subagents cannot spawn subagents.
pub const DEFAULT_MAX_SUBAGENT_DEPTH: u32 = 1;
/// Concurrency cap for pools running at depth >= 1.
pub const NESTED_SUBAGENT_CONCURRENCY: u32 = 2;
/// Hard ceiling on the configurable depth (worst case 5 * (2^3 - 1) = 35
/// processes).
pub const ABSOLUTE_MAX_SUBAGENT_DEPTH: u32 = 3;

/// Where the env-driven helpers read variables from.
pub trait SubagentEnv {
    /// The value of `<prefix><suffix>` for the first prefix that is set.
    fn var(&self, suffix: &str) -> Option<String>;
}

/// The process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl SubagentEnv for ProcessEnv {
    fn var(&self, suffix: &str) -> Option<String> {
        cortexcode_code_paths::ENV_PREFIXES
            .iter()
            .find_map(|prefix| std::env::var(format!("{prefix}{suffix}")).ok())
    }
}

/// An explicit environment (full variable names), for tests and children.
impl SubagentEnv for HashMap<String, String> {
    fn var(&self, suffix: &str) -> Option<String> {
        cortexcode_code_paths::ENV_PREFIXES
            .iter()
            .find_map(|prefix| self.get(&format!("{prefix}{suffix}")).cloned())
    }
}

/// `Number.parseInt(s, 10)`: leading whitespace, a sign, then digits; `None`
/// is `NaN`.
fn parse_int(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let (negative, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let n = digits.parse::<i64>().unwrap_or(i64::MAX);
    Some(if negative { -n } else { n })
}

/// Clamp a requested cap into `[1, ABSOLUTE_MAX_SUBAGENT_DEPTH]`, flooring
/// fractions; a non-finite value is the default cap.
pub fn clamp_max_subagent_depth(n: f64) -> u32 {
    if !n.is_finite() {
        return DEFAULT_MAX_SUBAGENT_DEPTH;
    }
    n.floor().clamp(1.0, ABSOLUTE_MAX_SUBAGENT_DEPTH as f64) as u32
}

/// Depth of the current process (0 = root/main session).
pub fn current_subagent_depth(env: &impl SubagentEnv) -> u32 {
    match env.var(SUBAGENT_DEPTH_ENV).and_then(|v| parse_int(&v)) {
        Some(n) if n > 0 => n.min(u32::MAX as i64) as u32,
        _ => 0,
    }
}

/// Tree-wide max depth: the inherited env value when present, else the
/// setting, clamped so the cap never disables delegation entirely.
pub fn resolve_max_subagent_depth(setting: Option<f64>, env: &impl SubagentEnv) -> u32 {
    if let Some(n) = env.var(SUBAGENT_MAX_DEPTH_ENV).and_then(|v| parse_int(&v)) {
        if n >= 1 {
            return clamp_max_subagent_depth(n as f64);
        }
    }
    match setting {
        Some(n) if n.is_finite() && n >= 1.0 => clamp_max_subagent_depth(n),
        _ => DEFAULT_MAX_SUBAGENT_DEPTH,
    }
}

/// True when a process at the current depth may still spawn subagents.
pub fn can_spawn_subagent(setting: Option<f64>, env: &impl SubagentEnv) -> bool {
    current_subagent_depth(env) < resolve_max_subagent_depth(setting, env)
}

/// Concurrency cap for pools at depth >= 1: the inherited env value, else the
/// setting, else the default; at least 1.
pub fn resolve_nested_concurrency(setting: Option<f64>, env: &impl SubagentEnv) -> u32 {
    if let Some(n) = env.var(NESTED_CONCURRENCY_ENV).and_then(|v| parse_int(&v)) {
        if n >= 1 {
            return n.min(u32::MAX as i64) as u32;
        }
    }
    match setting {
        Some(n) if n.is_finite() && n >= 1.0 => n.floor() as u32,
        _ => NESTED_SUBAGENT_CONCURRENCY,
    }
}

/// Concurrency for a pool created in this process: reduced when nested,
/// `None` (the pool default) at the root.
pub fn pool_concurrency_for_depth(env: &impl SubagentEnv) -> Option<u32> {
    (current_subagent_depth(env) >= 1).then(|| resolve_nested_concurrency(None, env))
}

/// Subagent types this process may delegate to; `None` when unrestricted.
pub fn delegate_allow_list(env: &impl SubagentEnv) -> Option<Vec<String>> {
    let raw = env.var(DELEGATE_ALLOW_ENV)?;
    let list: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    (!list.is_empty()).then_some(list)
}

/// Whether this process may delegate to the given subagent type.
pub fn is_delegate_allowed(subagent_type: &str, env: &impl SubagentEnv) -> bool {
    delegate_allow_list(env).is_none_or(|allow| allow.iter().any(|t| t == subagent_type))
}

/// Whether this process should skip connecting MCP servers at startup.
pub fn subagent_skip_mcp(env: &impl SubagentEnv) -> bool {
    env.var(SUBAGENT_SKIP_MCP_ENV).as_deref() == Some("1")
}

/// Whether this process should defer MCP tool schemas.
pub fn defer_mcp_schemas(env: &impl SubagentEnv) -> bool {
    env.var(DEFER_MCP_SCHEMAS_ENV).as_deref() == Some("1")
}

/// Whether a child with this tool allowlist needs MCP servers: a tool named
/// `mcp_<server>_<tool>` (prefix-tolerant) is MCP-sourced, and no allowlist
/// means "inherit every tool".
pub fn tool_allowlist_needs_mcp(tools: Option<&[String]>) -> bool {
    let Some(tools) = tools else {
        return true;
    };
    tools.iter().any(|t| {
        t.trim()
            .get(..3)
            .is_some_and(|p| p.eq_ignore_ascii_case("mcp"))
    })
}
