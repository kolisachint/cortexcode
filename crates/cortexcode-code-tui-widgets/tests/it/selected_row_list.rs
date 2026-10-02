//! Port of the pin's `test/selected-row-list.test.ts`.

use crate::support::lock;
use cortexcode_code_tui_theme::theme;
use cortexcode_code_tui_widgets::selected_row_list::{SelectableRow, SelectedRowList};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;

fn rows() -> Vec<SelectableRow> {
    vec![
        SelectableRow {
            text: "first".into(),
            selected: false,
        },
        SelectableRow {
            text: "second".into(),
            selected: true,
        },
        SelectableRow {
            text: "third".into(),
            selected: false,
        },
    ]
}

#[test]
fn fills_the_selected_row_to_the_full_width_and_leaves_the_rest_alone() {
    let _g = lock();
    let rendered = SelectedRowList::new(rows(), 0).render(80);
    let bg_open = theme().get_bg_ansi("selectedBg").to_string();
    assert_eq!(rendered.iter().filter(|l| l.contains(&bg_open)).count(), 1);
    assert!(rendered[1].contains("second"));
    assert_eq!(visible_width(&rendered[1]), 80);
    // Unselected rows keep their natural width: no band, no padding.
    assert_eq!(visible_width(&rendered[0]), visible_width("first"));
}

#[test]
fn puts_the_left_margin_inside_the_band_so_it_starts_at_column_0() {
    let _g = lock();
    let rendered = SelectedRowList::new(rows(), 2).render(80);
    assert_eq!(rendered[1].find(theme().get_bg_ansi("selectedBg")), Some(0));
    assert_eq!(visible_width(&rendered[1]), 80);
    assert_eq!(visible_width(&rendered[0]), visible_width("  first"));
}

#[test]
fn keeps_a_row_that_overruns_the_width_down_to_the_width() {
    let _g = lock();
    let rendered = SelectedRowList::new(
        vec![SelectableRow {
            text: "x".repeat(60),
            selected: true,
        }],
        0,
    )
    .render(20);
    assert_eq!(visible_width(&rendered[0]), 20);
}
