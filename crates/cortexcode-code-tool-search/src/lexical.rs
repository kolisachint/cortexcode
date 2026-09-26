//! `core/search/lexical-retriever.ts`: a query becomes a case-insensitive
//! pattern, and matching lines come back as `rel:line` hits.
//!
//! hoocode runs `rg --json --line-number --hidden --no-require-git
//! --ignore-case --sort path --glob '!**/.git/**' [--glob G] -- PATTERN DIR`.
//! This runs the same search in-process with ripgrep's own crates: the
//! `ignore` walker (ignore files, hidden files, overrides, sorted by file
//! name) and `grep-searcher` (binary detection, BOM sniffing).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use cortexcode_agent_harness::frontmatter::locale_compare;
use cortexcode_ai_types::AbortSignal;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};

use crate::adapter::GrepLineHit;

/// Terms considered per query (longest first).
const MAX_TERMS: usize = 4;
/// Minimum token length worth matching on.
const MIN_TERM_LENGTH: usize = 3;

/// `LexicalQueryPlan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalQueryPlan {
    /// Regex pattern (terms escaped, OR-ed).
    pub pattern: String,
    /// Lowercased raw terms, for per-line attribution.
    pub terms: Vec<String>,
}

/// JS `escapeRegExp`: `.*+?^${}()|[]\` get a backslash.
pub(crate) fn escape_reg_exp(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if ".*+?^${}()|[]\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// JS `trim()`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

static QUOTED: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"["'`]([^"'`]+)["'`]"#).expect("quoted pattern"));
static TOKEN: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z0-9_$][A-Za-z0-9_$.-]*").expect("token pattern"));

/// `buildLexicalQueryPlan`: the longest quoted segment verbatim, else the
/// longest few identifier-ish tokens OR-ed; `None` when nothing is
/// searchable.
pub fn build_lexical_query_plan(query: &str) -> Option<LexicalQueryPlan> {
    let mut quoted: Vec<&str> = QUOTED
        .captures_iter(query)
        .map(|c| js_trim(c.get(1).expect("group").as_str()))
        .filter(|s| !s.is_empty())
        .collect();
    quoted.sort_by_key(|s| std::cmp::Reverse(js_len(s)));
    if let Some(quoted) = quoted.first() {
        return Some(LexicalQueryPlan {
            pattern: escape_reg_exp(quoted),
            terms: vec![quoted.to_lowercase()],
        });
    }
    let mut tokens: Vec<&str> = Vec::new();
    for m in TOKEN.find_iter(query) {
        if !tokens.contains(&m.as_str()) {
            tokens.push(m.as_str());
        }
    }
    tokens.retain(|t| js_len(t) >= MIN_TERM_LENGTH);
    tokens.sort_by(|a, b| js_len(b).cmp(&js_len(a)).then_with(|| locale_compare(a, b)));
    tokens.truncate(MAX_TERMS);
    if tokens.is_empty() {
        let trimmed = js_trim(query);
        return (!trimmed.is_empty()).then(|| LexicalQueryPlan {
            pattern: escape_reg_exp(trimmed),
            terms: vec![trimmed.to_lowercase()],
        });
    }
    Some(LexicalQueryPlan {
        pattern: tokens
            .iter()
            .map(|t| escape_reg_exp(t))
            .collect::<Vec<_>>()
            .join("|"),
        terms: tokens.iter().map(|t| t.to_lowercase()).collect(),
    })
}

/// `buildLexicalPattern`.
pub fn build_lexical_pattern(query: &str) -> Option<String> {
    build_lexical_query_plan(query).map(|p| p.pattern)
}

fn terms_on_line(plan: &LexicalQueryPlan, line: &str) -> Vec<String> {
    if line.is_empty() {
        return Vec::new();
    }
    let lower = line.to_lowercase();
    plan.terms
        .iter()
        .filter(|t| lower.contains(t.as_str()))
        .cloned()
        .collect()
}

