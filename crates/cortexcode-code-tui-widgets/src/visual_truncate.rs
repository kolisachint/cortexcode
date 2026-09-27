//! `components/visual-truncate.ts`: keep the last N visual (wrapped) lines.

use cortexcode_tui_components::Text;
use cortexcode_tui_render::Component;

/// `VisualTruncateResult`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VisualTruncateResult {
    /// The visual lines to display.
    pub visual_lines: Vec<String>,
    /// Number of visual lines that were skipped (hidden).
    pub skipped_count: usize,
}

/// `truncateToVisualLines`: the last `max_visual_lines` lines `text` wraps to
/// at `width` (in a `Text` with `padding_x`).
pub fn truncate_to_visual_lines(
    text: &str,
    max_visual_lines: usize,
    width: u16,
    padding_x: usize,
) -> VisualTruncateResult {
    if text.is_empty() {
        return VisualTruncateResult::default();
    }
    let all = Text::new(text, padding_x, 0).render(width);
    if all.len() <= max_visual_lines {
        return VisualTruncateResult {
            visual_lines: all,
            skipped_count: 0,
        };
    }
    let skipped = all.len() - max_visual_lines;
    VisualTruncateResult {
        visual_lines: all[skipped..].to_vec(),
        skipped_count: skipped,
    }
}
