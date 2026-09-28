//! The few `chalk` modifiers the theme uses (`bold`, `italic`, `underline`,
//! `inverse`, `strikethrough`), with chalk's nesting behavior: an inner close
//! code re-opens the style, and every line break closes and re-opens it so a
//! style never bleeds across lines.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Whether styles are emitted (chalk's `level > 0`). On by default; the app
/// turns it off when stdout has no color support.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// chalk's `applyStyle` for a single modifier.
pub fn style(open: &str, close: &str, text: &str) -> String {
    if !enabled() || text.is_empty() {
        return text.to_string();
    }
    let mut body = text.to_string();
    if body.contains('\x1b') {
        body = body.replace(close, &format!("{close}{open}"));
    }
    if body.contains('\n') {
        let mut out = String::with_capacity(body.len());
        let mut rest = body.as_str();
        while let Some(i) = rest.find('\n') {
            let (line, cr) = match rest[..i].strip_suffix('\r') {
                Some(line) => (line, "\r\n"),
                None => (&rest[..i], "\n"),
            };
            out.push_str(line);
            out.push_str(close);
            out.push_str(cr);
            out.push_str(open);
            rest = &rest[i + 1..];
        }
        out.push_str(rest);
        body = out;
    }
    format!("{open}{body}{close}")
}

pub fn bold(text: &str) -> String {
    style("\x1b[1m", "\x1b[22m", text)
}

pub fn italic(text: &str) -> String {
    style("\x1b[3m", "\x1b[23m", text)
}

pub fn underline(text: &str) -> String {
    style("\x1b[4m", "\x1b[24m", text)
}

pub fn inverse(text: &str) -> String {
    style("\x1b[7m", "\x1b[27m", text)
}

pub fn strikethrough(text: &str) -> String {
    style("\x1b[9m", "\x1b[29m", text)
}
