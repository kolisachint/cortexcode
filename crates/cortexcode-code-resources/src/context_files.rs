//! `core/context-files.ts`: AGENTS.md / CLAUDE.md context files from the user
//! scopes and the cwd ancestor chain, with per-file and total size budgets, and
//! `resolvePromptInput`.

use crate::node_path;

/// `ContextFile.size`: set only past the soft limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextFileSize {
    Large,
    Truncated,
}

/// `ContextFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    pub path: String,
    pub content: String,
    /// Rough token estimate (bytes / 4).
    pub tokens: Option<u64>,
    pub size: Option<ContextFileSize>,
}

/// `resolvePromptInput`: an existing file's contents, else the input itself.
/// A read failure warns (the returned message) and falls back to the input.
pub fn resolve_prompt_input(
    input: Option<&str>,
    description: &str,
) -> (Option<String>, Option<String>) {
    let Some(input) = input.filter(|s| !s.is_empty()) else {
        return (None, None);
    };
    if std::path::Path::new(input).exists() {
        return match std::fs::read_to_string(input) {
            Ok(content) => (Some(content), None),
            Err(e) => (
                Some(input.to_string()),
                Some(format!(
                    "Warning: Could not read {description} file {input}: Error: {e}"
                )),
            ),
        };
    }
    (Some(input.to_string()), None)
}

const CONTEXT_FILE_WARN_BYTES: usize = 8 * 1024;
const CONTEXT_FILE_MAX_BYTES: usize = 40 * 1024;
const CONTEXT_TOTAL_WARN_BYTES: usize = 24 * 1024;
const CONTEXT_TOTAL_MAX_BYTES: usize = 64 * 1024;
const CONTEXT_TRIM_MIN_BYTES: usize = 512;

/// `str.slice(0, n)` in UTF-16 code units.
fn js_slice(s: &str, n: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&units)
}

/// `Math.round(bytes / 4)`.
fn tokens(bytes: usize) -> u64 {
    (bytes as f64 / 4.0).round() as u64
}

fn load_context_file_from_dir(dir: &str) -> (Option<ContextFile>, Vec<String>) {
    let mut warnings = Vec::new();
    for filename in ["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"] {
        let file_path = node_path::join(&[dir, filename]);
        if !std::path::Path::new(&file_path).exists() {
            continue;
        }
        match std::fs::read(&file_path) {
            Ok(bytes) => {
                let mut content = String::from_utf8_lossy(&bytes).into_owned();
                let len = content.len();
                let size = if len > CONTEXT_FILE_MAX_BYTES {
                    content = format!(
                        "{}\n\n[truncated: file exceeded {CONTEXT_FILE_MAX_BYTES} bytes (~10k tokens); keep context files brief — large specs belong in linked files, not in the system prompt]",
                        js_slice(&content, CONTEXT_FILE_MAX_BYTES)
                    );
                    Some(ContextFileSize::Truncated)
                } else if len > CONTEXT_FILE_WARN_BYTES {
                    Some(ContextFileSize::Large)
                } else {
                    None
                };
                return (
                    Some(ContextFile {
                        path: file_path,
                        content,
                        tokens: Some(tokens(len)),
                        size,
                    }),
                    warnings,
                );
            }
            Err(e) => warnings.push(format!("Could not read {file_path}: Error: {e}")),
        }
    }
    (None, warnings)
}

/// `LoadProjectContextFilesOptions`.
#[derive(Debug, Clone, Default)]
pub struct LoadProjectContextFilesOptions {
    pub cwd: String,
    /// The native home (`~/.cortexcode/agent`).
    pub agent_dir: String,
    /// The cross-vendor scope; defaults to `~/.agents`.
    pub user_agents_dir: Option<String>,
}

/// `loadProjectContextFiles`: `~/.agents`, the agent dir, then the cwd's
/// ancestors from the root down; each path once; then the total budget.
pub fn load_project_context_files(
    options: &LoadProjectContextFilesOptions,
) -> (Vec<ContextFile>, Vec<String>) {
    let user_agents_dir = options.user_agents_dir.clone().unwrap_or_else(|| {
        cortexcode_code_paths::user_agents_dir()
            .to_string_lossy()
            .into_owned()
    });
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for dir in [&user_agents_dir, &options.agent_dir] {
        let (file, w) = load_context_file_from_dir(dir);
        if let Some(file) = file.filter(|f| !seen.contains(&f.path)) {
            seen.push(file.path.clone());
            files.push(file);
        }
        warnings.extend(w);
    }

    let mut ancestors = Vec::new();
    let mut current = node_path::resolve("/", &options.cwd);
    loop {
        let (file, w) = load_context_file_from_dir(&current);
        if let Some(file) = file.filter(|f| !seen.contains(&f.path)) {
            seen.push(file.path.clone());
            ancestors.insert(0, file);
        }
        warnings.extend(w);
        if current == "/" {
            break;
        }
        let parent = node_path::resolve(&current, "..");
        if parent == current {
            break;
        }
        current = parent;
    }
    files.extend(ancestors);
    warnings.extend(enforce_total_budget(&mut files));
    (files, warnings)
}

/// JS number formatting of `Math.round(total / 4 / 100) / 10`.
fn thousands_of_tokens(total: usize) -> String {
    let tenths = (total as f64 / 4.0 / 100.0).round();
    let value = tenths / 10.0;
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// `enforceTotalBudget`: warn past the soft cap (for 2+ files); past the hard
/// cap trim from the least specific (front) file, nearest file keeping its budget.
fn enforce_total_budget(files: &mut [ContextFile]) -> Vec<String> {
    let mut warnings = Vec::new();
    let total: usize = files.iter().map(|f| f.content.len()).sum();
    if total <= CONTEXT_TOTAL_WARN_BYTES {
        return warnings;
    }
    if total <= CONTEXT_TOTAL_MAX_BYTES {
        if files.len() < 2 {
            return warnings;
        }
        warnings.push(format!(
            "Context files total ~{}k tokens across {} file(s), re-sent on every request. Keep rules to one line each, and move long or conditional guidance into a skill (loaded on demand) rather than a context file (loaded always).",
            thousands_of_tokens(total),
            files.len()
        ));
        return warnings;
    }
    let mut remaining = CONTEXT_TOTAL_MAX_BYTES;
    for file in files.iter_mut().rev() {
        let bytes = file.content.len();
        if bytes <= remaining {
            remaining -= bytes;
            continue;
        }
        let notice = format!(
            "\n\n[trimmed: context files exceeded {CONTEXT_TOTAL_MAX_BYTES} bytes (~16k tokens) in total; least-specific scopes are trimmed first — move long guidance into a skill]"
        );
        file.content = if remaining >= CONTEXT_TRIM_MIN_BYTES {
            format!("{}{notice}", js_slice(&file.content, remaining))
        } else {
            notice.trim_start().to_string()
        };
        file.size = Some(ContextFileSize::Truncated);
        file.tokens = Some(tokens(file.content.len()));
        remaining = 0;
        warnings.push(format!(
            "Trimmed {} — context files exceeded the total budget.",
            file.path
        ));
    }
    warnings
}
