//! The session chip (`components/session-chip.ts`): the session's name,
//! filled with its colour, in the top-right of the input box. The footer drops
//! its own copy of the name when the chip shows; both use
//! [`session_chip_fits`].

use cortexcode_code_tui_theme::{session_color_token, theme};
use cortexcode_tui_components::EditorTopBorderLabel;
use cortexcode_tui_util::{truncate_to_width, visible_width};

/// Longest name the chip shows before truncating.
const MAX_CHIP_NAME_WIDTH: usize = 20;

/// Terminal width at which the chip is drawn at all.
pub const SESSION_CHIP_MIN_WIDTH: usize = 48;

/// Whether a chip is drawn at this terminal width.
pub fn session_chip_fits(width: usize) -> bool {
    width >= SESSION_CHIP_MIN_WIDTH
}

/// `renderSessionChip`: the chip for a session, or `None` for an empty name.
/// A token the theme cannot fill falls back to an outline chip.
pub fn render_session_chip(name: &str, color_slot: i64) -> Option<EditorTopBorderLabel> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let shown = if visible_width(trimmed) > MAX_CHIP_NAME_WIDTH {
        truncate_to_width(trimmed, MAX_CHIP_NAME_WIDTH, "…", false)
    } else {
        trimmed.to_string()
    };
    let token = session_color_token(color_slot);
    let t = theme();
    if t.can_fill(token) {
        let plain = format!(" {shown} ");
        // Raw SGR bold survives inside the fill's own colour run.
        let styled = t.fill(token, &format!("\x1b[1m{plain}\x1b[22m"));
        return Some(EditorTopBorderLabel { plain, styled });
    }
    let plain = format!("┤{shown}├");
    let styled = t.fg("border", "┤") + &t.fg(token, &shown) + &t.fg("border", "├");
    Some(EditorTopBorderLabel { plain, styled })
}
