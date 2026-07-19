//! Pure string/path helpers used by [`super::CombinedAutocompleteProvider`],
//! ported from the free functions at the top of `autocomplete.ts`.

use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

static PATH_DELIMITERS: Lazy<HashSet<char>> =
    Lazy::new(|| [' ', '\t', '"', '\'', '='].into_iter().collect());

pub fn to_display_path(value: &str) -> String {
    value.replace('\\', "/")
}

pub fn escape_regex(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if ".*+?^${}()|[]\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Build an fd `--regex`-style path pattern from a user query (segments
/// joined by a separator-class regex so `a/b` matches `a/b` or `a\b`).
pub fn build_fd_path_query(query: &str) -> String {
    let normalized = to_display_path(query);
    if !normalized.contains('/') {
        return normalized;
    }

    let has_trailing_separator = normalized.ends_with('/');
    let trimmed = normalized.trim_matches('/');
    if trimmed.is_empty() {
        return normalized;
    }

    const SEPARATOR_PATTERN: &str = "[\\\\/]";
    let segments: Vec<String> = trimmed
        .split('/')
        .filter(|s| !s.is_empty())
        .map(escape_regex)
        .collect();
    if segments.is_empty() {
        return normalized;
    }

    let mut pattern = segments.join(SEPARATOR_PATTERN);
    if has_trailing_separator {
        pattern.push_str(SEPARATOR_PATTERN);
    }
    pattern
}

pub fn find_last_delimiter(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len())
        .rev()
        .find(|&i| PATH_DELIMITERS.contains(&chars[i]))
}

/// Returns the char index where an unclosed `"` starts, if the text ends
/// mid-quote.
pub fn find_unclosed_quote_start(text: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut quote_start = None;
    for (i, c) in text.chars().enumerate() {
        if c == '"' {
            in_quotes = !in_quotes;
            if in_quotes {
                quote_start = Some(i);
            }
        }
    }
    if in_quotes {
        quote_start
    } else {
        None
    }
}

fn is_token_start(chars: &[char], index: usize) -> bool {
    index == 0 || PATH_DELIMITERS.contains(&chars[index - 1])
}

pub fn extract_quoted_prefix(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let quote_start = find_unclosed_quote_start(text)?;

    if quote_start > 0 && chars[quote_start - 1] == '@' {
        if !is_token_start(&chars, quote_start - 1) {
            return None;
        }
        return Some(chars[quote_start - 1..].iter().collect());
    }

    if !is_token_start(&chars, quote_start) {
        return None;
    }

    Some(chars[quote_start..].iter().collect())
}

pub struct ParsedPathPrefix {
    pub raw_prefix: String,
    pub is_at_prefix: bool,
    pub is_quoted_prefix: bool,
}

pub fn parse_path_prefix(prefix: &str) -> ParsedPathPrefix {
    if let Some(rest) = prefix.strip_prefix("@\"") {
        return ParsedPathPrefix {
            raw_prefix: rest.to_string(),
            is_at_prefix: true,
            is_quoted_prefix: true,
        };
    }
    if let Some(rest) = prefix.strip_prefix('"') {
        return ParsedPathPrefix {
            raw_prefix: rest.to_string(),
            is_at_prefix: false,
            is_quoted_prefix: true,
        };
    }
    if let Some(rest) = prefix.strip_prefix('@') {
        return ParsedPathPrefix {
            raw_prefix: rest.to_string(),
            is_at_prefix: true,
            is_quoted_prefix: false,
        };
    }
    ParsedPathPrefix {
        raw_prefix: prefix.to_string(),
        is_at_prefix: false,
        is_quoted_prefix: false,
    }
}

pub struct CompletionValueOptions {
    pub is_directory: bool,
    pub is_at_prefix: bool,
    pub is_quoted_prefix: bool,
}

pub fn build_completion_value(path: &str, options: &CompletionValueOptions) -> String {
    let _ = options.is_directory; // parity with the TS signature; unused like the original.
    let needs_quotes = options.is_quoted_prefix || path.contains(' ');
    let prefix = if options.is_at_prefix { "@" } else { "" };

    if !needs_quotes {
        return format!("{prefix}{path}");
    }
    format!("{prefix}\"{path}\"")
}

