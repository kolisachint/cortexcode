//! `components/selected-row-list.ts`: picker rows rendered at the full
//! width, so the selected one's band fills edge to edge.

use cortexcode_code_tui_theme::paint_selected_row;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::truncate_to_width;

/// `SelectableRow`: already-styled content, without the left margin.
#[derive(Debug, Clone, Default)]
pub struct SelectableRow {
    pub text: String,
    pub selected: bool,
}

/// `SelectedRowList`: picker rows rendered at terminal width, so the
/// selected one fills edge to edge.
pub struct SelectedRowList {
    rows: Vec<SelectableRow>,
    margin_x: usize,
}

impl SelectedRowList {
    pub fn new(rows: Vec<SelectableRow>, margin_x: usize) -> Self {
        Self { rows, margin_x }
    }

    pub fn set_rows(&mut self, rows: Vec<SelectableRow>) {
        self.rows = rows;
    }
}

impl Component for SelectedRowList {
    fn render(&mut self, width: u16) -> Vec<String> {
        let margin = " ".repeat(self.margin_x);
        self.rows
            .iter()
            .map(|row| {
                let line = truncate_to_width(
                    &format!("{margin}{}", row.text),
                    width as usize,
                    "...",
                    false,
                );
                if row.selected {
                    paint_selected_row(&line, width as usize)
                } else {
                    line
                }
            })
            .collect()
    }
}
