//! The extension selector dialog (no pin test file covers it; these check
//! the behaviour of `components/extension-selector.ts` and
//! `countdown-timer.ts` directly).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::support::lock;
use cortexcode_code_tui_app::extension_selector::*;
use cortexcode_code_tui_app::input_frame::set_input_frame_border;
use cortexcode_tui_components::FrameBorderStyle;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::strip_vt_control_characters;

fn selector(
    options: &[&str],
) -> (
    ExtensionSelectorComponent,
    Rc<RefCell<Vec<SelectorOutcome>>>,
) {
    set_input_frame_border(FrameBorderStyle::Box);
    let outcomes = Rc::new(RefCell::new(Vec::new()));
    let sink = outcomes.clone();
    let s = ExtensionSelectorComponent::new(
        "Allow: $ ls",
        options.iter().map(|o| o.to_string()).collect(),
        None,
        Box::new(move |o| sink.borrow_mut().push(o)),
    );
    (s, outcomes)
}

fn plain(lines: Vec<String>) -> Vec<String> {
    lines
        .iter()
        .map(|l| strip_vt_control_characters(l).trim_end().to_string())
        .collect()
}

const DOWN: &str = "\x1b[B";
const UP: &str = "\x1b[A";
const ENTER: &str = "\r";
const ESC: &str = "\x1b";

#[test]
fn draws_the_title_in_the_border_the_options_and_the_hints() {
    let _g = lock(Some("dark"));
    let (mut s, _) = selector(&["Yes (once)", "No (block)", "Always"]);
    let lines = plain(s.render(60));
    assert!(lines[0].contains("Allow: $ ls"), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("› Yes (once)")));
    assert!(lines.iter().any(|l| l.contains("  No (block)")));
    assert!(lines
        .iter()
        .any(|l| l.contains("navigate") && l.contains("select") && l.contains("cancel")));
}

#[test]
fn moves_with_the_select_keys_and_j_k_and_stops_at_the_ends() {
    let _g = lock(Some("dark"));
    let (mut s, outcomes) = selector(&["a", "b", "c"]);
    s.handle_input(UP);
    s.handle_input(DOWN);
    s.handle_input("j");
    s.handle_input("j");
    s.handle_input("k");
    s.handle_input(ENTER);
    assert_eq!(
        *outcomes.borrow(),
        vec![SelectorOutcome::Selected("b".into())]
    );
}

#[test]
fn confirms_on_a_bare_newline_too_and_cancels_on_escape() {
    let _g = lock(Some("dark"));
    let (mut s, outcomes) = selector(&["a", "b"]);
    s.handle_input("\n");
    s.handle_input(ESC);
    assert_eq!(
        *outcomes.borrow(),
        vec![
            SelectorOutcome::Selected("a".into()),
            SelectorOutcome::Cancelled
        ]
    );
}

#[test]
fn fills_the_selected_row_to_the_full_width() {
    let _g = lock(Some("dark"));
    let mut list = SelectedRowList::new(
        vec![
            SelectableRow {
                text: "one".into(),
                selected: false,
            },
            SelectableRow {
                text: "two".into(),
                selected: true,
            },
        ],
        1,
    );
    let lines = list.render(20);
    assert_eq!(strip_vt_control_characters(&lines[0]), " one");
    assert_eq!(cortexcode_tui_util::visible_width(&lines[1]), 20);
}

#[test]
fn the_countdown_ticks_once_a_second_and_expires_at_zero() {
    let mut c = CountdownTimer::new(Duration::from_millis(2500));
    assert_eq!(c.remaining_seconds(), 3);
    let start = Instant::now();
    assert_eq!(c.poll(start), None);
    assert_eq!(c.poll(start + Duration::from_millis(1100)), Some(2));
    assert_eq!(c.poll(start + Duration::from_millis(2100)), Some(1));
    assert_eq!(c.poll(start + Duration::from_millis(3100)), Some(0));
    assert_eq!(c.deadline(), None);
    assert_eq!(c.poll(start + Duration::from_secs(10)), None);
}

#[test]
fn a_timed_selector_shows_the_seconds_left_and_cancels_when_they_run_out() {
    let _g = lock(Some("dark"));
    set_input_frame_border(FrameBorderStyle::Box);
    let outcomes = Rc::new(RefCell::new(Vec::new()));
    let sink = outcomes.clone();
    let mut s = ExtensionSelectorComponent::new(
        "Pick",
        vec!["a".into()],
        Some(Duration::from_millis(1)),
        Box::new(move |o| sink.borrow_mut().push(o)),
    );
    assert!(plain(s.render(40))[0].contains("Pick (1s)"));
    std::thread::sleep(Duration::from_millis(1050));
    assert!(s.poll());
    assert_eq!(*outcomes.borrow(), vec![SelectorOutcome::Cancelled]);
}

#[test]
fn draws_a_dynamic_border_across_the_width() {
    let _g = lock(Some("dark"));
    let lines = DynamicBorder::new(None).render(7);
    assert_eq!(strip_vt_control_characters(&lines[0]), "───────");
}

/// `picker-widths.test.ts`: "extension picker fits within N columns".
#[test]
fn extension_picker_fits_the_width_and_fills_its_selected_row() {
    let _g = lock(Some("dark"));
    const LONG: &str = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore";
    let bg = cortexcode_code_tui_theme::theme()
        .get_bg_ansi("selectedBg")
        .to_string();
    for width in [40u16, 80, 160] {
        let mut s = ExtensionSelectorComponent::new(
            "Pick one",
            vec![LONG.to_string(), "short".to_string()],
            None,
            Box::new(|_| {}),
        );
        let lines = s.render(width);
        for line in &lines {
            let w = cortexcode_tui_util::visible_width(line);
            assert!(w <= width as usize, "{w} > {width}: {line}");
            if line.contains(&bg) {
                assert_eq!(w, width as usize, "{line}");
            }
        }
    }
}
