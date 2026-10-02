//! Port of the pin's `test/theme-cutout-tokens.test.ts`.
//!
//! N/A until 11.2: the three radar-row cases ("draws no marker stroke on the
//! newest radar row", "strokes the newest radar row and nothing else") need
//! `tool-signal.ts`'s `renderToolSignalLine`, which arrives with the chat
//! widgets.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

use cortexcode_code_tui_theme::*;
use cortexcode_tui_components::{BoxComponent, Text};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{strip_vt_control_characters, visible_width};

/// These tests switch the process-wide theme, so they run one at a time.
fn lock(theme_name: &str) -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_theme(Some(theme_name), false);
    guard
}

fn strip(s: &str) -> String {
    strip_vt_control_characters(s)
}

fn filled_box() -> BoxComponent {
    BoxComponent::new(
        1,
        1,
        Some(Box::new(|t: &str| theme().bg("userMessageBg", t))),
    )
}

fn text(s: &str) -> Rc<RefCell<Text>> {
    Rc::new(RefCell::new(Text::new(s, 0, 0)))
}

fn sheet_of(rows: &[String]) -> BoxComponent {
    let mut b = filled_box();
    apply_paper_sheet(&mut b);
    for row in rows {
        b.add_child(text(row));
    }
    b
}

fn widths(lines: &[String]) -> Vec<usize> {
    lines.iter().map(|l| visible_width(l)).collect()
}

mod a_theme_without_them {
    use super::*;

    #[test]
    fn draws_no_shadow_pass() {
        let _g = lock("dark");
        assert!(get_paper_shadow_fn().is_none());
    }

    #[test]
    fn adds_no_shadow_row_to_a_filled_box_and_no_gutter() {
        let _g = lock("dark");
        let mut b = sheet_of(&["hello".into()]);
        let lines = b.render(20);
        assert_eq!(lines.len(), 3);
        assert_eq!(widths(&lines), vec![20, 20, 20]);
    }

    #[test]
    fn leaves_headings_as_coloured_text() {
        let _g = lock("dark");
        let md = get_markdown_theme();
        assert_eq!(md.heading_block.as_ref().unwrap()("Title", 2), "Title");
        assert_eq!((md.heading)("Title", 2), theme().fg("mdHeading", "Title"));
    }

    #[test]
    fn keeps_the_bracketed_message_label() {
        let _g = lock("dark");
        assert_eq!(
            message_label("skill"),
            theme().fg("customMessageLabel", "\x1b[1m[skill]\x1b[22m")
        );
    }

    #[test]
    fn paints_a_gauge_track_in_dim() {
        let _g = lock("dark");
        assert_eq!(theme().fg("halftone", "▱"), theme().fg("dim", "▱"));
    }

    #[test]
    fn has_no_marker_stroke_token() {
        // The radar half of this case waits on tool-signal (11.2).
        let _g = lock("dark");
        assert!(!theme().has_bg("activeToolBg"));
    }
}

mod cutout_themes {
    use super::*;

    const THEMES: [&str; 2] = ["vox-cutout-light", "vox-cutout-dark"];

    #[test]
    fn cuts_the_block_into_a_sheet_with_a_shadow_on_both_edges() {
        for name in THEMES {
            let _g = lock(name);
            let lines = sheet_of(&["hello".into()]).render(40);
            assert_eq!(lines.len(), 4, "{name}");
            let band = 40 - PAPER_INSET;
            assert_eq!(visible_width(&lines[0]), band);
            for line in &lines[1..3] {
                assert_eq!(visible_width(line), band + 1);
            }
            assert!(!lines[0].contains('▏'));
            assert!(format!("{}{}", lines[1], lines[2]).contains('▏'));
            assert!(lines[3].starts_with(' '));
            assert_eq!(lines[3].matches('▔').count(), band - 1);
            assert_eq!(visible_width(&lines[3]), band);
        }
    }

    #[test]
    fn closes_the_bottom_corner_flush_with_the_shadows_column() {
        for name in THEMES {
            let _g = lock(name);
            let lines = sheet_of(&["hello".into()]).render(40);
            let run = strip(&lines[3]);
            assert!(!run.contains('▏'));
            let column = strip(&lines[2]);
            let index = column.chars().position(|c| c == '▏').unwrap();
            assert_eq!(visible_width(&run), index, "{name}");
        }
    }

