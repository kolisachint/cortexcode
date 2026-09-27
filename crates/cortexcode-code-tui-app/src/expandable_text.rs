//! `ExpandableText` (`resource-display.ts`): text with a collapsed and an
//! expanded form, both computed on demand.

use cortexcode_tui_components::Text;
use cortexcode_tui_render::Component;

type TextFn = Box<dyn Fn() -> String>;

/// Something the expand key can open and close.
pub trait Expandable {
    fn set_expanded(&mut self, expanded: bool);
}

pub struct ExpandableText {
    text: Text,
    collapsed: TextFn,
    expanded: TextFn,
    is_expanded: bool,
}

impl ExpandableText {
    pub fn new(
        collapsed: impl Fn() -> String + 'static,
        expanded: impl Fn() -> String + 'static,
        initially_expanded: bool,
        padding_x: usize,
        padding_y: usize,
    ) -> Self {
        let initial = if initially_expanded {
            expanded()
        } else {
            collapsed()
        };
        Self {
            text: Text::new(initial, padding_x, padding_y),
            collapsed: Box::new(collapsed),
            expanded: Box::new(expanded),
            is_expanded: initially_expanded,
        }
    }

    pub fn is_expanded(&self) -> bool {
        self.is_expanded
    }

    /// Re-evaluate the current state's text (what it reads may have changed,
    /// e.g. the working directory after `/cd`).
    pub fn refresh(&mut self) {
        let text = if self.is_expanded {
            (self.expanded)()
        } else {
            (self.collapsed)()
        };
        self.text.set_text(text);
    }
}

impl Expandable for ExpandableText {
    fn set_expanded(&mut self, expanded: bool) {
        self.is_expanded = expanded;
        self.refresh();
    }
}

impl Component for ExpandableText {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.text.render(width)
    }

    fn invalidate(&mut self) {
        self.text.invalidate();
    }
}
