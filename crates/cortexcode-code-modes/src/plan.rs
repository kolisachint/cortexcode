//! The plan-file half of `extensions/core/modes.ts`: section parsing and the
//! `/approve`, `/grill` and `/goal` messages.

use std::path::{Path, PathBuf};

/// `AUTO_LOOP_DONE_TOKEN` (extensions/core/loop.ts).
pub const AUTO_LOOP_DONE_TOKEN: &str = "LOOP_DONE";

/// `PlanSections`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanSections {
    pub goal: Option<String>,
    pub files_to_modify: Option<String>,
    pub new_files: Option<String>,
    pub tests: Option<String>,
    pub verification: Option<String>,
    /// The whole plan, used when no section was recognised.
    pub raw: String,
}

/// JavaScript `\s`.
fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// JavaScript `.` (anything but a line terminator).
fn is_dot(c: char) -> bool {
    !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// JavaScript `^` in multiline mode.
fn at_line_start(t: &[char], i: usize) -> bool {
    i == 0 || matches!(t[i - 1], '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `\s*\n` at `p` (greedy): the end position (after the last `\n` of the
/// whitespace run), if the run contains a `\n`.
fn space_newline(t: &[char], p: usize) -> Option<usize> {
    let mut end = p;
    while end < t.len() && is_space(t[end]) {
        end += 1;
    }
    (p..end).rev().find(|&j| t[j] == '\n').map(|j| j + 1)
}

/// `#{1,3}\s+` at `p`.
fn hashes_then_space(t: &[char], p: usize) -> bool {
    let hashes = t[p..].iter().take(3).take_while(|&&c| c == '#').count();
    (1..=hashes).any(|k| p + k < t.len() && is_space(t[p + k]))
}

/// `\*\*[^*\n]+\*\*\s*\n` at `p`.
fn bold_line(t: &[char], p: usize) -> bool {
    if !(p + 1 < t.len() && t[p] == '*' && t[p + 1] == '*') {
        return false;
    }
    let mut j = p + 2;
    while j < t.len() && t[j] != '*' && t[j] != '\n' {
        j += 1;
    }
    j > p + 2
        && j + 1 < t.len()
        && t[j] == '*'
        && t[j + 1] == '*'
        && space_newline(t, j + 2).is_some()
}

/// The content lookahead: a heading line, a bold label, or the end of a line.
fn content_ends_at(t: &[char], e: usize) -> bool {
    (at_line_start(t, e) && hashes_then_space(t, e))
        || bold_line(t, e)
        || e == t.len()
        || matches!(t[e], '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The heading part at `i`: (heading text, index after `\s*\n`).
fn match_heading(t: &[char], i: usize) -> Option<(String, usize)> {
    if !at_line_start(t, i) {
        return None;
    }
    // `#{1,3}\s+(.+?)\s*\n`, with the regex engine's backtracking order.
    let hashes = t[i..].iter().take(3).take_while(|&&c| c == '#').count();
    for k in (1..=hashes).rev() {
        let s = i + k;
        let mut run = 0;
        while s + run < t.len() && is_space(t[s + run]) {
            run += 1;
        }
        for w in (1..=run).rev() {
            let h = s + w;
            let mut l = 1;
            while h + l <= t.len() && is_dot(t[h + l - 1]) {
                if let Some(end) = space_newline(t, h + l) {
                    return Some((t[h..h + l].iter().collect(), end));
                }
                l += 1;
            }
        }
    }
    // `\*\*(.+?)\*\*\s*\n`.
    if i + 1 < t.len() && t[i] == '*' && t[i + 1] == '*' {
        let h = i + 2;
        let mut l = 1;
        while h + l <= t.len() && is_dot(t[h + l - 1]) {
            let close = h + l;
            if close + 1 < t.len() && t[close] == '*' && t[close + 1] == '*' {
                if let Some(end) = space_newline(t, close + 2) {
                    return Some((t[h..close].iter().collect(), end));
                }
            }
            l += 1;
        }
    }
    None
}

/// `parsePlanSections`: `## Heading` / `**Heading**` sections (the first line
/// of each, as hoocode's multiline regex captures it).
pub fn parse_plan_sections(plan_content: &str) -> PlanSections {
    let mut result = PlanSections {
        raw: plan_content.to_string(),
        ..Default::default()
    };
    let t: Vec<char> = plan_content.chars().collect();
    let mut i = 0;
    while i < t.len() {
        let Some((heading, c)) = match_heading(&t, i) else {
            i += 1;
            continue;
        };
        let mut e = c;
        while !content_ends_at(&t, e) {
            e += 1;
        }
        let heading = heading.to_lowercase().trim().to_string();
        let content: String = t[c..e].iter().collect::<String>().trim().to_string();
        i = e.max(i + 1);
        if content.is_empty() {
            continue;
        }
        let slot = if heading.starts_with("goal") {
            &mut result.goal
        } else if is_files_to_modify(&heading) {
            &mut result.files_to_modify
        } else if is_new_files(&heading) {
            &mut result.new_files
        } else if heading.starts_with("test") {
            &mut result.tests
        } else if heading.starts_with("verif") {
            &mut result.verification
        } else {
            continue;
        };
        *slot = Some(content);
    }
    result
}

/// `/files?\s+to\s+modif|^modif/`.
fn is_files_to_modify(heading: &str) -> bool {
    if heading.starts_with("modif") {
        return true;
    }
    heading.match_indices("file").any(|(i, _)| {
        let rest = &heading[i + 4..];
        let rest = rest.strip_prefix('s').unwrap_or(rest);
        let after_space = rest.trim_start_matches(is_space);
        after_space.len() < rest.len()
            && after_space.strip_prefix("to").is_some_and(|r| {
                let r2 = r.trim_start_matches(is_space);
                r2.len() < r.len() && r2.starts_with("modif")
            })
    })
}

/// `/new\s+files?/`.
fn is_new_files(heading: &str) -> bool {
    heading.match_indices("new").any(|(i, _)| {
        let rest = &heading[i + 3..];
        let after = rest.trim_start_matches(is_space);
        after.len() < rest.len() && after.starts_with("file")
    })
}

/// `buildApproveMessage`.
pub fn build_approve_message(sections: &PlanSections) -> String {
    let mut steps = Vec::new();
    if let Some(goal) = &sections.goal {
        steps.push(format!("**Goal:** {goal}"));
    }
    if let Some(s) = &sections.files_to_modify {
        steps.push(format!("**Step 1 — Modify existing files:**\n{s}"));
    }
    if let Some(s) = &sections.new_files {
        steps.push(format!("**Step 2 — Create new files:**\n{s}"));
    }
    if let Some(s) = &sections.tests {
        steps.push(format!("**Step 3 — Update tests:**\n{s}"));
    }
    if let Some(s) = &sections.verification {
        steps.push(format!("**Step 4 — Verify:**\n{s}"));
    }
    if steps.is_empty() {
        return format!("Execute the following plan:\n\n{}", sections.raw);
    }
    format!(
        "Execute this plan step by step. Complete each step fully before moving to the next.\n\n{}",
        steps.join("\n\n")
    )
}

/// `GrillTarget`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrillTarget {
    Both,
    Me,
    Plan,
}

/// `parseGrillTarget`: bare = both, `me` / `plan`, anything else `None`.
pub fn parse_grill_target(args: &str) -> Option<GrillTarget> {
    match args.trim().to_lowercase().as_str() {
        "" => Some(GrillTarget::Both),
        "me" => Some(GrillTarget::Me),
        "plan" => Some(GrillTarget::Plan),
        _ => None,
    }
}

fn render_plan_for_review(sections: &PlanSections) -> String {
    let mut parts = Vec::new();
    for (label, value) in [
        ("Goal", &sections.goal),
        ("Files to modify", &sections.files_to_modify),
        ("New files", &sections.new_files),
        ("Tests", &sections.tests),
        ("Verification", &sections.verification),
    ] {
        if let Some(v) = value {
            parts.push(format!("**{label}**\n{v}"));
        }
    }
    if parts.is_empty() {
        sections.raw.clone()
    } else {
        parts.join("\n\n")
    }
}

/// `buildGrillMessage`: questions first, then the critique, then the plan.
pub fn build_grill_message(sections: &PlanSections, target: GrillTarget) -> String {
    let plan = render_plan_for_review(sections);
    let me = crate::prompts::grill_prompt("grill-me").trim();
    let critique = crate::prompts::grill_prompt("grill-plan").trim();
    match target {
        GrillTarget::Me => format!("{me}\n\n---\n\n{plan}"),
        GrillTarget::Plan => format!("{critique}\n\n---\n\n{plan}"),
        GrillTarget::Both => format!(
            "{me}\n\n{}\n\n{critique}\n\n---\n\n{plan}",
            crate::prompts::grill_prompt("grill-bridge").trim()
        ),
    }
}

/// `GoalInvocation`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoalInvocation {
    pub objective: Option<String>,
    pub max_turns: Option<u64>,
}

/// `parseGoalArgs`: `[--max-turns N] [objective]`; a malformed budget is `None`.
pub fn parse_goal_args(args: &str) -> Option<GoalInvocation> {
    let mut rest = args.trim().to_string();
    let mut max_turns = None;
    if let Some(after) = rest.strip_prefix("--max-turns") {
        // `/^--max-turns(?:\s+(\S+))?([\s\S]*)$/`
        let trimmed = after.trim_start_matches(is_space);
        let raw: String = if trimmed.len() < after.len() {
            trimmed.chars().take_while(|c| !is_space(*c)).collect()
        } else {
            String::new()
        };
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        max_turns = Some(raw.parse().unwrap_or(u64::MAX));
        rest = trimmed[raw.len()..].trim().to_string();
    }
    Some(GoalInvocation {
        objective: (!rest.is_empty()).then_some(rest),
        max_turns,
    })
}

/// `GoalMessages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalMessages {
    pub task: String,
    pub continue_prompt: String,
}

/// `buildGoalMessages`: a verification section becomes the completion condition.
pub fn build_goal_messages(objective: &str, verification: Option<&str>) -> GoalMessages {
    let condition = verification
        .map(|v| format!("\n\nThe goal counts as complete only once this verification passes. Run it, and if it fails, fix what it reports and run it again:\n\n{v}"))
        .unwrap_or_default();
    GoalMessages {
        task: format!("Work autonomously toward this goal:\n\n{objective}{condition}"),
        continue_prompt: format!(
            "Keep working toward the goal:\n\n{objective}{condition}\n\nReply with {AUTO_LOOP_DONE_TOKEN} only when it is genuinely complete — not when it is merely close."
        ),
    }
}

/// `getPlanPath`: `.cortexcode/plans/<sessionId>.md`.
pub fn plan_path(cwd: &Path, session_id: &str) -> PathBuf {
    cwd.join(cortexcode_code_paths::CONFIG_DIR_NAME)
        .join("plans")
        .join(format!("{session_id}.md"))
}

/// `getLegacyPlanPath`: `.cortexcode/plan.md`.
pub fn legacy_plan_path(cwd: &Path) -> PathBuf {
    cwd.join(cortexcode_code_paths::CONFIG_DIR_NAME)
        .join("plan.md")
}

/// `PlanLoadResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanLoad {
    Loaded(PlanSections),
    Missing,
    Error(PathBuf),
}

/// `loadPlanSections`: the first existing, non-empty candidate.
pub fn load_plan_sections(candidates: &[PathBuf]) -> PlanLoad {
    for path in candidates {
        if !path.exists() {
            continue;
        }
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                let raw = raw.trim();
                if !raw.is_empty() {
                    return PlanLoad::Loaded(parse_plan_sections(raw));
                }
            }
            Err(_) => return PlanLoad::Error(path.clone()),
        }
    }
    PlanLoad::Missing
}
