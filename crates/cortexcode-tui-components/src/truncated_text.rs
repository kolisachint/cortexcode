//! Single-line, truncate-to-fit text component, ported from
//! `components/truncated-text.ts`.

use cortexcode_tui_render::Component;
use cortexcode_tui_util::{truncate_to_width, visible_width};

pub struct TruncatedText {
    text: String,
    padding_x: usize,
    padding_y: usize,
}

impl TruncatedText {
    pub fn new(text: impl Into<String>, padding_x: usize, padding_y: usize) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }
}

impl Component for TruncatedText {
    fn render(&mut self, width: u16) -> Vec<String> {
        let width = width as usize;
        let mut result = Vec::new();
        let empty_line = " ".repeat(width);

        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        let available_width = (width.saturating_sub(self.padding_x * 2)).max(1);

        let single_line_text = match self.text.find('\n') {
            Some(idx) => &self.text[..idx],
            None => self.text.as_str(),
        };

        let display_text = truncate_to_width(single_line_text, available_width, "...", false);

        let left_padding = " ".repeat(self.padding_x);
        let right_padding = " ".repeat(self.padding_x);
        let line_with_padding = format!("{left_padding}{display_text}{right_padding}");

        let line_visible_width = visible_width(&line_with_padding);
        let padding_needed = width.saturating_sub(line_visible_width);
        let final_line = format!("{line_with_padding}{}", " ".repeat(padding_needed));

        result.push(final_line);

        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_short_text_to_width() {
        let mut t = TruncatedText::new("hi", 0, 0);
        let lines = t.render(10);
        assert_eq!(lines.len(), 1);
        assert_eq!(visible_width(&lines[0]), 10);
        assert!(lines[0].starts_with("hi"));
    }

    #[test]
    fn truncates_long_text() {
        let mut t = TruncatedText::new("a very long line of text here", 0, 0);
        let lines = t.render(10);
        assert_eq!(visible_width(&lines[0]), 10);
    }

    #[test]
    fn stops_at_first_newline() {
        let mut t = TruncatedText::new("first\nsecond", 0, 0);
        let lines = t.render(20);
        assert!(lines[0].starts_with("first"));
        assert!(!lines[0].contains("second"));
    }

    #[test]
    fn vertical_padding_adds_empty_lines() {
        let mut t = TruncatedText::new("hi", 0, 1);
        let lines = t.render(5);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "     ");
        assert_eq!(lines[2], "     ");
    }
}