/// `path.basename` for POSIX-style (`/`-separated) paths.
pub fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    match trimmed.rfind('/') {
        Some(idx) => trimmed[idx + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

/// `path.dirname` for POSIX-style paths.
pub fn dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => trimmed[..idx].to_string(),
        None => ".".to_string(),
    }
}

/// `path.join` for POSIX-style paths: joins segments and normalizes `.`/`..`.
pub fn join_path(base: &str, rest: &str) -> String {
    let combined = if rest.is_empty() {
        base.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), rest)
    };
    normalize_path(&combined)
}

fn normalize_path(path: &str) -> String {
    let is_absolute = path.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if matches!(stack.last(), Some(s) if *s != "..") {
                    stack.pop();
                } else if !is_absolute {
                    stack.push("..");
                }
            }
            s => stack.push(s),
        }
    }
    let joined = stack.join("/");
    if is_absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn expand_home_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        let Some(home) = home_dir() else {
            return path.to_string();
        };
        let expanded = home.join(rest);
        let expanded_str = to_display_path(&expanded.to_string_lossy());
        if path.ends_with('/') && !expanded_str.ends_with('/') {
            format!("{expanded_str}/")
        } else {
            expanded_str
        }
    } else if path == "~" {
        home_dir()
            .map(|p| to_display_path(&p.to_string_lossy()))
            .unwrap_or_else(|| path.to_string())
    } else {
        path.to_string()
    }
}

pub fn is_dir(path: &Path) -> bool {
    path.metadata().map(|m| m.is_dir()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_fd_path_query_returns_bare_query_without_slash() {
        assert_eq!(build_fd_path_query("foo"), "foo");
    }

    #[test]
    fn build_fd_path_query_builds_separator_pattern() {
        assert_eq!(build_fd_path_query("src/main"), "src[\\\\/]main");
    }

    #[test]
    fn build_fd_path_query_keeps_trailing_separator() {
        assert_eq!(build_fd_path_query("src/"), "src[\\\\/]");
    }

    #[test]
    fn find_last_delimiter_finds_space() {
        assert_eq!(find_last_delimiter("foo bar"), Some(3));
        assert_eq!(find_last_delimiter("foobar"), None);
    }

    #[test]
    fn find_unclosed_quote_start_detects_open_quote() {
        assert_eq!(find_unclosed_quote_start("foo \"bar"), Some(4));
        assert_eq!(find_unclosed_quote_start("foo \"bar\""), None);
    }

    #[test]
    fn extract_quoted_prefix_handles_at_prefix() {
        assert_eq!(extract_quoted_prefix("foo @\"ba").as_deref(), Some("@\"ba"));
    }

    #[test]
    fn extract_quoted_prefix_none_when_not_token_start() {
        assert_eq!(extract_quoted_prefix("foo\"ba"), None);
    }

    #[test]
    fn parse_path_prefix_variants() {
        let p = parse_path_prefix("@\"src");
        assert_eq!(p.raw_prefix, "src");
        assert!(p.is_at_prefix);
        assert!(p.is_quoted_prefix);

        let p = parse_path_prefix("@src");
        assert_eq!(p.raw_prefix, "src");
        assert!(p.is_at_prefix);
        assert!(!p.is_quoted_prefix);

        let p = parse_path_prefix("src");
        assert_eq!(p.raw_prefix, "src");
        assert!(!p.is_at_prefix);
        assert!(!p.is_quoted_prefix);
    }

    #[test]
    fn build_completion_value_quotes_when_needed() {
        let opts = CompletionValueOptions {
            is_directory: false,
            is_at_prefix: true,
            is_quoted_prefix: false,
        };
        assert_eq!(
            build_completion_value("my file.txt", &opts),
            "@\"my file.txt\""
        );
        assert_eq!(build_completion_value("file.txt", &opts), "@file.txt");
    }

    #[test]
    fn basename_and_dirname_match_node_semantics() {
        assert_eq!(basename("a/b/c.txt"), "c.txt");
        assert_eq!(basename("c.txt"), "c.txt");
        assert_eq!(dirname("a/b/c.txt"), "a/b");
        assert_eq!(dirname("c.txt"), ".");
        assert_eq!(dirname("/c.txt"), "/");
    }

    #[test]
    fn join_path_normalizes() {
        assert_eq!(join_path("/base", "./a/../b"), "/base/b");
        assert_eq!(join_path("/base", ""), "/base");
    }
}
