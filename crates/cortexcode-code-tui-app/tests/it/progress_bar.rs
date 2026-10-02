//! Port of the pin's `test/progress-bar.test.ts` (the voice-panel cases wait
//! on the voice UI, which is not ported).

use crate::support::{lock, plain};
use cortexcode_code_tui_app::progress_bar::*;

#[test]
fn distinguishes_filled_from_empty_by_shape_not_only_by_colour() {
    let _g = lock(Some("dark"));
    assert_eq!(
        plain(&render_progress_bar(0.25, "3/12 sessions", None)),
        "▰▰▰▱▱▱▱▱▱▱▱▱ 25% · 3/12 sessions"
    );
}

#[test]
fn keeps_one_filled_cell_once_work_has_started() {
    let _g = lock(Some("dark"));
    assert!(plain(&render_progress_bar(1.0 / 40.0, "1/40 files", None)).contains("▰▱▱▱▱▱▱▱▱▱▱▱"));
}

#[test]
fn shows_an_untouched_bar_as_fully_empty() {
    let _g = lock(Some("dark"));
    assert!(plain(&render_progress_bar(0.0, "0/12 files", None)).contains("▱▱▱▱▱▱▱▱▱▱▱▱"));
}

#[test]
fn clamps_out_of_range_ratios_rather_than_overflowing_the_track() {
    let _g = lock(Some("dark"));
    assert!(plain(&render_progress_bar(1.5, "x", None))
        .contains(&format!("{} 100%", "▰".repeat(PROGRESS_BAR_CELLS))));
    assert!(plain(&render_progress_bar(-1.0, "x", None))
        .contains(&format!("{} 0%", "▱".repeat(PROGRESS_BAR_CELLS))));
}

#[test]
fn lets_a_surface_choose_its_own_width_and_nothing_else() {
    let _g = lock(Some("dark"));
    assert_eq!(
        plain(&render_progress_bar(0.5, "x", Some(20))),
        format!("{}{} 50% · x", "▰".repeat(10), "▱".repeat(10))
    );
}

#[test]
fn format_megabytes_keeps_one_decimal() {
    assert_eq!(format_megabytes(450 * 1024 * 1024), "450.0 MB");
    assert_eq!(format_megabytes(1536 * 1024), "1.5 MB");
}

#[test]
fn download_drops_the_bar_for_a_running_count_when_the_length_is_unknown() {
    let _g = lock(Some("dark"));
    let text = plain(&render_download_progress(3 * 1024 * 1024, None, None));
    assert_eq!(text, "3.0 MB…");
    assert!(!text.contains('▰'));
    assert_eq!(
        plain(&render_download_progress(1024 * 1024, Some(0), None)),
        "1.0 MB…"
    );
}
