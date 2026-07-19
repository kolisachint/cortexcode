//! ANSI escape-sequence extraction and SGR/OSC-8 state tracking.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `utils.ts`
//! (`extractAnsiCode`, `AnsiCodeTracker`, OSC-8 hyperlink helpers).

/// Find the ANSI escape sequence starting at byte offset `pos`, if any.
/// Recognizes CSI (`ESC [ ... m/G/K/H/J`), OSC (`ESC ] ... BEL` or `ESC ] ... ESC \`),
/// and APC (`ESC _ ... BEL` or `ESC _ ... ESC \`) sequences.
pub fn extract_ansi_code(s: &str, pos: usize) -> Option<(&str, usize)> {
    let bytes = s.as_bytes();
    if pos >= bytes.len() || bytes[pos] != 0x1b {
        return None;
    }
    let next = bytes.get(pos + 1).copied();

    match next {
        Some(b'[') => {
            let mut j = pos + 2;
            while j < bytes.len() && !matches!(bytes[j], b'm' | b'G' | b'K' | b'H' | b'J') {
                j += 1;
            }
            if j < bytes.len() {
                Some((&s[pos..=j], j + 1 - pos))
            } else {
                None
            }
        }
        Some(b']') | Some(b'_') => {
            let mut j = pos + 2;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    return Some((&s[pos..=j], j + 1 - pos));
                }
                if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                    return Some((&s[pos..=j + 1], j + 2 - pos));
                }
                j += 1;
            }
            None
        }
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Osc8Terminator {
    Bel,
    St,
}

impl Osc8Terminator {
    fn as_str(self) -> &'static str {
        match self {
            Osc8Terminator::Bel => "\x07",
            Osc8Terminator::St => "\x1b\\",
        }
    }
}

#[derive(Clone)]
struct ActiveHyperlink {
    params: String,
    url: String,
    terminator: Osc8Terminator,
}

/// Parses an OSC-8 hyperlink escape (`ESC]8;params;url<terminator>`).
/// Returns `None` if `ansi_code` isn't an OSC-8 sequence, `Some(None)` for a
/// close marker (empty URL), `Some(Some(...))` for an open marker.
fn parse_osc8_hyperlink(ansi_code: &str) -> Option<Option<(String, String, Osc8Terminator)>> {
    if !ansi_code.starts_with("\x1b]8;") {
        return None;
    }
    let terminator = if ansi_code.ends_with('\x07') {
        Osc8Terminator::Bel
    } else {
        Osc8Terminator::St
    };
    let trim_len = if terminator == Osc8Terminator::Bel {
        1
    } else {
        2
    };
    let body = &ansi_code[4..ansi_code.len() - trim_len];
    let sep = body.find(';')?;
    let params = &body[..sep];
    let url = &body[sep + 1..];
    if url.is_empty() {
        Some(None)
    } else {
        Some(Some((params.to_string(), url.to_string(), terminator)))
    }
}

fn format_osc8_hyperlink(h: &ActiveHyperlink) -> String {
    format!("\x1b]8;{};{}{}", h.params, h.url, h.terminator.as_str())
}

fn format_osc8_close(terminator: Osc8Terminator) -> String {
    format!("\x1b]8;;{}", terminator.as_str())
}

/// Tracks active SGR attributes (and an OSC-8 hyperlink) across line breaks
/// so wrapped/truncated output can re-open styling on continuation lines.
#[derive(Default, Clone)]
pub struct AnsiCodeTracker {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
    fg_color: Option<String>,
    bg_color: Option<String>,
    active_hyperlink: Option<ActiveHyperlink>,
}

