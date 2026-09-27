//! The pin's `test/test-themes.ts` markdown theme (chalk at level 3).

#![allow(dead_code)]

use cortexcode_tui_components::MarkdownTheme;

pub fn sgr(open: &str, close: &str) -> Box<dyn Fn(&str) -> String> {
    let (open, close) = (open.to_string(), close.to_string());
    Box::new(move |t: &str| chalk(t, &open, &close))
}

/// chalk's wrap: re-open the style after every inner close of the same kind.
pub fn chalk(t: &str, open: &str, close: &str) -> String {
    if t.is_empty() {
        return String::new();
    }
    let inner = t.replace(close, &format!("{close}{open}"));
    // chalk also re-opens across line breaks.
    let inner = inner.replace('\n', &format!("{close}\n{open}"));
    format!("{open}{inner}{close}")
}

/// `defaultMarkdownTheme`.
pub fn theme() -> MarkdownTheme {
    MarkdownTheme {
        heading: Box::new(|t: &str, _| {
            chalk(&chalk(t, "\x1b[36m", "\x1b[39m"), "\x1b[1m", "\x1b[22m")
        }),
        heading_block: None,
        link: sgr("\x1b[34m", "\x1b[39m"),
        link_url: sgr("\x1b[2m", "\x1b[22m"),
        code: sgr("\x1b[33m", "\x1b[39m"),
        code_block: sgr("\x1b[32m", "\x1b[39m"),
        code_block_border: sgr("\x1b[2m", "\x1b[22m"),
        quote: sgr("\x1b[3m", "\x1b[23m"),
        quote_border: sgr("\x1b[2m", "\x1b[22m"),
        hr: sgr("\x1b[2m", "\x1b[22m"),
        list_bullet: sgr("\x1b[36m", "\x1b[39m"),
        bold: sgr("\x1b[1m", "\x1b[22m"),
        italic: sgr("\x1b[3m", "\x1b[23m"),
        strikethrough: sgr("\x1b[9m", "\x1b[29m"),
        underline: sgr("\x1b[4m", "\x1b[24m"),
        highlight_code: None,
        code_block_indent: None,
    }
}
