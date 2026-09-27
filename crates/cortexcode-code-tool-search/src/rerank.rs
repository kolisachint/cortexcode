//! `core/search/rerank.ts`: the deterministic reranker over the fused top-50
//! (term coverage with candidate-pool IDF, path affinity, fused prior, exact
//! path and declaration bonuses).

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use crate::lexical::{build_lexical_query_plan, escape_reg_exp};
use crate::types::FusedCandidate;

const WEIGHT_FUSED_PRIOR: f64 = 0.4;
const WEIGHT_TERM_COVERAGE: f64 = 0.35;
const WEIGHT_PATH_AFFINITY: f64 = 0.25;
const EXACT_PATH_BONUS: f64 = 0.5;
const DECLARATION_BONUS: f64 = 0.3;
const PROSE_WORD_THRESHOLD: usize = 2;

const PROSE_FUNCTION_WORDS: &[&str] = &[
    "a", "after", "all", "an", "and", "any", "are", "as", "at", "be", "been", "before", "between",
    "but", "by", "can", "does", "do", "each", "for", "from", "had", "has", "have", "how", "if",
    "in", "into", "is", "it", "its", "of", "on", "one", "or", "should", "so", "than", "that",
    "the", "then", "this", "to", "under", "was", "were", "what", "when", "where", "which", "why",
    "with", "would",
];

const DECLARATION_KEYWORDS: &str =
    "function|class|interface|type|enum|struct|impl|trait|fn|def|const|let|var|namespace|module";

static QUOTED_SEGMENT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"["'`][^"'`]+["'`]"#).expect("quoted pattern"));
static WORD: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[a-z]+").expect("word pattern"));
static PATH_SPLIT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[^a-z0-9_$]+").expect("path split pattern"));

/// `queryIsProse`: two or more sentence function words (a quoted segment is
/// never prose).
pub fn query_is_prose(query: &str) -> bool {
    if QUOTED_SEGMENT.is_match(query) {
        return false;
    }
    let lower = query.to_lowercase();
    WORD.find_iter(&lower)
        .filter(|w| PROSE_FUNCTION_WORDS.contains(&w.as_str()))
        .nth(PROSE_WORD_THRESHOLD - 1)
        .is_some()
}

/// Does `window` declare `term` rather than reference it?
fn declares_term(window: &str, term: &str) -> bool {
    let escaped = escape_reg_exp(term);
    // JS \b and \s are ASCII-only / JS-whitespace; the ASCII word boundary is
    // what matters here.
    let keyword = format!(r"(?-u:\b)(?:{DECLARATION_KEYWORDS})\s+{escaped}(?-u:\b)");
    if regex::Regex::new(&keyword).is_ok_and(|re| re.is_match(window)) {
        return true;
    }
    let member = format!(
        r"(?m)^\s*(?:(?:async|public|private|protected|static|export)\s+)*{escaped}\s*[(<]"
    );
    regex::Regex::new(&member).is_ok_and(|re| re.is_match(window))
}

fn inverse_document_frequency(document_frequency: usize, total: usize) -> f64 {
    (1.0 + total as f64 / document_frequency.max(1) as f64).ln()
}

/// Lines of a file, cached per path (read failures cached as `None`).
struct LineCache<'a> {
    cwd: &'a Path,
    lowercase: bool,
    files: HashMap<String, Option<Vec<String>>>,
}

impl<'a> LineCache<'a> {
    fn new(cwd: &'a Path, lowercase: bool) -> Self {
        Self {
            cwd,
            lowercase,
            files: HashMap::new(),
        }
    }

    fn window(&mut self, candidate: &FusedCandidate) -> Option<String> {
        let (cwd, lowercase) = (self.cwd, self.lowercase);
        let lines = self
            .files
            .entry(candidate.path.clone())
            .or_insert_with(|| {
                let bytes = std::fs::read(cwd.join(&candidate.path)).ok()?;
                let content = String::from_utf8_lossy(&bytes);
                let content = if lowercase {
                    content.to_lowercase()
                } else {
                    content.into_owned()
                };
                Some(content.split('\n').map(str::to_owned).collect())
            })
            .as_ref()?;
        let start = candidate.start_line.saturating_sub(1);
        let end = candidate.end_line.min(lines.len());
        Some(if start < end {
            lines[start..end].join("\n")
        } else {
            String::new()
        })
    }
}