impl AnsiCodeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one extracted ANSI code (as returned by [`extract_ansi_code`]) into the tracker.
    pub fn process(&mut self, ansi_code: &str) {
        if let Some(hyperlink) = parse_osc8_hyperlink(ansi_code) {
            self.active_hyperlink = hyperlink.map(|(params, url, terminator)| ActiveHyperlink {
                params,
                url,
                terminator,
            });
            return;
        }

        if !ansi_code.ends_with('m') {
            return;
        }
        let Some(params_str) = ansi_code
            .strip_prefix("\x1b[")
            .and_then(|s| s.strip_suffix('m'))
        else {
            return;
        };
        if params_str.is_empty() || params_str == "0" {
            self.reset();
            return;
        }

        let parts: Vec<&str> = params_str.split(';').collect();
        let mut i = 0;
        while i < parts.len() {
            let Ok(code) = parts[i].parse::<i32>() else {
                i += 1;
                continue;
            };

            if code == 38 || code == 48 {
                if parts.get(i + 1) == Some(&"5") && parts.get(i + 2).is_some() {
                    let color_code = format!("{};{};{}", parts[i], parts[i + 1], parts[i + 2]);
                    if code == 38 {
                        self.fg_color = Some(color_code);
                    } else {
                        self.bg_color = Some(color_code);
                    }
                    i += 3;
                    continue;
                } else if parts.get(i + 1) == Some(&"2") && parts.get(i + 4).is_some() {
                    let color_code = format!(
                        "{};{};{};{};{}",
                        parts[i],
                        parts[i + 1],
                        parts[i + 2],
                        parts[i + 3],
                        parts[i + 4]
                    );
                    if code == 38 {
                        self.fg_color = Some(color_code);
                    } else {
                        self.bg_color = Some(color_code);
                    }
                    i += 5;
                    continue;
                }
            }

            match code {
                0 => self.reset(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                5 => self.blink = true,
                7 => self.inverse = true,
                8 => self.hidden = true,
                9 => self.strikethrough = true,
                21 => self.bold = false,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                25 => self.blink = false,
                27 => self.inverse = false,
                28 => self.hidden = false,
                29 => self.strikethrough = false,
                39 => self.fg_color = None,
                49 => self.bg_color = None,
                _ => {
                    if (30..=37).contains(&code) || (90..=97).contains(&code) {
                        self.fg_color = Some(code.to_string());
                    } else if (40..=47).contains(&code) || (100..=107).contains(&code) {
                        self.bg_color = Some(code.to_string());
                    }
                }
            }
            i += 1;
        }
    }

    fn reset(&mut self) {
        self.bold = false;
        self.dim = false;
        self.italic = false;
        self.underline = false;
        self.blink = false;
        self.inverse = false;
        self.hidden = false;
        self.strikethrough = false;
        self.fg_color = None;
        self.bg_color = None;
    }

    /// Clear all state (including the hyperlink) for reuse.
    pub fn clear(&mut self) {
        self.reset();
        self.active_hyperlink = None;
    }

    /// Codes that reproduce the current styling state (for re-opening on a new line).
    pub fn active_codes(&self) -> String {
        let mut codes = Vec::new();
        if self.bold {
            codes.push("1".to_string());
        }
        if self.dim {
            codes.push("2".to_string());
        }
        if self.italic {
            codes.push("3".to_string());
        }
        if self.underline {
            codes.push("4".to_string());
        }
        if self.blink {
            codes.push("5".to_string());
        }
        if self.inverse {
            codes.push("7".to_string());
        }
        if self.hidden {
            codes.push("8".to_string());
        }
        if self.strikethrough {
            codes.push("9".to_string());
        }
        if let Some(fg) = &self.fg_color {
            codes.push(fg.clone());
        }
        if let Some(bg) = &self.bg_color {
            codes.push(bg.clone());
        }

        let mut result = if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        };
        if let Some(h) = &self.active_hyperlink {
            result.push_str(&format_osc8_hyperlink(h));
        }
        result
    }

    pub fn has_active_codes(&self) -> bool {
        self.bold
            || self.dim
            || self.italic
            || self.underline
            || self.blink
            || self.inverse
            || self.hidden
            || self.strikethrough
            || self.fg_color.is_some()
            || self.bg_color.is_some()
            || self.active_hyperlink.is_some()
    }

    /// Codes needed to close attributes that must not bleed past line end
    /// (underline, and any open OSC-8 hyperlink).
    pub fn line_end_reset(&self) -> String {
        let mut result = String::new();
        if self.underline {
            result.push_str("\x1b[24m");
        }
        if let Some(h) = &self.active_hyperlink {
            result.push_str(&format_osc8_close(h.terminator));
        }
        result
    }
}

/// Feed every ANSI code found in `text` into `tracker`.
pub fn update_tracker_from_text(text: &str, tracker: &mut AnsiCodeTracker) {
    let mut i = 0;
    while i < text.len() {
        if let Some((code, len)) = extract_ansi_code(text, i) {
            tracker.process(code);
            i += len;
        } else {
            i += char_len_at(text, i);
        }
    }
}

fn char_len_at(s: &str, byte_idx: usize) -> usize {
    s[byte_idx..]
        .chars()
        .next()
        .map(|c| c.len_utf8())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_ansi_code_csi() {
        let s = "\x1b[31mhello";
        let (code, len) = extract_ansi_code(s, 0).unwrap();
        assert_eq!(code, "\x1b[31m");
        assert_eq!(len, 5);
    }

    #[test]
    fn test_extract_ansi_code_none_for_plain_text() {
        assert!(extract_ansi_code("hello", 0).is_none());
    }

    #[test]
    fn test_extract_ansi_code_osc_bel() {
        let s = "\x1b]8;;https://x\x07link\x1b]8;;\x07";
        let (code, len) = extract_ansi_code(s, 0).unwrap();
        assert_eq!(code, "\x1b]8;;https://x\x07");
        assert_eq!(len, code.len());
    }

    #[test]
    fn test_ansi_tracker_bold_and_reset() {
        let mut t = AnsiCodeTracker::new();
        t.process("\x1b[1m");
        assert!(t.has_active_codes());
        assert_eq!(t.active_codes(), "\x1b[1m");
        t.process("\x1b[0m");
        assert!(!t.has_active_codes());
    }

    #[test]
    fn test_ansi_tracker_256_color() {
        let mut t = AnsiCodeTracker::new();
        t.process("\x1b[38;5;196m");
        assert_eq!(t.active_codes(), "\x1b[38;5;196m");
    }

    #[test]
    fn test_ansi_tracker_underline_line_end_reset() {
        let mut t = AnsiCodeTracker::new();
        t.process("\x1b[4m");
        assert_eq!(t.line_end_reset(), "\x1b[24m");
    }

    #[test]
    fn test_ansi_tracker_hyperlink_roundtrip() {
        let mut t = AnsiCodeTracker::new();
        t.process("\x1b]8;;https://example.com\x07");
        assert!(t.active_codes().contains("https://example.com"));
        t.process("\x1b]8;;\x07");
        assert!(!t.has_active_codes());
    }
}
