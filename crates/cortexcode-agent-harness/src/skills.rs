//! Skills: hoocode `harness/skills.ts` (v0.5.89): loading `SKILL.md` files
//! through an [`ExecutionEnv`], invocation formatting, the name and
//! description rules, and the `/`-separated env path helpers.

use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::env::{ExecutionEnv, FileInfo, FileKind};
use crate::frontmatter::{locale_compare, parse_frontmatter};
use crate::types::Skill;

const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

/// `SkillDiagnostic` (always a warning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDiagnostic {
    pub message: String,
    pub path: String,
}

/// `loadSkills`' result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadedSkills {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// A value tagged with the source it was loaded from.
#[derive(Debug, Clone, PartialEq)]
pub struct Sourced<T, S> {
    pub item: T,
    pub source: S,
}

/// `loadSourcedSkills`' result.
#[derive(Debug, Clone, PartialEq)]
pub struct SourcedSkills<S> {
    pub skills: Vec<Sourced<Skill, S>>,
    pub diagnostics: Vec<Sourced<SkillDiagnostic, S>>,
}

/// The ignore rules gathered from `.gitignore` / `.ignore` / `.fdignore`
/// files while walking (the `ignore` package, rules relative to the root).
struct IgnoreMatcher {
    lines: Vec<String>,
    matcher: ignore::gitignore::Gitignore,
}

impl IgnoreMatcher {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            matcher: ignore::gitignore::Gitignore::empty(),
        }
    }

    fn add(&mut self, patterns: Vec<String>) {
        self.lines.extend(patterns);
        let mut builder = ignore::gitignore::GitignoreBuilder::new("/");
        for line in &self.lines {
            let _ = builder.add_line(None, line);
        }
        if let Ok(matcher) = builder.build() {
            self.matcher = matcher;
        }
    }

    /// `ignores(relPath)`; a trailing `/` marks a directory.
    fn ignores(&self, rel_path: &str) -> bool {
        if self.lines.is_empty() || rel_path.is_empty() {
            return false;
        }
        let is_dir = rel_path.ends_with('/');
        let path = format!("/{}", rel_path.trim_end_matches('/'));
        self.matcher
            .matched_path_or_any_parents(&path, is_dir)
            .is_ignore()
    }
}

async fn safe_file_info(env: &dyn ExecutionEnv, path: &str) -> Option<FileInfo> {
    env.file_info(path).await.ok()
}

/// `resolveKind`: a symlink resolves to its target's kind.
pub(crate) async fn resolve_kind(env: &dyn ExecutionEnv, info: &FileInfo) -> Option<FileKind> {
    match info.kind {
        FileKind::File | FileKind::Directory => Some(info.kind),
        FileKind::Symlink => {
            let real = env.real_path(&info.path).await.ok()?;
            match env.file_info(&real).await.ok()?.kind {
                kind @ (FileKind::File | FileKind::Directory) => Some(kind),
                FileKind::Symlink => None,
            }
        }
    }
}

/// `prefixIgnorePattern`: an ignore-file line rebased onto the root.
fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || (trimmed.starts_with('#') && !trimmed.starts_with("\\#")) {
        return None;
    }
    let mut pattern = line;
    let mut negated = false;
    if let Some(rest) = pattern.strip_prefix('!') {
        negated = true;
        pattern = rest;
    } else if pattern.starts_with("\\!") {
        pattern = &pattern[1..];
    }
    let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
    let prefixed = format!("{prefix}{pattern}");
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

async fn add_ignore_rules(
    env: &dyn ExecutionEnv,
    matcher: &mut IgnoreMatcher,
    dir: &str,
    root: &str,
) {
    let relative = relative_env_path(root, dir);
    let prefix = if relative.is_empty() {
        String::new()
    } else {
        format!("{relative}/")
    };
    for name in IGNORE_FILE_NAMES {
        let path = join_env_path(dir, name);
        if safe_file_info(env, &path).await.map(|i| i.kind) != Some(FileKind::File) {
            continue;
        }
        let Ok(content) = env.read_text_file(&path).await else {
            continue;
        };
        let patterns: Vec<String> = content
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter_map(|line| prefix_ignore_pattern(line, &prefix))
            .collect();
        if !patterns.is_empty() {
            matcher.add(patterns);
        }
    }
}

/// A non-empty string field of the frontmatter.
fn string_field<'a>(frontmatter: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    frontmatter.get(key).and_then(Value::as_str)
}

