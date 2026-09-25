//! Path resolution for tool arguments (`core/tools/path-utils.ts`).

use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

const NARROW_NO_BREAK_SPACE: char = '\u{202F}';

/// `/[  -   　]/`
fn is_unicode_space(c: char) -> bool {
    matches!(
        c,
        '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// `normalizeFileUrl`: a `file:///` URL becomes a path (`fileURLToPath`); when
/// that fails the `file://` prefix is stripped by hand.
fn normalize_file_url(file_path: &str) -> String {
    if !file_path.starts_with("file:///") {
        return file_path.to_string();
    }
    let stripped = &file_path["file://".len()..];
    // fileURLToPath refuses an encoded "/" in a path segment.
    let lower = file_path.to_ascii_lowercase();
    if lower.contains("%2f") {
        return stripped.to_string();
    }
    url::Url::parse(file_path)
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| stripped.to_string())
}

/// The user's home directory as Node's `os.homedir()` reports it on POSIX.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `expandPath`: file URL → path, strip a leading `@`, normalize unicode
/// spaces, expand `~` against `home`.
pub fn expand_path_with_home(file_path: &str, home: &Path) -> String {
    let url_normalized = normalize_file_url(file_path);
    let without_at = url_normalized.strip_prefix('@').unwrap_or(&url_normalized);
    let normalized: String = without_at
        .chars()
        .map(|c| if is_unicode_space(c) { ' ' } else { c })
        .collect();
    let home = home.to_string_lossy();
    if normalized == "~" {
        return home.into_owned();
    }
    if let Some(rest) = normalized.strip_prefix('~') {
        if rest.starts_with('/') {
            return format!("{home}{rest}");
        }
    }
    normalized
}

/// `expandPath` against the real home directory.
pub fn expand_path(file_path: &str) -> String {
    expand_path_with_home(file_path, &home_dir())
}

/// Node's `path.resolve(cwd, p)`: absolute, with `.` and `..` resolved
/// lexically and trailing separators dropped.
pub fn node_resolve(cwd: &Path, p: &str) -> PathBuf {
    let joined = cwd.join(p);
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

/// `resolveToCwd`: expand, then resolve relative paths against `cwd`.
pub fn resolve_to_cwd_with_home(file_path: &str, cwd: &Path, home: &Path) -> PathBuf {
    let expanded = expand_path_with_home(file_path, home);
    // path.resolve also normalizes an absolute path.
    node_resolve(cwd, &expanded)
}

/// `resolveToCwd` against the real home directory.
pub fn resolve_to_cwd(file_path: &str, cwd: &Path) -> PathBuf {
    resolve_to_cwd_with_home(file_path, cwd, &home_dir())
}

fn file_exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// `/ (AM|PM)\./gi` → ` $1.` (macOS screenshot names).
fn try_macos_screenshot_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(' ') {
        let after = &rest[i + 1..];
        let matched = after.len() >= 3
            && after.is_char_boundary(2)
            && (after[..2].eq_ignore_ascii_case("am") || after[..2].eq_ignore_ascii_case("pm"))
            && after.as_bytes()[2] == b'.';
        out.push_str(&rest[..i]);
        if matched {
            out.push(NARROW_NO_BREAK_SPACE);
            out.push_str(&after[..3]);
            rest = &after[3..];
        } else {
            out.push(' ');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// macOS stores filenames in NFD (decomposed) form.
fn try_nfd_variant(s: &str) -> String {
    s.nfd().collect()
}

/// macOS uses U+2019 in screenshot names like "Capture d'écran".
fn try_curly_quote_variant(s: &str) -> String {
    s.replace('\'', "\u{2019}")
}

/// `resolveReadPath` with an explicit home directory.
pub fn resolve_read_path_with_home(file_path: &str, cwd: &Path, home: &Path) -> PathBuf {
    let resolved = resolve_to_cwd_with_home(file_path, cwd, home);
    let text = resolved.to_string_lossy().into_owned();
    if file_exists(&text) {
        return resolved;
    }

    let am_pm = try_macos_screenshot_path(&text);
    if am_pm != text && file_exists(&am_pm) {
        return PathBuf::from(am_pm);
    }

    let nfd = try_nfd_variant(&text);
    if nfd != text && file_exists(&nfd) {
        return PathBuf::from(nfd);
    }

    let curly = try_curly_quote_variant(&text);
    if curly != text && file_exists(&curly) {
        return PathBuf::from(curly);
    }

    // NFD + curly quote (French macOS screenshots like "Capture d'écran").
    let nfd_curly = try_curly_quote_variant(&nfd);
    if nfd_curly != text && file_exists(&nfd_curly) {
        return PathBuf::from(nfd_curly);
    }

    resolved
}

/// `resolveReadPath`: resolve against `cwd`, then try the macOS filename
/// variants when the plain path does not exist.
pub fn resolve_read_path(file_path: &str, cwd: &Path) -> PathBuf {
    resolve_read_path_with_home(file_path, cwd, &home_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_path_handles_at_tilde_url_and_unicode_spaces() {
        let home = Path::new("/home/u");
        assert_eq!(expand_path_with_home("~", home), "/home/u");
        assert_eq!(expand_path_with_home("~/a.txt", home), "/home/u/a.txt");
        assert_eq!(expand_path_with_home("~a", home), "~a");
        assert_eq!(expand_path_with_home("@x.md", home), "x.md");
        assert_eq!(expand_path_with_home("@~/x.md", home), "/home/u/x.md");
        assert_eq!(
            expand_path_with_home("file:///tmp/a%20b.txt", home),
            "/tmp/a b.txt"
        );
        assert_eq!(expand_path_with_home("a\u{00A0}b\u{3000}c", home), "a b c");
    }

    #[test]
    fn only_triple_slash_file_urls_are_converted() {
        let home = Path::new("/h");
        assert_eq!(expand_path_with_home("file://x/y", home), "file://x/y");
        assert_eq!(expand_path_with_home("file:///a/../b", home), "/b");
        // An encoded slash makes fileURLToPath throw: the prefix is stripped instead.
        assert_eq!(expand_path_with_home("file:///a%2Fb", home), "/a%2Fb");
    }

    #[test]
    fn resolve_is_lexical_like_node_path_resolve() {
        assert_eq!(
            node_resolve(Path::new("/w/sub"), "../a/./b.txt"),
            PathBuf::from("/w/a/b.txt")
        );
        assert_eq!(node_resolve(Path::new("/w"), "/abs"), PathBuf::from("/abs"));
        assert_eq!(
            node_resolve(Path::new("/w"), "/a/b/../c/"),
            PathBuf::from("/a/c")
        );
        assert_eq!(node_resolve(Path::new("/"), ".."), PathBuf::from("/"));
    }

    #[test]
    fn am_pm_variant_inserts_narrow_no_break_space() {
        assert_eq!(
            try_macos_screenshot_path("Shot 1.2.3 at 9.41.00 pm.png"),
            "Shot 1.2.3 at 9.41.00\u{202F}pm.png"
        );
        assert_eq!(
            try_macos_screenshot_path("A AM. B PM."),
            "A\u{202F}AM. B\u{202F}PM."
        );
        assert_eq!(try_macos_screenshot_path("no match here"), "no match here");
    }

    #[test]
    fn resolve_read_path_finds_macos_variants() {
        let dir = std::env::temp_dir().join(format!("cortex-path-utils-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let home = Path::new("/nonexistent-home");

        let am = "Screenshot 2024 at 10.00.00\u{202F}AM.png";
        std::fs::write(dir.join(am), "x").unwrap();
        assert_eq!(
            resolve_read_path_with_home("Screenshot 2024 at 10.00.00 AM.png", &dir, home),
            dir.join(am)
        );

        let nfd: String = "Capture d\u{2019}écran.png".nfd().collect();
        std::fs::write(dir.join(&nfd), "x").unwrap();
        assert_eq!(
            resolve_read_path_with_home("Capture d'écran.png", &dir, home),
            dir.join(&nfd)
        );

        std::fs::write(dir.join("it\u{2019}s.txt"), "x").unwrap();
        assert_eq!(
            resolve_read_path_with_home("it's.txt", &dir, home),
            dir.join("it\u{2019}s.txt")
        );

        // Nothing matches: the plain resolution comes back.
        assert_eq!(
            resolve_read_path_with_home("missing.txt", &dir, home),
            dir.join("missing.txt")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
