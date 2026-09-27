//! Node's `path` functions over string paths (POSIX): `join`, `resolve`,
//! `dirname`, `basename`, `relative`, plus hoocode's `~` expansion.

use std::path::PathBuf;

/// `path.normalize` (lexical: `.`, `..` and repeated separators).
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    let trailing = path.ends_with('/') && !joined.is_empty();
    let mut out = match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    };
    if trailing {
        out.push('/');
    }
    out
}

/// `path.join`.
pub fn join(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    normalize(&joined)
}

/// `path.resolve(base, p)`.
pub fn resolve(base: &str, path: &str) -> String {
    let combined = if path.starts_with('/') {
        path.to_string()
    } else {
        let base = if base.starts_with('/') {
            base.to_string()
        } else {
            let cwd = std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "/".into());
            format!("{cwd}/{base}")
        };
        format!("{base}/{path}")
    };
    let normalized = normalize(&combined);
    if normalized.len() > 1 {
        normalized.trim_end_matches('/').to_string()
    } else {
        normalized
    }
}

/// `path.dirname`.
pub fn dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => trimmed[..i].to_string(),
        None if path.starts_with('/') => "/".into(),
        None => ".".into(),
    }
}

/// `path.basename`.
pub fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or("").to_string()
}

/// `path.relative(from, to)` for absolute paths.
pub fn relative(from: &str, to: &str) -> String {
    let from = resolve("/", from);
    let to = resolve("/", to);
    let from_parts: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
    let to_parts: Vec<&str> = to.split('/').filter(|p| !p.is_empty()).collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out: Vec<&str> = vec![".."; from_parts.len() - common];
    out.extend(&to_parts[common..]);
    out.join("/")
}

/// hoocode's `normalizePath` for configured paths: `~`, `~/x` and `~x` expand
/// against the home directory.
pub fn expand_home(input: &str) -> String {
    let trimmed = input.trim();
    let home = || {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/"))
            .to_string_lossy()
            .into_owned()
    };
    if trimmed == "~" {
        return home();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return join(&[&home(), rest]);
    }
    if let Some(rest) = trimmed.strip_prefix('~') {
        return join(&[&home(), rest]);
    }
    trimmed.to_string()
}

/// `isAbsolute(p) ? p : resolve(cwd, p)` after `~` expansion.
pub fn resolve_config_path(path: &str, cwd: &str) -> String {
    let normalized = expand_home(path);
    if normalized.starts_with('/') {
        normalized
    } else {
        resolve(cwd, &normalized)
    }
}

/// The `isUnderPath` helper of skills.ts / prompt-templates.ts.
pub fn is_under_path(target: &str, root: &str) -> bool {
    let root = resolve("/", root);
    if target == root {
        return true;
    }
    let prefix = if root.ends_with('/') {
        root
    } else {
        format!("{root}/")
    };
    target.starts_with(&prefix)
}

/// `canonicalizePath`: `realpath`, else the path unchanged.
pub fn canonicalize(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_path_semantics() {
        assert_eq!(join(&["/a/b", "../c", "d.md"]), "/a/c/d.md");
        assert_eq!(join(&["/a//b/", "./c"]), "/a/b/c");
        assert_eq!(resolve("/w", ".hoocode/skills"), "/w/.hoocode/skills");
        assert_eq!(resolve("/w", "/abs/x/"), "/abs/x");
        assert_eq!(dirname("/a/b/SKILL.md"), "/a/b");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(basename("/a/b/"), "b");
        assert_eq!(relative("/a/b", "/a/b/c/d"), "c/d");
        assert_eq!(relative("/a/b", "/a/b"), "");
        assert!(is_under_path("/a/b/c", "/a/b"));
        assert!(!is_under_path("/a/bc", "/a/b"));
    }
}