/// `loadSkillFromFile`: `None` (with diagnostics) when the file is
/// unreadable, malformed or has no description.
async fn load_skill_from_file(
    env: &dyn ExecutionEnv,
    file_path: &str,
) -> (Option<Skill>, Vec<SkillDiagnostic>) {
    let mut diagnostics = Vec::new();
    let warn = |message: String| SkillDiagnostic {
        message,
        path: file_path.to_string(),
    };
    let raw = match env.read_text_file(file_path).await {
        Ok(raw) => raw,
        Err(e) => return (None, vec![warn(e.message)]),
    };
    let (frontmatter, body) = match parse_frontmatter(&raw) {
        Ok(parsed) => parsed,
        Err(message) => return (None, vec![warn(message)]),
    };
    let skill_dir = dirname_env_path(file_path);
    let parent_dir_name = basename_env_path(&skill_dir);
    let description = string_field(&frontmatter, "description");
    diagnostics.extend(validate_description(description).into_iter().map(warn));
    let name = string_field(&frontmatter, "name")
        .filter(|n| !n.is_empty())
        .unwrap_or(&parent_dir_name)
        .to_string();
    diagnostics.extend(validate_name(&name, &parent_dir_name).into_iter().map(warn));
    let Some(description) = description.filter(|d| !d.trim().is_empty()) else {
        return (None, diagnostics);
    };
    let skill = Skill {
        name,
        description: description.to_string(),
        content: body,
        file_path: file_path.to_string(),
        disable_model_invocation: Some(
            frontmatter.get("disable-model-invocation") == Some(&Value::Bool(true)),
        ),
    };
    (Some(skill), diagnostics)
}

/// `loadSkillsFromDirInternal`: a `SKILL.md` makes the directory one skill;
/// otherwise subdirectories are walked (and, at the root, direct `.md` files
/// are skills), honoring ignore files and skipping dot entries and
/// `node_modules`.
fn load_skills_from_dir<'a>(
    env: &'a dyn ExecutionEnv,
    dir: String,
    include_root_files: bool,
    matcher: &'a mut IgnoreMatcher,
    root: &'a str,
) -> BoxFuture<'a, LoadedSkills> {
    Box::pin(async move {
        let mut result = LoadedSkills::default();
        if !env.exists(&dir).await.unwrap_or(false) {
            return result;
        }
        let Some(info) = safe_file_info(env, &dir).await else {
            return result;
        };
        if resolve_kind(env, &info).await != Some(FileKind::Directory) {
            return result;
        }
        add_ignore_rules(env, matcher, &dir, root).await;
        let Ok(mut entries) = env.list_dir(&dir).await else {
            return result;
        };

        for entry in &entries {
            if entry.name != "SKILL.md" || resolve_kind(env, entry).await != Some(FileKind::File) {
                continue;
            }
            if matcher.ignores(&relative_env_path(root, &entry.path)) {
                continue;
            }
            let (skill, diagnostics) = load_skill_from_file(env, &entry.path).await;
            result.skills.extend(skill);
            result.diagnostics.extend(diagnostics);
            return result;
        }

        entries.sort_by(|a, b| locale_compare(&a.name, &b.name));
        for entry in entries {
            if entry.name.starts_with('.') || entry.name == "node_modules" {
                continue;
            }
            let Some(kind) = resolve_kind(env, &entry).await else {
                continue;
            };
            let rel = relative_env_path(root, &entry.path);
            let ignore_path = if kind == FileKind::Directory {
                format!("{rel}/")
            } else {
                rel
            };
            if matcher.ignores(&ignore_path) {
                continue;
            }
            if kind == FileKind::Directory {
                let nested =
                    load_skills_from_dir(env, entry.path.clone(), false, matcher, root).await;
                result.skills.extend(nested.skills);
                result.diagnostics.extend(nested.diagnostics);
                continue;
            }
            if kind != FileKind::File || !include_root_files || !entry.name.ends_with(".md") {
                continue;
            }
            let (skill, diagnostics) = load_skill_from_file(env, &entry.path).await;
            result.skills.extend(skill);
            result.diagnostics.extend(diagnostics);
        }
        result
    })
}

/// `loadSkills`: skills under each directory (missing ones are skipped).
pub async fn load_skills(env: &dyn ExecutionEnv, dirs: &[&str]) -> LoadedSkills {
    let mut result = LoadedSkills::default();
    for dir in dirs {
        let Some(root) = safe_file_info(env, dir).await else {
            continue;
        };
        if resolve_kind(env, &root).await != Some(FileKind::Directory) {
            continue;
        }
        let mut matcher = IgnoreMatcher::new();
        let loaded =
            load_skills_from_dir(env, root.path.clone(), true, &mut matcher, &root.path).await;
        result.skills.extend(loaded.skills);
        result.diagnostics.extend(loaded.diagnostics);
    }
    result
}

