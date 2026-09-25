//! The initial prompt for non-interactive runs. Ports hoocode
//! `cli/initial-message.ts`, `cli/file-processor.ts` (`@file` args) and the
//! part of `core/tools/path-utils.ts` they use.

use std::path::{Component, Path, PathBuf};

/// `buildInitialMessage`: stdin content, then `@file` text, then the first CLI
/// message, concatenated without separators. The first message is removed from
/// `messages`; the rest are sent as separate prompts.
pub fn build_initial_message(
    messages: &mut Vec<String>,
    file_text: Option<&str>,
    stdin_content: Option<&str>,
) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(stdin) = stdin_content {
        parts.push(stdin);
    }
    if let Some(text) = file_text.filter(|t| !t.is_empty()) {
        parts.push(text);
    }
    let first = if messages.is_empty() {
        None
    } else {
        Some(messages.remove(0))
    };
    if let Some(first) = &first {
        parts.push(first);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.concat())
    }
}

/// `readPipedStdin`: the whole of stdin, trimmed; `None` when empty.
pub fn normalize_piped_stdin(data: &str) -> Option<String> {
    let trimmed = data.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

const UNICODE_SPACES: [char; 15] = [
    '\u{00A0}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}',
    '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{202F}', '\u{205F}', '\u{3000}',
];

/// `expandPath`: file URL → path, strip a leading `@`, normalize unicode
/// spaces, expand `~`.
pub fn expand_path(file_path: &str, home: &Path) -> String {
    let path = file_path
        .strip_prefix("file://")
        .map(|rest| {
            url::Url::parse(file_path)
                .ok()
                .and_then(|u| u.to_file_path().ok())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| rest.to_string())
        })
        .unwrap_or_else(|| file_path.to_string());
    let path = path.strip_prefix('@').unwrap_or(&path);
    let normalized: String = path
        .chars()
        .map(|c| if UNICODE_SPACES.contains(&c) { ' ' } else { c })
        .collect();
    if normalized == "~" {
        return home.to_string_lossy().into_owned();
    }
    if let Some(rest) = normalized.strip_prefix("~/") {
        return format!("{}/{}", home.to_string_lossy(), rest);
    }
    normalized
}

/// Node's `path.resolve(cwd, p)`: absolute, with `.` and `..` resolved lexically.
fn resolve_path(cwd: &Path, p: &str) -> PathBuf {
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
    out
}

/// `resolveReadPath`: resolve against `cwd`, then try the macOS screenshot
/// variants (narrow no-break space before AM/PM, curly apostrophe). The NFD
/// variant needs Unicode normalization and arrives with the tools port (10.2).
pub fn resolve_read_path(file_path: &str, cwd: &Path, home: &Path) -> PathBuf {
    let resolved = resolve_path(cwd, &expand_path(file_path, home));
    if resolved.exists() {
        return resolved;
    }
    let text = resolved.to_string_lossy().into_owned();
    let am_pm = am_pm_variant(&text);
    if am_pm != text && Path::new(&am_pm).exists() {
        return PathBuf::from(am_pm);
    }
    let curly = text.replace('\'', "\u{2019}");
    if curly != text && Path::new(&curly).exists() {
        return PathBuf::from(curly);
    }
    resolved
}

