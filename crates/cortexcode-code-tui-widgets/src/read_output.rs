//! `components/read-output.ts`: file content with line numbers, in the
//! diff view's style (` 12 content`).

use cortexcode_code_tui_theme::theme;

/// `renderReadOutput`: number each line from `start_line` (1-indexed).
pub fn render_read_output(content: &str, start_line: i64) -> Vec<String> {
    let lines: Vec<&str> = content.split('\n').collect();
    let end_line = start_line + lines.len() as i64 - 1;
    let max_digits = end_line.to_string().len().max(2);
    let t = theme();
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let number = format!("{:>w$}", start_line + index as i64, w = max_digits);
            format!("{} {}", t.fg("dim", &number), t.fg("toolOutput", line))
        })
        .collect()
}
