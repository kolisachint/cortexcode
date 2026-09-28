//! `core/tools/render-utils.ts`: helpers the tool renderers share.

use cortexcode_ai_types::Content;
use cortexcode_code_tool_bash::shell::{sanitize_binary_output, strip_ansi};
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_images::{get_capabilities, get_image_dimensions, image_fallback};
use serde_json::Value;

/// `shortenPath`: the home directory as `~`.
pub fn shorten_path(path: &str) -> String {
    let home = cortexcode_code_tool_api::path_utils::home_dir();
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// `str(value)`: a string as itself, `null`/missing as `""`, anything else
/// as `None` (an invalid argument).
pub fn str_arg(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => Some(String::new()),
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => None,
    }
}

/// `args?.a ?? args?.b`: the first key present and not `null`.
pub fn arg_or<'a>(args: &'a Value, first: &str, second: &str) -> Option<&'a Value> {
    match args.get(first) {
        Some(Value::Null) | None => args.get(second),
        some => some,
    }
}

/// `replaceTabs`.
pub fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// `normalizeDisplayText`.
pub fn normalize_display_text(text: &str) -> String {
    text.replace('\r', "")
}

/// `getTextOutput`: the text blocks (ANSI stripped, sanitized), plus a
/// placeholder per image when images are not drawn.
pub fn get_text_output(content: Option<&[Content]>, show_images: bool) -> String {
    let Some(content) = content else {
        return String::new();
    };
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for c in content {
        match c {
            Content::Text(t) => texts.push(t),
            Content::Image(i) => images.push(i),
            _ => {}
        }
    }
    let mut output = texts
        .iter()
        .map(|t| sanitize_binary_output(&strip_ansi(&t.text)).replace('\r', ""))
        .collect::<Vec<_>>()
        .join("\n");
    let caps = get_capabilities();
    if !images.is_empty() && (caps.images.is_none() || !show_images) {
        let indicators = images
            .iter()
            .map(|img| {
                let mime = if img.media_type.is_empty() {
                    "image/unknown"
                } else {
                    img.media_type.as_str()
                };
                let dims = if img.data.is_empty() || img.media_type.is_empty() {
                    None
                } else {
                    get_image_dimensions(&img.data, &img.media_type)
                };
                image_fallback(mime, dims, None)
            })
            .collect::<Vec<_>>()
            .join("\n");
        output = if output.is_empty() {
            indicators
        } else {
            format!("{output}\n{indicators}")
        };
    }
    output
}

/// `invalidArgText`.
pub fn invalid_arg_text() -> String {
    theme().fg("error", "[invalid arg]")
}

/// Lines beyond the peek budget, as `split("\n").length` counts them.
pub fn js_line_count(text: &str) -> usize {
    text.split('\n').count()
}
