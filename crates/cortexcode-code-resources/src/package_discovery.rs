//! `core/package-resource-discovery.ts`: resource file discovery (ignore files,
//! skill/prompt/theme/extension layouts) and the include/exclude pattern engine
//! (plain globs plus `!` / `+` / `-` overrides).

use crate::node_path;
use serde_json::Value;
use std::collections::HashSet;

/// `ResourceType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    Extensions,
    Skills,
    Prompts,
    Themes,
    Agents,
}

impl ResourceType {
    /// The settings / manifest key.
    pub fn key(self) -> &'static str {
        match self {
            Self::Extensions => "extensions",
            Self::Skills => "skills",
            Self::Prompts => "prompts",
            Self::Themes => "themes",
            Self::Agents => "agents",
        }
    }

    fn matches_file(self, name: &str) -> bool {
        match self {
            Self::Extensions => name.ends_with(".ts") || name.ends_with(".js"),
            Self::Skills | Self::Prompts | Self::Agents => name.ends_with(".md"),
            Self::Themes => name.ends_with(".json"),
        }
    }
}

/// `SETTINGS_RESOURCE_TYPES`.
pub const SETTINGS_RESOURCE_TYPES: [ResourceType; 4] = [
    ResourceType::Extensions,
    ResourceType::Skills,
    ResourceType::Prompts,
    ResourceType::Themes,
];

const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

/// The `ignore` package matcher, rooted at the scan root.
#[derive(Clone)]
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

