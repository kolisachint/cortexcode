//! `showRecord`'s transcript row: a spacer and a text that stay in the
//! transcript (a path written to, a URL). Back-to-back records coalesce: the
//! second replaces the first's text when nothing was added in between.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_tui_components::{Spacer, Text};
use cortexcode_tui_render::{ComponentHandle, Container};

/// The last record's spacer and text (`lastStatusSpacer`, `lastStatusText`).
#[derive(Default)]
pub struct RecordRows {
    last: Option<(ComponentHandle, Rc<RefCell<Text>>)>,
}

impl RecordRows {
    /// Forget the last record (the transcript was rebuilt).
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// Show `styled` in `chat`, reusing the previous record's row when it is
    /// still the last thing there.
    pub fn show(&mut self, chat: &mut Container, styled: String) {
        if let Some((spacer, text)) = &self.last {
            let n = chat.children.len();
            let text_handle: ComponentHandle = text.clone();
            if n >= 2
                && Rc::ptr_eq(&chat.children[n - 1], &text_handle)
                && Rc::ptr_eq(&chat.children[n - 2], spacer)
            {
                text.borrow_mut().set_text(styled);
                return;
            }
        }
        let spacer: ComponentHandle = Rc::new(RefCell::new(Spacer::new(1)));
        let text = Rc::new(RefCell::new(Text::new(styled, 1, 0)));
        chat.add_child(spacer.clone());
        chat.add_child(text.clone());
        self.last = Some((spacer, text));
    }
}

#[cfg(test)]
mod tests {
    //! Port of the `showRecord` cases of the pin's
    //! `test/interactive-mode-status.test.ts`.

    use super::*;

    fn last_line(chat: &Container) -> String {
        chat.children
            .last()
            .map(|c| c.borrow_mut().render(120).join("\n"))
            .unwrap_or_default()
    }

    #[test]
    fn coalesces_immediately_sequential_status_messages() {
        let mut chat = Container::new();
        let mut rows = RecordRows::default();
        rows.show(&mut chat, "STATUS_ONE".into());
        assert_eq!(chat.children.len(), 2);
        assert!(last_line(&chat).contains("STATUS_ONE"));
        rows.show(&mut chat, "STATUS_TWO".into());
        // The second updates the previous line instead of appending.
        assert_eq!(chat.children.len(), 2);
        assert!(last_line(&chat).contains("STATUS_TWO"));
        assert!(!last_line(&chat).contains("STATUS_ONE"));
    }

    #[test]
    fn appends_a_new_status_line_if_something_else_was_added_in_between() {
        let mut chat = Container::new();
        let mut rows = RecordRows::default();
        rows.show(&mut chat, "STATUS_ONE".into());
        assert_eq!(chat.children.len(), 2);
        chat.add_child(Rc::new(RefCell::new(Text::new("OTHER", 0, 0))));
        assert_eq!(chat.children.len(), 3);
        rows.show(&mut chat, "STATUS_TWO".into());
        // A spacer and the text.
        assert_eq!(chat.children.len(), 5);
        assert!(last_line(&chat).contains("STATUS_TWO"));
    }
}
