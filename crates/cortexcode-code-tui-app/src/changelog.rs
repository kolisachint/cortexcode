//! The changelog: hoocode `utils/changelog.ts` (parsing), `getChangelogPath`
//! and `getChangelogForDisplay` (startup-checks.ts).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use cortexcode_code_settings::SettingsManager;
use regex::Regex;

/// `ChangelogEntry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogEntry {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub content: String,
}

impl ChangelogEntry {
    fn version(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }

    pub fn version_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

static VERSION_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"##\s+\[?(\d+)\.(\d+)\.(\d+)\]?").expect("static pattern"));

/// `getChangelogPath`: `CHANGELOG.md` in the package directory.
pub fn changelog_path() -> PathBuf {
    let path = cortexcode_code_paths::package_dir().join("CHANGELOG.md");
    std::path::absolute(&path).unwrap_or(path)
}

/// `parseChangelog`: one entry per `## [x.y.z]` section, newest first as
/// written; sections without a version are skipped.
pub fn parse_changelog(path: &Path) -> Vec<ChangelogEntry> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    let mut current_lines: Vec<&str> = Vec::new();
    let mut current_version: Option<(u64, u64, u64)> = None;
    let mut push = |version: Option<(u64, u64, u64)>, lines: &[&str]| {
        if let (Some((major, minor, patch)), false) = (version, lines.is_empty()) {
            entries.push(ChangelogEntry {
                major,
                minor,
                patch,
                content: lines.join("\n").trim().to_string(),
            });
        }
    };
    for line in content.split('\n') {
        if line.starts_with("## ") {
            push(current_version, &current_lines);
            match VERSION_HEADER.captures(line) {
                Some(caps) => {
                    let n = |i: usize| caps[i].parse::<u64>().unwrap_or(0);
                    current_version = Some((n(1), n(2), n(3)));
                    current_lines = vec![line];
                }
                None => {
                    current_version = None;
                    current_lines = Vec::new();
                }
            }
        } else if current_version.is_some() {
            current_lines.push(line);
        }
    }
    push(current_version, &current_lines);
    entries
}

/// `getNewEntries`: the entries newer than `last_version`.
pub fn get_new_entries(entries: &[ChangelogEntry], last_version: &str) -> Vec<ChangelogEntry> {
    let parts: Vec<u64> = last_version
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    let last = (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    );
    entries
        .iter()
        .filter(|e| e.version() > last)
        .cloned()
        .collect()
}

/// `getChangelogForDisplay`: the entries new since the last run, as one
/// markdown document, and the version recorded as seen. Nothing for a
/// resumed session, and nothing on a fresh install (which records the
/// latest entry, or `version` when there are none).
pub fn changelog_for_display(
    has_messages: bool,
    settings: &mut SettingsManager,
    version: &str,
) -> Option<String> {
    if has_messages {
        return None;
    }
    let entries = parse_changelog(&changelog_path());
    let Some(last_version) = settings.last_changelog_version() else {
        let seed = entries
            .first()
            .map_or_else(|| version.to_string(), ChangelogEntry::version_string);
        settings.set_last_changelog_version(&seed);
        return None;
    };
    let new_entries = get_new_entries(&entries, &last_version);
    let latest = new_entries.first()?;
    settings.set_last_changelog_version(&latest.version_string());
    Some(
        new_entries
            .iter()
            .map(|e| e.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CHANGELOG.md");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn parses_versioned_sections_and_skips_the_rest() {
        let (_dir, path) = write(
            "# Changelog\n\n## [Unreleased]\n\n- wip\n\n## [0.2.0] - 2026-01-02\n\n- two\n\n## 0.1.3\n\n- one\n",
        );
        let entries = parse_changelog(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].version_string(), "0.2.0");
        assert_eq!(entries[0].content, "## [0.2.0] - 2026-01-02\n\n- two");
        assert_eq!(entries[1].version_string(), "0.1.3");
        assert_eq!(entries[1].content, "## 0.1.3\n\n- one");
    }

    #[test]
    fn a_missing_file_has_no_entries() {
        assert!(parse_changelog(Path::new("/nonexistent/CHANGELOG.md")).is_empty());
    }

    #[test]
    fn new_entries_are_the_ones_after_the_last_version() {
        let (_dir, path) = write("## [1.2.0]\na\n## [1.1.10]\nb\n## [1.1.9]\nc\n");
        let entries = parse_changelog(&path);
        let new: Vec<String> = get_new_entries(&entries, "1.1.9")
            .iter()
            .map(ChangelogEntry::version_string)
            .collect();
        assert_eq!(new, vec!["1.2.0", "1.1.10"]);
        assert!(get_new_entries(&entries, "2").is_empty());
    }
}
