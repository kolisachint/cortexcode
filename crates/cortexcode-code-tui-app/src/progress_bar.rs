//! The one progress bar (`components/progress-bar.ts`): every surface that
//! makes the user wait on measurable work renders it through here.

use cortexcode_code_agent_session::format::js_to_fixed;
use cortexcode_code_tui_theme::theme;

/// Default cells in a bar.
pub const PROGRESS_BAR_CELLS: usize = 12;

fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// Megabytes, to one decimal (so a slow download does not look frozen).
pub fn format_megabytes(bytes: u64) -> String {
    format!("{} MB", js_to_fixed(bytes as f64 / (1024.0 * 1024.0), 1))
}

/// The filled/empty track; a started-but-tiny ratio keeps one filled cell.
fn track(ratio: f64, cells: usize) -> String {
    let clamped = ratio.clamp(0.0, 1.0);
    let min = if clamped > 0.0 { 1 } else { 0 };
    let filled = (js_round(clamped * cells as f64) as usize)
        .min(cells)
        .max(min);
    let t = theme();
    t.fg("accent", &"▰".repeat(filled)) + &t.fg("halftone", &"▱".repeat(cells - filled))
}

/// A determinate bar: track, percent, and a trailing detail. No leading or
/// trailing space; callers width-clamp.
pub fn render_progress_bar(ratio: f64, detail: &str, cells: Option<usize>) -> String {
    let cells = cells.unwrap_or(PROGRESS_BAR_CELLS);
    let clamped = ratio.clamp(0.0, 1.0);
    let percent = format!("{}%", js_round(clamped * 100.0) as i64);
    let t = theme();
    format!(
        "{} {} {}",
        track(clamped, cells),
        t.fg("muted", &percent),
        t.fg("dim", &format!("· {detail}"))
    )
}

/// A download's progress: a bar when the length is known, a running byte
/// count when it is not.
pub fn render_download_progress(
    received_bytes: u64,
    total_bytes: Option<u64>,
    cells: Option<usize>,
) -> String {
    match total_bytes.filter(|t| *t > 0) {
        None => theme().fg("dim", &format!("{}…", format_megabytes(received_bytes))),
        Some(total) => {
            let detail = format!(
                "{} / {}",
                format_megabytes(received_bytes),
                format_megabytes(total)
            );
            render_progress_bar(received_bytes as f64 / total as f64, &detail, cells)
        }
    }
}