    #[test]
    fn rules_the_sheets_right_edge_instead_of_nicking_it() {
        for name in THEMES {
            let _g = lock(name);
            let rows: Vec<String> = (0..24).map(|i| format!("row {i}")).collect();
            let rendered = sheet_of(&rows).render(40);
            let band = 40 - PAPER_INSET;
            assert!(!rendered.iter().any(|l| strip(l).contains('█')));
            assert_eq!(visible_width(&rendered[0]), band);
            assert!(!rendered[0].contains('▏'));
            for line in &rendered[1..rendered.len() - 1] {
                assert_eq!(visible_width(line), band + 1);
                assert_eq!(
                    strip(line).chars().position(|c| c == '▏'),
                    Some(band),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn holds_every_row_of_the_sheet_to_one_width() {
        for name in THEMES {
            let _g = lock(name);
            let rows: Vec<String> = (0..12).map(|i| format!("row {i}")).collect();
            let lines = sheet_of(&rows).render(40);
            let edge = 40 - PAPER_INSET + 1;
            for line in &lines[1..lines.len() - 1] {
                assert_eq!(visible_width(line), edge);
            }
            assert_eq!(visible_width(&lines[lines.len() - 1]), edge - 1);
            assert!(lines[1..lines.len() - 1].iter().all(|l| l.contains('▏')));
        }
    }

    #[test]
    fn keeps_the_gutter_off_the_content() {
        for name in THEMES {
            let _g = lock(name);
            let text = "x".repeat(30);
            let rendered = sheet_of(std::slice::from_ref(&text)).render(40).join("\n");
            assert!(strip(&rendered).contains(&text), "{name}");
        }
    }

    #[test]
    fn renders_headings_as_a_filled_chip() {
        for name in THEMES {
            let _g = lock(name);
            let md = get_markdown_theme();
            assert!(md.heading_block.is_some());
            assert_eq!(
                (md.heading)("Title", 2),
                theme().fg("headlineText", "Title")
            );
            assert!(!(md.heading)("Title", 2).contains(" T"));
            assert_eq!(
                md.heading_block.as_ref().unwrap()("Title", 2),
                theme().bg("headlineBg", " Title ")
            );
        }
    }

    #[test]
    fn leaves_headings_below_h2_as_coloured_text() {
        for name in THEMES {
            let _g = lock(name);
            let md = get_markdown_theme();
            assert_eq!((md.heading)("Deeper", 3), theme().fg("mdHeading", "Deeper"));
            assert_eq!(md.heading_block.as_ref().unwrap()("Deeper", 3), "Deeper");
        }
    }

    #[test]
    fn renders_the_message_label_as_a_tape_strip() {
        for name in THEMES {
            let _g = lock(name);
            let label = message_label("skill");
            let t = theme();
            assert_eq!(
                label,
                t.bg("tapeBg", &t.fg("tapeText", "\x1b[1m skill \x1b[22m"))
            );
            assert!(!label.contains("[skill]"));
        }
    }

    #[test]
    fn paints_a_gauge_track_apart_from_dim() {
        for name in THEMES {
            let _g = lock(name);
            assert_ne!(theme().fg("halftone", "▱"), theme().fg("dim", "▱"));
        }
    }
}

#[test]
fn a_theme_that_sets_only_half_a_chip_pair_falls_back_rather_than_rendering_a_half_styled_chip() {
    let _g = lock("dark");
    let t = theme();
    assert!(!t.has_bg("headlineBg"));
    assert!(!t.has("headlineText"));
    assert!(!t.has_bg("tapeBg"));
    assert!(!t.has("tapeText"));
}

#[test]
fn a_chip_painted_inside_a_filled_block_does_not_punch_a_hole_in_the_block_behind_it() {
    let _g = lock("vox-cutout-light");
    let sentinel = "\u{0}";
    let block_opener = theme()
        .bg("customMessageBg", sentinel)
        .split(sentinel)
        .next()
        .unwrap()
        .to_string();
    assert!(!block_opener.is_empty());

    let mut b = BoxComponent::new(
        1,
        1,
        Some(Box::new(|t: &str| theme().bg("customMessageBg", t))),
    );
    b.add_child(text(&message_label("extension")));
    let lines = b.render(40);
    let line = lines
        .iter()
        .find(|l| l.contains("extension"))
        .expect("label row");

    let chip_close = line.find("\x1b[49m").expect("chip close");
    let tail = &line[chip_close + "\x1b[49m".len()..];
    assert!(tail.starts_with(&block_opener), "{tail:?}");
    assert!(tail.ends_with("\x1b[49m"));
}

mod switching_theme_under_blocks_already_on_screen {
    use super::*;

    fn sheet() -> BoxComponent {
        sheet_of(&["hello".into()])
    }

    #[test]
    fn drops_the_paper_treatment_when_leaving_a_cut_out_theme() {
        let _g = lock("vox-cutout-dark");
        let mut b = sheet();
        assert_eq!(b.render(40).len(), 4);
        set_theme("dark", false).unwrap();
        let lines = b.render(40);
        assert_eq!(lines.len(), 3);
        assert_eq!(widths(&lines), vec![40, 40, 40]);
    }

    #[test]
    fn picks_the_paper_treatment_up_when_entering_one() {
        let _g = lock("dark");
        let mut b = sheet();
        assert_eq!(b.render(40).len(), 3);
        set_theme("vox-cutout-dark", false).unwrap();
        let lines = b.render(40);
        assert_eq!(lines.len(), 4);
        assert_eq!(visible_width(&lines[3]), 40 - PAPER_INSET);
    }

    #[test]
    fn keeps_a_markdown_theme_rendering_across_the_switch_both_ways() {
        let _g = lock("vox-cutout-dark");
        let from_cutout = get_markdown_theme();
        set_theme("dark", false).unwrap();
        assert_eq!(
            (from_cutout.heading)("Title", 2),
            theme().fg("mdHeading", "Title")
        );
        assert_eq!(
            from_cutout.heading_block.as_ref().unwrap()("Title", 2),
            "Title"
        );

        let from_plain = get_markdown_theme();
        set_theme("vox-cutout-dark", false).unwrap();
        assert_eq!(
            (from_plain.heading)("Title", 2),
            theme().fg("headlineText", "Title")
        );
        assert_eq!(
            from_plain.heading_block.as_ref().unwrap()("Title", 2),
            theme().bg("headlineBg", " Title ")
        );
    }
}
