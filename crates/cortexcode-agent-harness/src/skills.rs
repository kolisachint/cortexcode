//! Skills: hoocode `harness/skills.ts` (v0.5.89) without the
//! `ExecutionEnv`-backed loader (ledger 9.3b): invocation formatting, the
//! name/description rules, and the `/`-separated env path helpers.

use crate::types::Skill;

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