/// `/ (AM|PM)\./gi` → ` $1.`
fn am_pm_variant(s: &str) -> String {
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
            out.push('\u{202F}');
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

/// Supported image types (`IMAGE_MIME_TYPES` in `utils/mime.ts`), sniffed from
/// the file's magic bytes like `file-type` does.
fn sniff_image_mime(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// `processFileArguments` for text files. On failure returns the message
/// hoocode prints (in red) before exiting 1. Images need resizing
/// (`code-media`, 11.4) and are reported as not yet supported.
pub fn process_file_arguments(
    file_args: &[String],
    cwd: &Path,
    home: &Path,
) -> Result<String, String> {
    let mut text = String::new();
    for file_arg in file_args {
        let absolute = resolve_read_path(file_arg, cwd, home);
        let shown = absolute.display();
        let meta =
            std::fs::metadata(&absolute).map_err(|_| format!("Error: File not found: {shown}"))?;
        if meta.len() == 0 {
            continue;
        }
        let bytes = std::fs::read(&absolute)
            .map_err(|e| format!("Error: Could not read file {shown}: {e}"))?;
        if sniff_image_mime(&bytes).is_some() {
            return Err(format!(
                "Error: image @file arguments are not yet supported by cortex: {shown}"
            ));
        }
        // Node's readFile(..., "utf-8") replaces invalid sequences.
        let content = String::from_utf8_lossy(&bytes);
        text.push_str(&format!("<file name=\"{shown}\">\n{content}\n</file>\n"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msgs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn initial_message_takes_the_first_message_only() {
        let mut m = msgs(&["one", "two"]);
        assert_eq!(
            build_initial_message(&mut m, None, None).as_deref(),
            Some("one")
        );
        assert_eq!(m, msgs(&["two"]));
    }

    #[test]
    fn initial_message_concatenates_stdin_files_and_first_message() {
        let mut m = msgs(&["ask"]);
        assert_eq!(
            build_initial_message(&mut m, Some("<file>\n"), Some("piped")).as_deref(),
            Some("piped<file>\nask")
        );
        assert!(m.is_empty());
    }

    #[test]
    fn initial_message_is_none_without_input() {
        assert_eq!(build_initial_message(&mut Vec::new(), Some(""), None), None);
    }

    #[test]
    fn piped_stdin_is_trimmed_and_empty_is_none() {
        assert_eq!(normalize_piped_stdin("  hi \n").as_deref(), Some("hi"));
        assert_eq!(normalize_piped_stdin(" \n"), None);
    }

    #[test]
    fn expand_path_handles_at_tilde_url_and_unicode_spaces() {
        let home = Path::new("/home/u");
        assert_eq!(expand_path("~", home), "/home/u");
        assert_eq!(expand_path("~/a.txt", home), "/home/u/a.txt");
        assert_eq!(expand_path("@x.md", home), "x.md");
        assert_eq!(expand_path("file:///tmp/a%20b.txt", home), "/tmp/a b.txt");
        assert_eq!(expand_path("a\u{00A0}b", home), "a b");
    }

    #[test]
    fn resolve_is_lexical_like_node_path_resolve() {
        assert_eq!(
            resolve_path(Path::new("/w/sub"), "../a/./b.txt"),
            PathBuf::from("/w/a/b.txt")
        );
        assert_eq!(resolve_path(Path::new("/w"), "/abs"), PathBuf::from("/abs"));
    }

    #[test]
    fn am_pm_variant_inserts_narrow_no_break_space() {
        assert_eq!(
            am_pm_variant("Shot 1.2.3 at 9.41.00 pm.png"),
            "Shot 1.2.3 at 9.41.00\u{202F}pm.png"
        );
        assert_eq!(am_pm_variant("no match here"), "no match here");
    }

    #[test]
    fn file_arguments_wrap_text_and_skip_empty_files() {
        let dir = std::env::temp_dir().join(format!("cortex-fileargs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.join("empty.txt"), "").unwrap();
        let text =
            process_file_arguments(&msgs(&["a.txt", "empty.txt"]), &dir, Path::new("/nohome"))
                .unwrap();
        assert_eq!(
            text,
            format!(
                "<file name=\"{}\">\nalpha\n</file>\n",
                dir.join("a.txt").display()
            )
        );
        let err = process_file_arguments(&msgs(&["missing.txt"]), &dir, Path::new("/nohome"))
            .unwrap_err();
        assert_eq!(
            err,
            format!(
                "Error: File not found: {}",
                dir.join("missing.txt").display()
            )
        );
        std::fs::write(
            dir.join("i.png"),
            [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0],
        )
        .unwrap();
        assert!(
            process_file_arguments(&msgs(&["i.png"]), &dir, Path::new("/nohome"))
                .unwrap_err()
                .contains("not yet supported")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
