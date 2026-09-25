//! The initial prompt for non-interactive runs. Ports hoocode
//! `cli/initial-message.ts`, `cli/file-processor.ts` (`@file` args) and the
//! part of `core/tools/path-utils.ts` they use.

use std::path::{Path, PathBuf};

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

/// `resolveReadPath` (`core/tools/path-utils.ts`), shared with the read tool.
pub fn resolve_read_path(file_path: &str, cwd: &Path, home: &Path) -> PathBuf {
    cortexcode_code_tool_api::path_utils::resolve_read_path_with_home(file_path, cwd, home)
}

/// Supported image types (`IMAGE_MIME_TYPES` in `utils/mime.ts`), sniffed from
/// the file's magic bytes like `file-type` does.
fn sniff_image_mime(head: &[u8]) -> Option<&'static str> {
    cortexcode_code_media::detect_supported_image_mime_type(head)
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
        // A real 1x1 PNG: file-type needs the IHDR/IDAT chunks, not just the signature.
        let png: [u8; 70] = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x60, 0x60, 0x60, 0xF8, 0x0F, 0x00, 0x01, 0x04, 0x01, 0x00, 0x5F, 0xE5,
            0xC3, 0x4B, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(dir.join("i.png"), png).unwrap();
        assert!(
            process_file_arguments(&msgs(&["i.png"]), &dir, Path::new("/nohome"))
                .unwrap_err()
                .contains("not yet supported")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
