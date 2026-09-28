//! `components/tool-chain-summary.ts`: the radar view's chain line.
//!
//! While a run is going it shows its shape, in order; once it is over, what it
//! amounted to:
//!
//! ```text
//! ◐ grep › read › bash✗ › edit › bash…            4 done · 1 failed · running
//! ● Edited packages/tui/src/keys.ts               5 calls · 1 failed
//! ```
//!
//! Everything here is pure and deterministic.

use std::collections::BTreeSet;

use once_cell::sync::Lazy;
use regex::Regex;

/// One tool call's contribution to its chain.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChainEntry {
    pub tool: String,
    /// The call's subject — a path, a command, a pattern.
    pub subject: String,
    pub is_error: bool,
    /// Still executing.
    pub is_partial: bool,
    /// Lines of text output the call returned.
    pub output_lines: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainState {
    /// Calls are still arriving or executing.
    Running,
    /// Every call finished and the turn moved on cleanly.
    Done,
    /// The turn was aborted, errored, or hit a length cap partway through.
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentTone {
    Ok,
    Error,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSegment {
    pub label: String,
    pub tone: SegmentTone,
}

const MAX_SEGMENTS: usize = 8;
const HEAD_SEGMENTS: usize = 3;
const TAIL_SEGMENTS: usize = 2;

fn tone(entry: &ChainEntry) -> SegmentTone {
    if entry.is_error {
        SegmentTone::Error
    } else if entry.is_partial {
        SegmentTone::Running
    } else {
        SegmentTone::Ok
    }
}

/// Collapse consecutive successful calls to the same tool into one segment.
fn collapse_repeats(entries: &[ChainEntry]) -> Vec<ChainSegment> {
    let mut segments = Vec::new();
    let mut index = 0;
    while index < entries.len() {
        let entry = &entries[index];
        let entry_tone = tone(entry);
        let mut run = 1;
        while index + run < entries.len()
            && entries[index + run].tool == entry.tool
            && tone(&entries[index + run]) == entry_tone
            && entry_tone == SegmentTone::Ok
        {
            run += 1;
        }
        let marker = if entry_tone == SegmentTone::Error {
            "✗"
        } else {
            ""
        };
        segments.push(ChainSegment {
            label: if run > 1 {
                format!("{} ×{run}", entry.tool)
            } else {
                format!("{}{marker}", entry.tool)
            },
            tone: entry_tone,
        });
        index += run;
    }
    segments
}

/// Elide the middle of a long chain; failures are never elided.
fn cap_segments(segments: Vec<ChainSegment>) -> Vec<ChainSegment> {
    if segments.len() <= MAX_SEGMENTS {
        return segments;
    }
    let mut keep = BTreeSet::new();
    keep.extend(0..HEAD_SEGMENTS);
    keep.extend(segments.len() - TAIL_SEGMENTS..segments.len());
    for (i, s) in segments.iter().enumerate() {
        if s.tone == SegmentTone::Error {
            keep.insert(i);
        }
    }
    let mut out = Vec::new();
    let mut gap = 0;
    let flush = |out: &mut Vec<ChainSegment>, gap: &mut usize| {
        if *gap > 0 {
            out.push(ChainSegment {
                label: format!("… {gap} more …"),
                tone: SegmentTone::Ok,
            });
        }
        *gap = 0;
    };
    for (i, segment) in segments.into_iter().enumerate() {
        if keep.contains(&i) {
            flush(&mut out, &mut gap);
            out.push(segment);
        } else {
            gap += 1;
        }
    }
    flush(&mut out, &mut gap);
    out
}

/// The `grep › read ×3 › bash✗` part of a running chain line.
pub fn chain_segments(entries: &[ChainEntry]) -> Vec<ChainSegment> {
    cap_segments(collapse_repeats(entries))
}

fn plural(n: usize, one: &str) -> String {
    format!(
        "{n} {}",
        if n == 1 {
            one.to_string()
        } else {
            format!("{one}s")
        }
    )
}

/// The flush-right stats for either rendering.
pub fn chain_stats(entries: &[ChainEntry], state: ChainState) -> String {
    let failed = entries.iter().filter(|e| e.is_error).count();
    let mut parts = Vec::new();
    if state == ChainState::Running {
        parts.push(format!(
            "{} done",
            entries.iter().filter(|e| !e.is_partial).count()
        ));
    } else {
        parts.push(plural(entries.len(), "call"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    match state {
        ChainState::Running => parts.push("running".into()),
        ChainState::Interrupted => parts.push("interrupted".into()),
        ChainState::Done => {
            let lines: usize = entries.iter().map(|e| e.output_lines).sum();
            if lines > 0 {
                parts.push(plural(lines, "line"));
            }
        }
    }
    parts.join(" · ")
}

struct ToolFamily {
    verb: &'static str,
    noun: &'static str,
    tail: &'static str,
    tools: &'static [&'static str],
    subject_is_path: bool,
    act: Option<fn(&str) -> String>,
}

/// Leading `cd <dir> &&` runs, however many are chained.
static CD_PREFIX_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^(?:cd\s+(?:"[^"]*"|'[^']*'|[^\s;&|]+)\s*&&\s*)+"#).expect("cd prefix")
});

/// A command's act, with the navigation stripped off the front.
fn command_act(subject: &str) -> String {
    let stripped = CD_PREFIX_RE.replace(subject, "");
    let trimmed = cortexcode_tui_components::markdown::js_trim(&stripped);
    if trimmed.is_empty() {
        subject.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Tool families, in the order a settled chain reports them.
const FAMILIES: [ToolFamily; 6] = [
    ToolFamily {
        verb: "Edited",
        noun: "file",
        tail: "edit",
        tools: &["edit", "write", "MultiEdit", "NotebookEdit"],
        subject_is_path: true,
        act: None,
    },
    ToolFamily {
        verb: "Ran",
        noun: "command",
        tail: "command",
        tools: &["bash"],
        subject_is_path: false,
        act: Some(command_act),
    },
    ToolFamily {
        verb: "Delegated",
        noun: "task",
        tail: "task",
        tools: &["Task", "TaskOutput"],
        subject_is_path: false,
        act: None,
    },
    ToolFamily {
        verb: "Fetched",
        noun: "page",
        tail: "fetch",
        tools: &["webfetch", "websearch"],
        subject_is_path: false,
        act: None,
    },
    ToolFamily {
        verb: "Read",
        noun: "file",
        tail: "read",
        tools: &["read"],
        subject_is_path: true,
        act: None,
    },
    ToolFamily {
        verb: "Searched",
        noun: "search",
        tail: "search",
        tools: &["SearchCodebase"],
        subject_is_path: true,
        act: None,
    },
];

/// Calls from here up, a chain is long enough that priority order alone starts lying.
const LONG_CHAIN_CALLS: usize = 10;
/// A family this far below the chain's own size did not characterise it.
const INCIDENTAL_SHARE: f64 = 0.1;
/// Below this, a secondary family is a footnote.
const MIN_SECONDARY_CALLS: usize = 3;

/// Longest common directory prefix of the paths, or `""` when they share none.
fn common_path_prefix(paths: &[String], long: bool) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let split: Vec<Vec<&str>> = paths
        .iter()
        .map(|p| p.split('/').filter(|s| !s.is_empty()).collect())
        .collect();
    let (first, rest) = split.split_first().unwrap();
    let mut prefix = Vec::new();
    for (i, part) in first.iter().enumerate() {
        if rest.iter().all(|parts| parts.get(i) == Some(part)) {
            prefix.push(*part);
        } else {
            break;
        }
    }
    if prefix.is_empty() || (long && prefix.len() < 2) {
        return String::new();
    }
    let joined = prefix.join("/");
    if paths[0].starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Whether a subject can stand in for a location (globs cannot).
fn usable_as_target(subject: &str) -> bool {
    if subject.contains(['*', '?']) {
        return false;
    }
    if subject.contains('/') {
        return true;
    }
    match subject.rfind('.') {
        Some(dot) => {
            let ext = &subject[dot + 1..];
            !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

fn family_phrase(family: &ToolFamily, matched: &[&ChainEntry], long: bool) -> String {
    let subjects: Vec<String> = matched
        .iter()
        .map(|e| match family.act {
            Some(act) => act(&e.subject),
            None => e.subject.clone(),
        })
        .filter(|s| !s.is_empty())
        .collect();
    let distinct: BTreeSet<&String> = subjects.iter().collect();
    if subjects.len() == matched.len() && distinct.len() == 1 {
        return format!("{} {}", family.verb, subjects[0]);
    }
    let target = if family.subject_is_path {
        let usable: Vec<String> = subjects
            .iter()
            .filter(|s| usable_as_target(s))
            .cloned()
            .collect();
        common_path_prefix(&usable, long)
    } else {
        String::new()
    };
    if !target.is_empty() {
        return format!("{} {target}", family.verb);
    }
    if family.noun == "search" {
        return "Explored".into();
    }
    format!("{} {}", family.verb, plural(matched.len(), family.noun))
}

fn family_matches<'a>(entries: &'a [ChainEntry], family: &ToolFamily) -> Vec<&'a ChainEntry> {
    entries
        .iter()
        .filter(|e| family.tools.contains(&e.tool.as_str()))
        .collect()
}

/// The family with the most calls other than `chosen`, if any reaches the floor.
fn largest_other_family(entries: &[ChainEntry], chosen: usize) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for (i, family) in FAMILIES.iter().enumerate() {
        if i == chosen {
            continue;
        }
        let count = family_matches(entries, family).len();
        if count > best.map_or(0, |b| b.1) {
            best = Some((i, count));
        }
    }
    best.filter(|b| b.1 >= MIN_SECONDARY_CALLS)
}

/// The settled chain's phrase: what this run amounted to.
pub fn chain_phrase(entries: &[ChainEntry]) -> String {
    if entries.is_empty() {
        return "No calls".into();
    }
    let long = entries.len() >= LONG_CHAIN_CALLS;

    let mut chosen: Option<usize> = None;
    let mut matched: Vec<&ChainEntry> = Vec::new();
    for (i, family) in FAMILIES.iter().enumerate() {
        let m = family_matches(entries, family);
        if m.is_empty() {
            continue;
        }
        if long && (m.len() as f64 / entries.len() as f64) < INCIDENTAL_SHARE {
            continue;
        }
        chosen = Some(i);
        matched = m;
        break;
    }
    // Every family was incidental: the largest describes it best.
    if chosen.is_none() {
        for (i, family) in FAMILIES.iter().enumerate() {
            let m = family_matches(entries, family);
            if m.len() > matched.len() {
                chosen = Some(i);
                matched = m;
            }
        }
    }
    let Some(chosen) = chosen else {
        let mut names: Vec<&str> = Vec::new();
        for e in entries {
            if !names.contains(&e.tool.as_str()) {
                names.push(&e.tool);
            }
        }
        return if names.len() == 1 {
            format!("Called {}", names[0])
        } else {
            format!("Called {}", plural(names.len(), "tool"))
        };
    };

    let head = family_phrase(&FAMILIES[chosen], &matched, long);
    if !long || matched.len() as f64 / entries.len() as f64 >= 0.5 {
        return head;
    }
    match largest_other_family(entries, chosen) {
        Some((family, count)) => format!("{head} · {}", plural(count, FAMILIES[family].tail)),
        None => head,
    }
}