/// `readCandidateWindows`: each candidate's source text (shared with the
/// cross-encoder path).
pub fn read_candidate_windows(candidates: &[FusedCandidate], cwd: &Path) -> Vec<Option<String>> {
    let mut cache = LineCache::new(cwd, false);
    candidates.iter().map(|c| cache.window(c)).collect()
}

/// `rerankCandidates`: score desc, fused order on ties.
pub fn rerank_candidates(
    query: &str,
    candidates: &[FusedCandidate],
    cwd: &Path,
) -> Vec<FusedCandidate> {
    let plan = build_lexical_query_plan(query);
    let Some(plan) = plan.filter(|_| candidates.len() >= 2) else {
        return candidates.to_vec();
    };
    let terms = &plan.terms;
    let query_path = crate::lexical::js_trim(query).to_lowercase();
    let prose = query_is_prose(query);

    let mut cache = LineCache::new(cwd, true);
    let windows: Vec<Option<String>> = candidates.iter().map(|c| cache.window(c)).collect();
    let term_weight: HashMap<&str, f64> = terms
        .iter()
        .map(|t| {
            let df = windows
                .iter()
                .filter(|w| w.as_deref().is_some_and(|w| w.contains(t.as_str())))
                .count();
            (t.as_str(), inverse_document_frequency(df, candidates.len()))
        })
        .collect();
    let total_term_weight: f64 = terms.iter().map(|t| term_weight[t.as_str()]).sum();
    let max_rrf = candidates
        .iter()
        .map(|c| c.hit.rrf_score)
        .fold(f64::MIN_POSITIVE, f64::max);
    // A quoted phrase rarely names a file; split it into path-ish tokens.
    let path_terms: Vec<String> = if terms.len() == 1 {
        PATH_SPLIT
            .split(&terms[0])
            .filter(|t| t.encode_utf16().count() >= 3)
            .map(str::to_owned)
            .collect()
    } else {
        terms.clone()
    };

    let mut scored: Vec<(usize, f64)> = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let fused_prior = candidate.hit.rrf_score / max_rrf;
            let mut term_coverage = 0.0;
            let mut declares_any = false;
            if let Some(window) = windows[index].as_deref().filter(|_| !terms.is_empty()) {
                let present: Vec<&String> = terms
                    .iter()
                    .filter(|t| window.contains(t.as_str()))
                    .collect();
                term_coverage = if total_term_weight > 0.0 {
                    present.iter().map(|t| term_weight[t.as_str()]).sum::<f64>() / total_term_weight
                } else {
                    present.len() as f64 / terms.len() as f64
                };
                declares_any = !prose && present.iter().any(|t| declares_term(window, t));
            }
            let lower_path = candidate.path.to_lowercase();
            let path_affinity = if path_terms.is_empty() {
                0.0
            } else {
                path_terms
                    .iter()
                    .filter(|t| lower_path.contains(t.as_str()))
                    .count() as f64
                    / path_terms.len() as f64
            };
            let exact_path = query_path.encode_utf16().count() >= 3
                && (lower_path == query_path || lower_path.ends_with(&format!("/{query_path}")));
            let score = WEIGHT_FUSED_PRIOR * fused_prior
                + WEIGHT_TERM_COVERAGE * term_coverage
                + WEIGHT_PATH_AFFINITY * path_affinity
                + if exact_path { EXACT_PATH_BONUS } else { 0.0 }
                + if declares_any { DECLARATION_BONUS } else { 0.0 };
            (index, score)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    scored
        .into_iter()
        .map(|(i, _)| candidates[i].clone())
        .collect()
}