/// `normalizeSearchGlob`: a slash glob is anchored anywhere in the tree.
pub fn normalize_search_glob(glob: Option<&str>) -> Option<String> {
    let glob = glob.filter(|g| !g.is_empty())?;
    if glob.contains('/') && !glob.starts_with('/') && !glob.starts_with("**/") {
        Some(format!("**/{glob}"))
    } else {
        Some(glob.to_owned())
    }
}

/// `RunLexicalOptions`.
#[derive(Debug, Clone)]
pub struct RunLexicalOptions<'a> {
    pub cwd: &'a Path,
    pub query: &'a str,
    pub limit: usize,
    /// Raw glob (normalized here).
    pub glob: Option<&'a str>,
    pub signal: Option<AbortSignal>,
    /// Search only these repo-relative files; empty means nothing to search.
    pub paths: Option<&'a [String]>,
}

fn aborted(signal: &Option<AbortSignal>) -> bool {
    signal.as_ref().is_some_and(AbortSignal::aborted)
}

/// `path.relative(cwd, file)` with `/` separators, or the path itself when
/// it lies outside `cwd`.
fn to_rel(cwd: &Path, file: &Path) -> String {
    let rel = match file.strip_prefix(cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_path_buf(),
        _ => file.to_path_buf(),
    };
    rel.to_string_lossy().replace('\\', "/")
}

/// `runLexicalRetriever`: up to `limit` line hits in ripgrep's output order.
pub fn run_lexical_retriever(options: RunLexicalOptions<'_>) -> Result<Vec<GrepLineHit>, String> {
    let glob = normalize_search_glob(options.glob);
    let Some(plan) = build_lexical_query_plan(options.query) else {
        return Ok(Vec::new());
    };
    if options.paths.is_some_and(<[String]>::is_empty) {
        return Ok(Vec::new());
    }
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(true)
        .build(&plan.pattern)
        .map_err(|e| e.to_string())?;
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .build();

    let files: Box<dyn Iterator<Item = Result<PathBuf, String>>> = match options.paths {
        Some(paths) => Box::new(paths.iter().map(|rel| Ok(options.cwd.join(rel)))),
        None => {
            let mut overrides = ignore::overrides::OverrideBuilder::new(options.cwd);
            overrides.add("!**/.git/**").map_err(|e| e.to_string())?;
            if let Some(glob) = &glob {
                overrides.add(glob).map_err(|e| e.to_string())?;
            }
            let overrides = overrides.build().map_err(|e| e.to_string())?;
            let walker = ignore::WalkBuilder::new(options.cwd)
                .hidden(false)
                .require_git(false)
                .add_custom_ignore_filename(".rgignore")
                .overrides(overrides)
                .sort_by_file_name(|a, b| a.cmp(b))
                .build();
            Box::new(walker.filter_map(|entry| match entry {
                Ok(entry) if entry.file_type().is_some_and(|t| t.is_file()) => {
                    Some(Ok(entry.into_path()))
                }
                Ok(_) => None,
                Err(e) => Some(Err(format!("rg: {e}"))),
            }))
        }
    };

    let mut hits = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for file in files {
        if aborted(&options.signal) {
            return Err("Operation aborted".into());
        }
        if hits.len() >= options.limit {
            // rg is killed at the cap, so errors past it are never reported.
            return Ok(hits);
        }
        let path = match file {
            Ok(path) => path,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let rel = to_rel(options.cwd, &path);
        let result = searcher.search_path(
            &matcher,
            &path,
            Lossy(|line_number, line| {
                let text = line.trim_end_matches(['\n', '\r']);
                hits.push(GrepLineHit {
                    rel: rel.clone(),
                    line: line_number as usize,
                    terms: Some(terms_on_line(&plan, text)),
                });
                Ok(hits.len() < options.limit)
            }),
        );
        if let Err(e) = result {
            errors.push(format!("rg: {}: {e}", path.display()));
        }
    }
    if aborted(&options.signal) {
        return Err("Operation aborted".into());
    }
    // rg exits 2 when any file or directory failed, which hoocode reports.
    if hits.len() < options.limit && !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(hits)
}
