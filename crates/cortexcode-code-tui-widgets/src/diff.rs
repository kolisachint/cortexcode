//! `components/diff.ts`: a unified-ish diff (`+12 line`, `-12 line`,
//! ` 12 line`) coloured, with word-level inverse on a one-line change.

use cortexcode_code_tui_theme::theme;

use crate::jsdiff::diff_words;
use crate::render_utils::replace_tabs;

struct DiffLine<'a> {
    prefix: char,
    line_num: &'a str,
    content: &'a str,
}

/// `parseDiffLine`: `/^([+-\s])(\s*\d*)\s(.*)$/`.
fn parse_diff_line(line: &str) -> Option<DiffLine<'_>> {
    let mut chars = line.char_indices();
    let (_, prefix) = chars.next()?;
    if !(prefix == '+' || prefix == '-' || crate::is_js_space(prefix)) {
        return None;
    }
    let rest = &line[prefix.len_utf8()..];
    // `(\s*\d*)\s(.*)`: the greedy groups backtrack until a whitespace follows.
    let spaces = rest.len() - rest.trim_start_matches(crate::is_js_space).len();
    let digits = rest[spaces..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    let mut num_end = spaces + digits;
    loop {
        let after = &rest[num_end..];
        if let Some(c) = after
            .chars()
            .next()
            .filter(|c| crate::is_js_space(*c) && *c != '\n')
        {
            // `.` excludes line terminators; content must not contain them.
            let content = &after[c.len_utf8()..];
            if !content.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
                return Some(DiffLine {
                    prefix,
                    line_num: &rest[..num_end],
                    content,
                });
            }
        }
        if num_end == 0 {
            return None;
        }
        // Give back one character of the number group and retry.
        let back = rest[..num_end].chars().next_back().unwrap().len_utf8();
        num_end -= back;
    }
}

/// `renderIntraLineDiff`: the changed words inverted.
fn render_intra_line_diff(old: &str, new: &str) -> (String, String) {
    let t = theme();
    let mut removed = String::new();
    let mut added = String::new();
    let (mut first_removed, mut first_added) = (true, true);
    for part in diff_words(old, new) {
        if part.removed {
            let mut value = part.value.as_str();
            if first_removed {
                let ws = value.len() - value.trim_start_matches(crate::is_js_space).len();
                removed.push_str(&value[..ws]);
                value = &value[ws..];
                first_removed = false;
            }
            if !value.is_empty() {
                removed.push_str(&t.inverse(value));
            }
        } else if part.added {
            let mut value = part.value.as_str();
            if first_added {
                let ws = value.len() - value.trim_start_matches(crate::is_js_space).len();
                added.push_str(&value[..ws]);
                value = &value[ws..];
                first_added = false;
            }
            if !value.is_empty() {
                added.push_str(&t.inverse(value));
            }
        } else {
            removed.push_str(&part.value);
            added.push_str(&part.value);
        }
    }
    (removed, added)
}

/// `renderDiff`.
pub fn render_diff(diff_text: &str) -> String {
    let t = theme();
    let lines: Vec<&str> = diff_text.split('\n').collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let Some(parsed) = parse_diff_line(line) else {
            result.push(t.fg("toolDiffContext", line));
            i += 1;
            continue;
        };
        match parsed.prefix {
            '-' => {
                let mut removed = Vec::new();
                while i < lines.len() {
                    match parse_diff_line(lines[i]) {
                        Some(p) if p.prefix == '-' => removed.push(p),
                        _ => break,
                    }
                    i += 1;
                }
                let mut added = Vec::new();
                while i < lines.len() {
                    match parse_diff_line(lines[i]) {
                        Some(p) if p.prefix == '+' => added.push(p),
                        _ => break,
                    }
                    i += 1;
                }
                if removed.len() == 1 && added.len() == 1 {
                    let (r, a) = (&removed[0], &added[0]);
                    let (removed_line, added_line) =
                        render_intra_line_diff(&replace_tabs(r.content), &replace_tabs(a.content));
                    result.push(t.fg(
                        "toolDiffRemoved",
                        &format!("-{} {removed_line}", r.line_num),
                    ));
                    result.push(t.fg("toolDiffAdded", &format!("+{} {added_line}", a.line_num)));
                } else {
                    for r in &removed {
                        result.push(t.fg(
                            "toolDiffRemoved",
                            &format!("-{} {}", r.line_num, replace_tabs(r.content)),
                        ));
                    }
                    for a in &added {
                        result.push(t.fg(
                            "toolDiffAdded",
                            &format!("+{} {}", a.line_num, replace_tabs(a.content)),
                        ));
                    }
                }
            }
            '+' => {
                result.push(t.fg(
                    "toolDiffAdded",
                    &format!("+{} {}", parsed.line_num, replace_tabs(parsed.content)),
                ));
                i += 1;
            }
            _ => {
                result.push(t.fg(
                    "toolDiffContext",
                    &format!(" {} {}", parsed.line_num, replace_tabs(parsed.content)),
                ));
                i += 1;
            }
        }
    }
    result.join("\n")
}