fn add_ignore_rules(matcher: &mut IgnoreMatcher, dir: &str, root: &str) {
    let relative = node_path::relative(root, dir);
    let prefix = if relative.is_empty() {
        String::new()
    } else {
        format!("{relative}/")
    };
    for name in IGNORE_FILE_NAMES {
        let Ok(content) = std::fs::read_to_string(node_path::join(&[dir, name])) else {
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

/// A directory listing in `readdirSync` order (libuv sorts by name), with each
/// entry's file / directory kind after following symlinks; broken links dropped.
fn list_dir(dir: &str) -> Option<Vec<(String, bool, bool)>> {
    let read_dir = std::fs::read_dir(dir).ok()?;
    let mut entries: Vec<(String, bool, bool)> = read_dir
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let file_type = entry.file_type().ok()?;
            if file_type.is_symlink() {
                let meta = std::fs::metadata(node_path::join(&[dir, &name])).ok()?;
                return Some((name, meta.is_file(), meta.is_dir()));
            }
            Some((name, file_type.is_file(), file_type.is_dir()))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Some(entries)
}

fn exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

/// `isOverridePattern`.
pub fn is_override_pattern(s: &str) -> bool {
    s.starts_with('!') || s.starts_with('+') || s.starts_with('-')
}

/// `hasGlobPattern`.
pub fn has_glob_pattern(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

/// `splitPatterns`: plain entries vs patterns (overrides and globs).
pub fn split_patterns(entries: &[String]) -> (Vec<String>, Vec<String>) {
    entries
        .iter()
        .cloned()
        .partition(|e| !(is_override_pattern(e) || has_glob_pattern(e)))
}

/// `collectFiles`: recursive, ignore files honored, dot entries and
/// `node_modules` skipped.
fn collect_files(
    dir: &str,
    resource_type: ResourceType,
    matcher: &mut IgnoreMatcher,
    root: &str,
) -> Vec<String> {
    let mut files = Vec::new();
    if !exists(dir) {
        return files;
    }
    add_ignore_rules(matcher, dir, root);
    let Some(entries) = list_dir(dir) else {
        return files;
    };
    for (name, is_file, is_dir) in entries {
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let full = node_path::join(&[dir, &name]);
        let rel = node_path::relative(root, &full);
        let ignore_path = if is_dir { format!("{rel}/") } else { rel };
        if matcher.ignores(&ignore_path) {
            continue;
        }
        if is_dir {
            files.extend(collect_files(&full, resource_type, matcher, root));
        } else if is_file && resource_type.matches_file(&name) {
            files.push(full);
        }
    }
    files
}

/// `SkillDiscoveryMode`: `Hoocode` also takes root-level `*.md` files as skills;
/// `Agents` (the `~/.agents` layout) only `<name>/SKILL.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillDiscoveryMode {
    Hoocode,
    Agents,
}

fn collect_skill_entries(
    dir: &str,
    mode: SkillDiscoveryMode,
    matcher: &mut IgnoreMatcher,
    root: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    if !exists(dir) {
        return out;
    }
    add_ignore_rules(matcher, dir, root);
    let Some(entries) = list_dir(dir) else {
        return out;
    };
    for (name, is_file, _) in &entries {
        if name != "SKILL.md" {
            continue;
        }
        let full = node_path::join(&[dir, name]);
        if *is_file && !matcher.ignores(&node_path::relative(root, &full)) {
            out.push(full);
            return out;
        }
    }
    for (name, is_file, is_dir) in entries {
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let full = node_path::join(&[dir, &name]);
        let rel = node_path::relative(root, &full);
        if mode == SkillDiscoveryMode::Hoocode
            && dir == root
            && is_file
            && name.ends_with(".md")
            && !matcher.ignores(&rel)
        {
            out.push(full);
            continue;
        }
        if !is_dir || matcher.ignores(&format!("{rel}/")) {
            continue;
        }
        out.extend(collect_skill_entries(&full, mode, matcher, root));
    }
    out
}

/// `collectAutoSkillEntries`.
pub fn collect_auto_skill_entries(dir: &str, mode: SkillDiscoveryMode) -> Vec<String> {
    collect_skill_entries(dir, mode, &mut IgnoreMatcher::new(), dir)
}

fn find_git_repo_root(start: &str) -> Option<String> {
    let mut dir = node_path::resolve("/", start);
    loop {
        if exists(&node_path::join(&[&dir, ".git"])) {
            return Some(dir);
        }
        let parent = node_path::dirname(&dir);
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}

/// `collectAncestorAgentsSkillDirs`: `.agents/skills` from `start` up to the
/// git root (or the filesystem root), cwd first.
pub fn collect_ancestor_agents_skill_dirs(start: &str) -> Vec<String> {
    let start = node_path::resolve("/", start);
    let git_root = find_git_repo_root(&start);
    let mut dirs = Vec::new();
    let mut dir = start;
    loop {
        dirs.push(node_path::join(&[&dir, ".agents", "skills"]));
        if git_root.as_deref() == Some(dir.as_str()) {
            break;
        }
        let parent = node_path::dirname(&dir);
        if parent == dir {
            break;
        }
        dir = parent;
    }
    dirs
}

/// Flat files of one kind directly in `dir` (prompts, themes).
fn collect_flat(dir: &str, extension: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !exists(dir) {
        return out;
    }
    let mut matcher = IgnoreMatcher::new();
    add_ignore_rules(&mut matcher, dir, dir);
    let Some(entries) = list_dir(dir) else {
        return out;
    };
    for (name, is_file, _) in entries {
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let full = node_path::join(&[dir, &name]);
        if matcher.ignores(&node_path::relative(dir, &full)) {
            continue;
        }
        if is_file && name.ends_with(extension) {
            out.push(full);
        }
    }
    out
}

/// `collectAutoPromptEntries`.
pub fn collect_auto_prompt_entries(dir: &str) -> Vec<String> {
    collect_flat(dir, ".md")
}

/// `collectAutoThemeEntries`.
pub fn collect_auto_theme_entries(dir: &str) -> Vec<String> {
    collect_flat(dir, ".json")
}

/// `readHooCodeManifestFile`: the `hoocode` (or legacy `pi`) key of a package.json.
/// cortex also accepts `cortexcode`.
pub fn read_package_manifest(package_json_path: &str) -> Option<serde_json::Map<String, Value>> {
    let text = std::fs::read_to_string(package_json_path).ok()?;
    let pkg: Value = serde_json::from_str(&text).ok()?;
    ["cortexcode", "hoocode", "pi"]
        .iter()
        .find_map(|key| pkg.get(*key).and_then(Value::as_object).cloned())
}

/// A manifest's string list for `key`.
pub fn manifest_list(manifest: &serde_json::Map<String, Value>, key: &str) -> Vec<String> {
    manifest
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `resolveExtensionEntries`: a directory's manifest extensions, else its
/// `index.ts` / `index.js`.
fn resolve_extension_entries(dir: &str) -> Option<Vec<String>> {
    let package_json = node_path::join(&[dir, "package.json"]);
    if exists(&package_json) {
        if let Some(manifest) = read_package_manifest(&package_json) {
            let entries: Vec<String> = manifest_list(&manifest, "extensions")
                .iter()
                .map(|p| node_path::resolve(dir, p))
                .filter(|p| exists(p))
                .collect();
            if !entries.is_empty() {
                return Some(entries);
            }
        }
    }
    ["index.ts", "index.js"]
        .iter()
        .map(|f| node_path::join(&[dir, f]))
        .find(|p| exists(p))
        .map(|p| vec![p])
}

/// `collectAutoExtensionEntries`: the directory's own entry points, else its
/// `*.ts` / `*.js` files and subdirectories with entry points.
pub fn collect_auto_extension_entries(dir: &str) -> Vec<String> {
    if !exists(dir) {
        return Vec::new();
    }
    if let Some(entries) = resolve_extension_entries(dir) {
        return entries;
    }
    let mut out = Vec::new();
    let mut matcher = IgnoreMatcher::new();
    add_ignore_rules(&mut matcher, dir, dir);
    let Some(entries) = list_dir(dir) else {
        return out;
    };
    for (name, is_file, is_dir) in entries {
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let full = node_path::join(&[dir, &name]);
        let rel = node_path::relative(dir, &full);
        let ignore_path = if is_dir { format!("{rel}/") } else { rel };
        if matcher.ignores(&ignore_path) {
            continue;
        }
        if is_file && (name.ends_with(".ts") || name.ends_with(".js")) {
            out.push(full);
        } else if is_dir {
            out.extend(resolve_extension_entries(&full).unwrap_or_default());
        }
    }
    out
}

/// `collectResourceFiles`.
pub fn collect_resource_files(dir: &str, resource_type: ResourceType) -> Vec<String> {
    match resource_type {
        ResourceType::Skills => collect_auto_skill_entries(dir, SkillDiscoveryMode::Hoocode),
        ResourceType::Extensions => collect_auto_extension_entries(dir),
        other => collect_files(dir, other, &mut IgnoreMatcher::new(), dir),
    }
}

/// `minimatch(path, pattern)` with default options.
fn minimatch(path: &str, pattern: &str) -> bool {
    glob::Pattern::new(pattern).is_ok_and(|p| {
        p.matches_with(
            path,
            glob::MatchOptions {
                case_sensitive: true,
                require_literal_separator: true,
                require_literal_leading_dot: true,
            },
        )
    })
}

/// The forms a file is matched by: relative to the base dir, its name, and its
/// full path; a `SKILL.md` also by its directory in the same three forms.
struct MatchForms {
    rel: String,
    name: String,
    full: String,
    skill_dir: Option<(String, String, String)>,
}

fn match_forms(file_path: &str, base_dir: &str) -> MatchForms {
    let name = node_path::basename(file_path);
    let skill_dir = (name == "SKILL.md").then(|| {
        let parent = node_path::dirname(file_path);
        (
            node_path::relative(base_dir, &parent),
            node_path::basename(&parent),
            parent,
        )
    });
    MatchForms {
        rel: node_path::relative(base_dir, file_path),
        name,
        full: file_path.to_string(),
        skill_dir,
    }
}

fn matches_any_pattern(file_path: &str, patterns: &[String], base_dir: &str) -> bool {
    let f = match_forms(file_path, base_dir);
    patterns.iter().any(|p| {
        if minimatch(&f.rel, p) || minimatch(&f.name, p) || minimatch(&f.full, p) {
            return true;
        }
        f.skill_dir.as_ref().is_some_and(|(rel, name, full)| {
            minimatch(rel, p) || minimatch(name, p) || minimatch(full, p)
        })
    })
}

fn matches_any_exact_pattern(file_path: &str, patterns: &[String], base_dir: &str) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let f = match_forms(file_path, base_dir);
    patterns.iter().any(|p| {
        let normalized = p.strip_prefix("./").unwrap_or(p);
        if normalized == f.rel || normalized == f.full {
            return true;
        }
        f.skill_dir
            .as_ref()
            .is_some_and(|(rel, _, full)| normalized == rel || normalized == full)
    })
}

fn strip_each(patterns: &[String], prefix: char) -> Vec<String> {
    patterns
        .iter()
        .filter_map(|p| p.strip_prefix(prefix).map(str::to_string))
        .collect()
}

/// `isEnabledByOverrides`: `!` excludes (glob), `+` force-includes and `-`
/// force-excludes (exact paths), in that order.
pub fn is_enabled_by_overrides(file_path: &str, patterns: &[String], base_dir: &str) -> bool {
    let excludes = strip_each(patterns, '!');
    let force_includes = strip_each(patterns, '+');
    let force_excludes = strip_each(patterns, '-');
    let mut enabled = true;
    if !excludes.is_empty() && matches_any_pattern(file_path, &excludes, base_dir) {
        enabled = false;
    }
    if matches_any_exact_pattern(file_path, &force_includes, base_dir) {
        enabled = true;
    }
    if matches_any_exact_pattern(file_path, &force_excludes, base_dir) {
        enabled = false;
    }
    enabled
}

/// `applyPatterns`: plain globs include (all when none), `!` excludes, `+`
/// force-includes, `-` force-excludes. Returns the enabled paths.
pub fn apply_patterns(
    all_paths: &[String],
    patterns: &[String],
    base_dir: &str,
) -> HashSet<String> {
    let (mut includes, mut excludes, mut force_includes, mut force_excludes) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for p in patterns {
        if let Some(rest) = p.strip_prefix('+') {
            force_includes.push(rest.to_string());
        } else if let Some(rest) = p.strip_prefix('-') {
            force_excludes.push(rest.to_string());
        } else if let Some(rest) = p.strip_prefix('!') {
            excludes.push(rest.to_string());
        } else {
            includes.push(p.clone());
        }
    }
    let mut result: Vec<String> = if includes.is_empty() {
        all_paths.to_vec()
    } else {
        all_paths
            .iter()
            .filter(|p| matches_any_pattern(p, &includes, base_dir))
            .cloned()
            .collect()
    };
    if !excludes.is_empty() {
        result.retain(|p| !matches_any_pattern(p, &excludes, base_dir));
    }
    for p in all_paths {
        if !result.contains(p) && matches_any_exact_pattern(p, &force_includes, base_dir) {
            result.push(p.clone());
        }
    }
    if !force_excludes.is_empty() {
        result.retain(|p| !matches_any_exact_pattern(p, &force_excludes, base_dir));
    }
    result.into_iter().collect()
}
