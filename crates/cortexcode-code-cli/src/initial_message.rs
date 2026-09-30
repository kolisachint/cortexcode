//! The initial prompt (print and interactive modes). Ports hoocode
//! `cli/initial-message.ts`, `cli/file-processor.ts` (`@file` args) and the
//! part of `core/tools/path-utils.ts` they use. Tests port
//! `test/initial-message.test.ts`, and the `processFileArguments` cases of
//! `test/block-images.test.ts` and `test/image-resize-callers.test.ts`.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use cortexcode_ai_types::ImageContent;
use cortexcode_code_media::{format_dimension_note, resize_image, ImageResizeOptions};

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

/// `ProcessedFiles`: the `<file>` text and the image attachments of the
/// `@file` arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcessedFiles {
    pub text: String,
    pub images: Vec<ImageContent>,
}

/// `processFileArguments`. Images are auto-resized to 2000x2000 max unless
/// `auto_resize_images` is off. On failure returns the message hoocode prints
/// (in red) before exiting 1.
pub fn process_file_arguments(
    file_args: &[String],
    cwd: &Path,
    home: &Path,
    auto_resize_images: bool,
) -> Result<ProcessedFiles, String> {
    process_file_arguments_with(
        file_args,
        cwd,
        home,
        auto_resize_images.then(ImageResizeOptions::default),
    )
}

/// [`process_file_arguments`] with explicit resize limits (`None`: no resize).
pub fn process_file_arguments_with(
    file_args: &[String],
    cwd: &Path,
    home: &Path,
    resize: Option<ImageResizeOptions>,
) -> Result<ProcessedFiles, String> {
    let mut out = ProcessedFiles::default();
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
        if let Some(mime_type) = sniff_image_mime(&bytes) {
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let (attachment, dimension_note) = match resize {
                Some(options) => {
                    let Some(resized) = resize_image(&data, Some(mime_type), options) else {
                        out.text.push_str(&format!(
                            "<file name=\"{shown}\">[Image omitted: could not be resized below the inline image size limit.]</file>\n"
                        ));
                        continue;
                    };
                    let note = format_dimension_note(&resized);
                    (
                        ImageContent {
                            data: resized.data,
                            media_type: resized.mime_type,
                        },
                        note,
                    )
                }
                None => (
                    ImageContent {
                        data,
                        media_type: mime_type.to_string(),
                    },
                    None,
                ),
            };
            out.images.push(attachment);
            out.text.push_str(&format!(
                "<file name=\"{shown}\">{}</file>\n",
                dimension_note.unwrap_or_default()
            ));
            continue;
        }
        // Node's readFile(..., "utf-8") replaces invalid sequences.
        let content = String::from_utf8_lossy(&bytes);
        out.text
            .push_str(&format!("<file name=\"{shown}\">\n{content}\n</file>\n"));
    }
    Ok(out)
}

