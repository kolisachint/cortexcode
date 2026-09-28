//! A `SelectList` in the prompt's frame: the shape the small pickers share
//! (`class XSelectorComponent extends InputFrame` holding one `SelectList`).

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tui_theme::get_select_list_theme;
use cortexcode_code_tui_widgets::input_frame::{InputFrame, InputFrameOptions};
use cortexcode_tui_components::{SelectItem, SelectList, SelectListLayoutOptions};
use cortexcode_tui_render::{Component, ComponentHandle};

/// The pickers' primary column: at least 12 cells, at most 32.
pub(crate) fn layout(min: usize, max: usize) -> SelectListLayoutOptions {
    SelectListLayoutOptions {
        min_primary_column_width: Some(min),
        max_primary_column_width: Some(max),
        truncate_primary: None,
    }
}

/// An `InputFrame` titled `title` around one select list.
pub struct FramedSelectList {
    frame: InputFrame,
    list: Rc<RefCell<SelectList>>,
}

impl FramedSelectList {
    pub(crate) fn new(
        title: &str,
        items: Vec<SelectItem>,
        max_visible: usize,
        layout: SelectListLayoutOptions,
        selected: Option<usize>,
    ) -> Self {
        let mut list = SelectList::new(items, max_visible, get_select_list_theme(), layout);
        if let Some(index) = selected {
            list.set_selected_index(index);
        }
        let list = Rc::new(RefCell::new(list));
        let mut frame = InputFrame::new(InputFrameOptions {
            title: Some(title.to_string()),
            ..Default::default()
        });
        frame.add_child(list.clone() as ComponentHandle);
        Self { frame, list }
    }

    /// `getSelectList()`.
    pub fn select_list(&self) -> Rc<RefCell<SelectList>> {
        self.list.clone()
    }
}

impl Component for FramedSelectList {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.list.borrow_mut().handle_input(data);
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }
}