/// `loadSourcedSkills`: [`load_skills`] per input, every skill and
/// diagnostic tagged with its input's source. (TS's `mapSkill` is a map
/// over the result here.)
pub async fn load_sourced_skills<S: Clone>(
    env: &dyn ExecutionEnv,
    inputs: &[(String, S)],
) -> SourcedSkills<S> {
    let mut result = SourcedSkills {
        skills: Vec::new(),
        diagnostics: Vec::new(),
    };
    for (path, source) in inputs {
        let loaded = load_skills(env, &[path.as_str()]).await;
        result
            .skills
            .extend(loaded.skills.into_iter().map(|item| Sourced {
                item,
                source: source.clone(),
            }));
        result
            .diagnostics
            .extend(loaded.diagnostics.into_iter().map(|item| Sourced {
                item,
                source: source.clone(),
            }));
    }
    result
}

pub const MAX_NAME_LENGTH: usize = 64;
pub const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// `formatSkillInvocation`: the `<skill>` block, then any extra
/// instructions.
pub fn format_skill_invocation(skill: &Skill, additional_instructions: Option<&str>) -> String {
    let block = format!(
        "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
        skill.name,
        skill.file_path,
        dirname_env_path(&skill.file_path),
        skill.content
    );
    match additional_instructions.filter(|s| !s.is_empty()) {
        Some(extra) => format!("{block}\n\n{extra}"),
        None => block,
    }
}

/// `str.length` in UTF-16 code units.
fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `validateName`: the name must match its directory and be lowercase
/// kebab-case, at most 64 characters.
pub fn validate_name(name: &str, parent_dir_name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if name != parent_dir_name {
        errors.push(format!(
            "name \"{name}\" does not match parent directory \"{parent_dir_name}\""
        ));
    }
    if js_len(name) > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({})",
            js_len(name)
        ));
    }
    let valid_chars = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !valid_chars {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
                .to_string(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }
    errors
}

/// `validateDescription`: required, at most 1024 characters.
pub fn validate_description(description: Option<&str>) -> Vec<String> {
    match description {
        Some(d) if !d.trim().is_empty() => {
            if js_len(d) > MAX_DESCRIPTION_LENGTH {
                vec![format!(
                    "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({})",
                    js_len(d)
                )]
            } else {
                Vec::new()
            }
        }
        _ => vec!["description is required".to_string()],
    }
}

/// `joinEnvPath`.
pub fn join_env_path(base: &str, child: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        child.trim_start_matches('/')
    )
}

/// `dirnameEnvPath`: `/` for a top-level or relative single segment.
pub fn dirname_env_path(path: &str) -> String {
    let normalized = path.trim_end_matches('/');
    match normalized.rfind('/') {
        Some(index) if index > 0 => normalized[..index].to_string(),
        _ => "/".to_string(),
    }
}

/// `basenameEnvPath`.
pub fn basename_env_path(path: &str) -> String {
    let normalized = path.trim_end_matches('/');
    match normalized.rfind('/') {
        Some(index) => normalized[index + 1..].to_string(),
        None => normalized.to_string(),
    }
}

/// `relativeEnvPath`: `path` below `root`, else `path` without leading `/`.
pub fn relative_env_path(root: &str, path: &str) -> String {
    let root = root.trim_end_matches('/');
    let path = path.trim_end_matches('/');
    if path == root {
        return String::new();
    }
    match path
        .strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        Some(rest) => rest.to_string(),
        None => path.trim_start_matches('/').to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_must_match_the_directory_and_be_kebab_case() {
        assert!(validate_name("my-skill", "my-skill").is_empty());
        assert_eq!(
            validate_name("Bad--", "dir"),
            [
                "name \"Bad--\" does not match parent directory \"dir\"",
                "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)",
                "name must not start or end with a hyphen",
                "name must not contain consecutive hyphens",
            ]
        );
        let long = "a".repeat(65);
        assert_eq!(
            validate_name(&long, &long),
            ["name exceeds 64 characters (65)"]
        );
    }

    #[test]
    fn descriptions_are_required_and_capped() {
        assert_eq!(validate_description(None), ["description is required"]);
        assert_eq!(
            validate_description(Some("  ")),
            ["description is required"]
        );
        assert!(validate_description(Some("ok")).is_empty());
        assert_eq!(
            validate_description(Some(&"d".repeat(1025))),
            ["description exceeds 1024 characters (1025)"]
        );
    }

    #[test]
    fn env_paths() {
        assert_eq!(join_env_path("/a/", "/b"), "/a/b");
        assert_eq!(dirname_env_path("/a/b/SKILL.md"), "/a/b");
        assert_eq!(dirname_env_path("/SKILL.md"), "/");
        assert_eq!(dirname_env_path("SKILL.md"), "/");
        assert_eq!(basename_env_path("/a/b/"), "b");
        assert_eq!(relative_env_path("/r/", "/r/x/y"), "x/y");
        assert_eq!(relative_env_path("/r", "/r"), "");
        assert_eq!(relative_env_path("/r", "/other/z"), "other/z");
    }
}