/// `prepareInitialMessage` (`main.ts`): the `@file` arguments, then
/// [`build_initial_message`]. Returns the initial message and the images sent
/// with it; the first CLI message is removed from `messages`.
pub fn prepare_initial_message(
    file_args: &[String],
    messages: &mut Vec<String>,
    stdin_content: Option<&str>,
    auto_resize_images: bool,
    cwd: &Path,
    home: &Path,
) -> Result<(Option<String>, Vec<ImageContent>), String> {
    if file_args.is_empty() {
        return Ok((
            build_initial_message(messages, None, stdin_content),
            Vec::new(),
        ));
    }
    let files = process_file_arguments(file_args, cwd, home, auto_resize_images)?;
    Ok((
        build_initial_message(messages, Some(&files.text), stdin_content),
        files.images,
    ))
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

    // Ports of `coding-agent/test/initial-message.test.ts`.

    #[test]
    fn merges_piped_stdin_with_the_first_cli_message() {
        let mut m = msgs(&["Summarize the text given"]);
        assert_eq!(
            build_initial_message(&mut m, None, Some("README contents\n")).as_deref(),
            Some("README contents\nSummarize the text given")
        );
        assert!(m.is_empty());
    }

    #[test]
    fn uses_stdin_as_the_initial_prompt_without_a_cli_message() {
        let mut m = Vec::new();
        assert_eq!(
            build_initial_message(&mut m, None, Some("README contents")).as_deref(),
            Some("README contents")
        );
        assert!(m.is_empty());
    }

    #[test]
    fn combines_stdin_file_text_and_first_cli_message() {
        let mut m = msgs(&["Explain it", "Second message"]);
        assert_eq!(
            build_initial_message(&mut m, Some("file\n"), Some("stdin\n")).as_deref(),
            Some("stdin\nfile\nExplain it")
        );
        assert_eq!(m, msgs(&["Second message"]));
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

    /// A real 1x1 PNG: file-type needs the IHDR/IDAT chunks, not just the signature.
    const TINY_PNG_BASE64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

    fn tiny_png() -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(TINY_PNG_BASE64)
            .unwrap()
    }

    fn process(dir: &Path, args: &[&str]) -> Result<ProcessedFiles, String> {
        process_file_arguments(&msgs(args), dir, Path::new("/nohome"), true)
    }

    #[test]
    fn file_arguments_wrap_text_and_skip_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        std::fs::write(dir.join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.join("empty.txt"), "").unwrap();
        let files = process(dir, &["a.txt", "empty.txt"]).unwrap();
        assert_eq!(
            files.text,
            format!(
                "<file name=\"{}\">\nalpha\n</file>\n",
                dir.join("a.txt").display()
            )
        );
        assert!(files.images.is_empty());
        assert_eq!(
            process(dir, &["missing.txt"]).unwrap_err(),
            format!(
                "Error: File not found: {}",
                dir.join("missing.txt").display()
            )
        );
    }

    #[test]
    fn image_arguments_become_attachments_with_an_empty_file_tag() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        std::fs::write(dir.join("i.png"), tiny_png()).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha").unwrap();
        let expected_text = format!(
            "<file name=\"{}\"></file>\n<file name=\"{}\">\nalpha\n</file>\n",
            dir.join("i.png").display(),
            dir.join("a.txt").display()
        );
        let expected_images = vec![ImageContent {
            data: TINY_PNG_BASE64.to_string(),
            media_type: "image/png".to_string(),
        }];
        // Within limits: passed through unchanged, with or without auto-resize.
        for auto_resize in [true, false] {
            let files = process_file_arguments(
                &msgs(&["i.png", "a.txt"]),
                dir,
                Path::new("/nohome"),
                auto_resize,
            )
            .unwrap();
            assert_eq!(files.text, expected_text);
            assert_eq!(files.images, expected_images);
        }
    }

    #[test]
    fn resized_image_arguments_carry_the_dimension_note() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let png = cortexcode_code_media::clipboard_image::rgba_to_png(2, 1, vec![255; 8]).unwrap();
        std::fs::write(dir.join("wide.png"), png).unwrap();
        let options = ImageResizeOptions {
            max_width: 1,
            max_height: 1,
            ..Default::default()
        };
        let files = process_file_arguments_with(
            &msgs(&["wide.png"]),
            dir,
            Path::new("/nohome"),
            Some(options),
        )
        .unwrap();
        assert_eq!(
            files.text,
            format!(
                "<file name=\"{}\">[Image: original 2x1, displayed at 1x1. Multiply coordinates by 2.00 to map to original image.]</file>\n",
                dir.join("wide.png").display()
            )
        );
        assert_eq!(files.images.len(), 1);
        assert_eq!(files.images[0].media_type, "image/png");
    }

    #[test]
    fn prepare_initial_message_sends_file_text_and_images_with_the_first_message() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        std::fs::write(dir.join("i.png"), tiny_png()).unwrap();
        let mut m = msgs(&["describe", "next"]);
        let (message, images) = prepare_initial_message(
            &msgs(&["i.png"]),
            &mut m,
            Some("stdin\n"),
            true,
            dir,
            Path::new("/nohome"),
        )
        .unwrap();
        assert_eq!(
            message.unwrap(),
            format!(
                "stdin\n<file name=\"{}\"></file>\ndescribe",
                dir.join("i.png").display()
            )
        );
        assert_eq!(images.len(), 1);
        assert_eq!(m, msgs(&["next"]));

        let mut m = msgs(&["only"]);
        let (message, images) =
            prepare_initial_message(&[], &mut m, None, true, dir, Path::new("/nohome")).unwrap();
        assert_eq!(message.as_deref(), Some("only"));
        assert!(images.is_empty());
    }

    // Ports of `test/block-images.test.ts` ("processFileArguments").

    #[test]
    fn should_always_process_images_filtering_happens_at_convert_to_llm_layer() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.png"), tiny_png()).unwrap();
        let files = process(dir.path(), &["test.png"]).unwrap();
        assert_eq!(files.images.len(), 1);
    }

    #[test]
    fn should_process_text_files_normally() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "Hello, world!").unwrap();
        let files = process(dir.path(), &["test.txt"]).unwrap();
        assert!(files.images.is_empty());
        assert!(files.text.contains("Hello, world!"));
    }

    // Port of `test/image-resize-callers.test.ts` (file processor case): the
    // TS test mocks `resizeImage` to fail; here the limit is out of reach.

    #[test]
    fn file_processor_omits_image_attachments_when_auto_resize_cannot_produce_a_safe_image() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.png"), tiny_png()).unwrap();
        let options = ImageResizeOptions {
            max_bytes: 1,
            ..Default::default()
        };
        let files = process_file_arguments_with(
            &msgs(&["test.png"]),
            dir.path(),
            Path::new("/nohome"),
            Some(options),
        )
        .unwrap();
        assert!(files.images.is_empty());
        assert!(files.text.contains("Image omitted"));
        assert_eq!(
            files.text,
            format!(
                "<file name=\"{}\">[Image omitted: could not be resized below the inline image size limit.]</file>\n",
                dir.path().join("test.png").display()
            )
        );
    }
}
