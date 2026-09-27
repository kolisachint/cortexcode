//! `core/dispatch-evaluator.ts`: the subagent dispatch guard plus a
//! complexity estimate for the dispatch log.
//!
//! The parent agent picks the subagent (via the Task tool), so there is no
//! routing here: only the depth guard and an LLM-free heuristic.

use std::sync::LazyLock;

use regex::Regex;

use crate::depth::{can_spawn_subagent, resolve_max_subagent_depth, ProcessEnv, SubagentEnv};

/// `estimated_complexity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Complexity {
    Low,
    Medium,
    High,
}

impl Complexity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// `TaskAnalysis`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAnalysis {
    /// False only when the depth guard blocks delegation.
    pub should_delegate: bool,
    pub reason: String,
    pub estimated_complexity: Complexity,
}

// JS regexes: ASCII `\w`, `\b` and `\d`.
static FILES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?-u:\b)[A-Za-z0-9_/-]+\.(ts|js|tsx|jsx|py|go|rs|java|cpp|c|h|md|json|yaml|yml|toml)(?-u:\b)",
    )
    .expect("valid regex")
});
static LINES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)([0-9]+)\s*(lines?|loc)(?-u:\b)").expect("valid regex"));
static HIGH_SCOPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?-u:\b)(across|multiple|many|several|all files|rearchitect|redesign|migrate|restructure)(?-u:\b)",
    )
    .expect("valid regex")
});
static MEDIUM_FILES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?-u:\b)(2|3|4|5)\s*files?(?-u:\b)").expect("valid regex"));
static MEDIUM_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?-u:\b)(few|some|couple)(?-u:\b)").expect("valid regex"));

/// Heuristic complexity from file/line/scope mentions in the task.
pub fn estimate_complexity(task: &str) -> Complexity {
    let file_count = FILES.find_iter(task).count();
    let line_count = LINES
        .captures(task)
        .and_then(|c| c[1].parse::<u64>().ok())
        .unwrap_or(0);
    let high_scope = HIGH_SCOPE.is_match(task);
    let medium_scope = MEDIUM_FILES.is_match(task) || MEDIUM_WORDS.is_match(task);

    if line_count > 200 || file_count >= 4 || high_scope {
        Complexity::High
    } else if line_count > 50 || file_count >= 2 || medium_scope {
        Complexity::Medium
    } else {
        Complexity::Low
    }
}

/// `DispatchEvaluator`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DispatchEvaluator;

impl DispatchEvaluator {
    /// Evaluate against the process environment.
    pub fn evaluate(&self, task: &str) -> TaskAnalysis {
        self.evaluate_with_env(task, &ProcessEnv)
    }

    pub fn evaluate_with_env(&self, task: &str, env: &impl SubagentEnv) -> TaskAnalysis {
        if !can_spawn_subagent(None, env) {
            let max_depth = resolve_max_subagent_depth(None, env);
            return TaskAnalysis {
                should_delegate: false,
                // The original message at the default cap; the depth reached
                // when nesting has been opted into.
                reason: if max_depth <= 1 {
                    "Subagents cannot spawn subagents".into()
                } else {
                    format!("Maximum subagent depth ({max_depth}) reached")
                },
                estimated_complexity: Complexity::Low,
            };
        }
        TaskAnalysis {
            should_delegate: true,
            reason: "delegated to subagent".into(),
            estimated_complexity: estimate_complexity(task),
        }
    }
}
