//! `core/context-gc.ts`: stub provably dead tool results in the outgoing
//! context (never the persisted session).
//!
//! A `read` result is superseded once the same path is later edited/written
//! successfully, or read again over an overlapping line range; disjoint
//! ranges coexist, and at-call dedup pointers neither supersede nor get
//! stubbed. Under token-budget pressure (>= 0.6) bash output is elided too,
//! except for commands that look side-effecting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{Content, TextContent, ToolResultMessage};

use crate::read_dedup::{
    is_dedup_pointer_text, ranges_overlap, read_range_from_args, ReadRange, WHOLE_FILE_RANGE,
};

/// Commands whose output is never elided (re-running them could repeat a
/// side effect). Deliberately over-broad.
static BASH_SIDE_EFFECT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)((?-u:\b)(write|insert|delete|update|curl|wget|psql|mysql|sqlite3|migrate|drop|truncate|rm|rmdir|mv|cp|dd|kill|tee|chmod|chown|ln|mkdir|touch)(?-u:\b)|(?-u:\b)git\s+(commit|push|reset|checkout|rebase|clean|apply|merge)(?-u:\b)|(?-u:\b)sed(?-u:\b)[^|]*-i|>>?)",
    )
    .expect("side-effect pattern")
});

/// Below 80% pressure, only bash outputs longer than this (UTF-16 units) go.
const BASH_EVICTION_CHAR_THRESHOLD: usize = 2000;

/// `ContextGcOptions`.
#[derive(Debug, Clone, Default)]
pub struct ContextGcOptions {
    /// Resolves relative tool path arguments.
    pub cwd: PathBuf,
    /// Fraction of the context window in use, `[0, 1]`; 0 = read-only GC.
    pub budget_pressure: f64,
}

fn result_text(message: &ToolResultMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect()
}

fn stub(message: &ToolResultMessage, text: String) -> AgentMessage {
    AgentMessage::ToolResult(ToolResultMessage {
        content: vec![Content::Text(TextContent {
            text_signature: None,
            text,
        })],
        ..message.clone()
    })
}

/// JS `Math.round` for non-negative values.
fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

/// `evictSupersededReads`. `None` when nothing changed (hoocode returns the
/// same array).
pub fn evict_superseded_reads(
    messages: &[AgentMessage],
    options: &ContextGcOptions,
) -> Option<Vec<AgentMessage>> {
    let cwd: &Path = &options.cwd;
    let mut path_by_call: HashMap<&str, PathBuf> = HashMap::new();
    let mut display_by_path: HashMap<PathBuf, String> = HashMap::new();
    let mut bash_command_by_call: HashMap<&str, &str> = HashMap::new();
    let mut range_by_call: HashMap<&str, ReadRange> = HashMap::new();

    for message in messages {
        let AgentMessage::Assistant(assistant) = message else {
            continue;
        };
        for block in &assistant.content {
            let Content::ToolCall(call) = block else {
                continue;
            };
            if call.id.is_empty() {
                continue;
            }
            if call.name == "bash" {
                if let Some(cmd) = call.arguments.get("command").and_then(|v| v.as_str()) {
                    if !cmd.is_empty() {
                        bash_command_by_call.insert(&call.id, cmd);
                    }
                }
            }
            let Some(raw) = call.arguments.get("path").and_then(|v| v.as_str()) else {
                continue;
            };
            if raw.is_empty() {
                continue;
            }
            let resolved = cortexcode_code_tool_api::path_utils::node_resolve(cwd, raw);
            if call.name == "read" {
                range_by_call.insert(&call.id, read_range_from_args(Some(&call.arguments)));
            }
            display_by_path
                .entry(resolved.clone())
                .or_insert_with(|| raw.to_owned());
            path_by_call.insert(&call.id, resolved);
        }
    }

    // Per path: every successful read (index, range) and the last mutate.
    let mut reads_by_path: HashMap<&PathBuf, Vec<(usize, ReadRange)>> = HashMap::new();
    let mut last_mutate: HashMap<&PathBuf, usize> = HashMap::new();
    for (i, message) in messages.iter().enumerate() {
        let AgentMessage::ToolResult(result) = message else {
            continue;
        };
        if result.is_error {
            continue;
        }
        let Some(path) = path_by_call.get(result.tool_call_id.as_str()) else {
            continue;
        };
        match result.tool_name.as_str() {
            "read" => {
                if is_dedup_pointer_text(&result_text(result)) {
                    continue;
                }
                let range = range_by_call
                    .get(result.tool_call_id.as_str())
                    .copied()
                    .unwrap_or(WHOLE_FILE_RANGE);
                reads_by_path.entry(path).or_default().push((i, range));
            }
            "edit" | "write" => {
                last_mutate.insert(path, i);
            }
            _ => {}
        }
    }

    let pressure = options.budget_pressure;
    let mut changed = false;
    let out: Vec<AgentMessage> = messages
        .iter()
        .enumerate()
        .map(|(i, message)| {
            let AgentMessage::ToolResult(result) = message else {
                return message.clone();
            };
            if result.is_error {
                return message.clone();
            }
            if result.tool_name == "read" {
                let Some(path) = path_by_call.get(result.tool_call_id.as_str()) else {
                    return message.clone();
                };
                if is_dedup_pointer_text(&result_text(result)) {
                    return message.clone();
                }
                let range = range_by_call
                    .get(result.tool_call_id.as_str())
                    .copied()
                    .unwrap_or(WHOLE_FILE_RANGE);
                let later_mutate = last_mutate.get(path).is_some_and(|&m| m > i);
                let later_read = reads_by_path
                    .get(path)
                    .is_some_and(|reads| reads.iter().any(|&(j, r)| j > i && ranges_overlap(r, range)));
                if !later_read && !later_mutate {
                    return message.clone();
                }
                changed = true;
                let display = display_by_path
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                let reason = if later_mutate {
                    "the file was modified after this read"
                } else {
                    "the file was read again later"
                };
                return stub(
                    result,
                    format!("[Superseded read of {display} elided to save context — {reason}. Re-read the file if you need its current contents.]"),
                );
            }
            if pressure >= 0.6 && result.tool_name == "bash" {
                let cmd = bash_command_by_call
                    .get(result.tool_call_id.as_str())
                    .copied()
                    .unwrap_or("");
                if BASH_SIDE_EFFECT.is_match(cmd) {
                    return message.clone();
                }
                let text_len: usize = result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text(t) => Some(t.text.encode_utf16().count()),
                        _ => None,
                    })
                    .sum();
                if pressure >= 0.8 || text_len > BASH_EVICTION_CHAR_THRESHOLD {
                    changed = true;
                    return stub(
                        result,
                        format!(
                            "[Bash output elided at {}% token budget — re-run if needed.]",
                            js_round(pressure * 100.0)
                        ),
                    );
                }
            }
            message.clone()
        })
        .collect();
    changed.then_some(out)
}

/// The budget-pressure gauge with its high-water latch (sdk.ts
/// `getBudgetPressure`): tracks rising usage, resets after a drop of more
/// than 0.15 (compaction, fork), so evictions cannot oscillate.
#[derive(Debug, Default)]
pub struct BudgetPressureLatch {
    high_water: f64,
}

impl BudgetPressureLatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// `tokens / context_window`, capped at 1, latched.
    pub fn update(&mut self, tokens: u64, context_window: u64) -> f64 {
        if context_window == 0 {
            return 0.0;
        }
        let gauge = (tokens as f64 / context_window as f64).min(1.0);
        if gauge > self.high_water || gauge < self.high_water - 0.15 {
            self.high_water = gauge;
        }
        self.high_water
    }
}
