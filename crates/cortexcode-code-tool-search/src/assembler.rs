//! `core/search/context-assembler.ts`: a `path:start-end [sources]` header
//! per candidate, snippets top-down within a token budget.

use std::collections::HashMap;
use std::path::Path;

use crate::types::FusedCandidate;

const CHARS_PER_TOKEN: usize = 4;
const DEFAULT_TOKEN_BUDGET: usize = 2000;
const MAX_SNIPPET_LINES: usize = 20;
const TOP_FULL_SNIPPETS: usize = 3;
const TAIL_SNIPPET_LINES: usize = 8;
const MAX_SNIPPET_LINE_CHARS: usize = 200;

/// `AssembledContext`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledContext {
    pub text: String,
    /// Candidates that got an inline snippet (the rest are bare headers).
    pub snippet_count: usize,
}

fn sources_label(candidate: &FusedCandidate) -> String {
    let mut sources: Vec<&str> = candidate
        .hit
        .ranks
        .iter()
        .map(|(s, _)| s.as_str())
        .collect();
    sources.sort_unstable();
    sources.join("+")
}

/// JS `.length` (UTF-16 units).
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `text.slice(0, n)` in UTF-16 units.
fn js_slice(s: &str, n: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&units)
}

/// `assembleContext`.
pub fn assemble_context(
    candidates: &[FusedCandidate],
    cwd: &Path,
    token_budget: Option<usize>,
) -> AssembledContext {
    let budget_chars = token_budget.unwrap_or(DEFAULT_TOKEN_BUDGET) * CHARS_PER_TOKEN;
    let mut files: HashMap<&str, Option<Vec<String>>> = HashMap::new();
    let mut sections: Vec<String> = Vec::new();
    let mut used = 0;
    let mut snippet_count = 0;
    let mut exhausted = false;

    for candidate in candidates {
        let lines = files
            .entry(candidate.path.as_str())
            .or_insert_with(|| {
                let bytes = std::fs::read(cwd.join(&candidate.path)).ok()?;
                let content = String::from_utf8_lossy(&bytes)
                    .replace("\r\n", "\n")
                    .replace('\r', "\n");
                Some(content.split('\n').map(str::to_owned).collect())
            })
            .as_deref();
        let start = candidate.start_line.max(1);
        let end = match lines {
            Some(lines) => candidate.end_line.min(lines.len()),
            None => candidate.end_line,
        };
        let header = format!(
            "{}:{start}-{end} [{}]",
            candidate.path,
            sources_label(candidate)
        );
        used += js_len(&header) + 1;
        if let (false, Some(lines), true) = (exhausted, lines, end >= start) {
            let depth = if snippet_count < TOP_FULL_SNIPPETS {
                MAX_SNIPPET_LINES
            } else {
                TAIL_SNIPPET_LINES
            };
            let snippet_end = end.min(start + depth - 1);
            let mut raw: Vec<&String> = lines[start - 1..snippet_end].iter().collect();
            while raw
                .last()
                .is_some_and(|l| crate::lexical::js_trim(l).is_empty())
            {
                raw.pop();
            }
            let snippet = raw
                .iter()
                .enumerate()
                .map(|(i, text)| {
                    let shown = if js_len(text) > MAX_SNIPPET_LINE_CHARS {
                        format!("{}…", js_slice(text, MAX_SNIPPET_LINE_CHARS))
                    } else {
                        (*text).clone()
                    };
                    format!("  {}: {shown}", start + i)
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !snippet.is_empty() {
                if used + js_len(&snippet) <= budget_chars {
                    sections.push(format!("{header}\n{snippet}"));
                    used += js_len(&snippet) + 1;
                    snippet_count += 1;
                    continue;
                }
                exhausted = true;
            }
        }
        sections.push(header);
    }
    AssembledContext {
        text: sections.join("\n\n"),
        snippet_count,
    }
}
