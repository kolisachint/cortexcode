//! `components/user-message.ts`.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tui_theme::{apply_block_fill, get_markdown_theme, theme, BlockFill};
use cortexcode_tui_components::{BoxComponent, DefaultTextStyle, Markdown, MarkdownTheme};
use cortexcode_tui_render::{Component, Container};

use crate::wrap_zone;

/// A user message: its markdown on the user-message fill, in an OSC 133 zone.
pub struct UserMessageComponent {
    container: Container,
}

impl UserMessageComponent {
    pub fn new(text: &str) -> Self {
        Self::with_theme(text, get_markdown_theme())
    }

    pub fn with_theme(text: &str, markdown_theme: MarkdownTheme) -> Self {
        let mut content = BoxComponent::new(1, 1, None);
        apply_block_fill(&mut content, BlockFill::UserMessageBg);
        content.add_child(Rc::new(RefCell::new(Markdown::new(
            text,
            0,
            0,
            markdown_theme,
            Some(DefaultTextStyle {
                color: Some(Box::new(|s: &str| theme().fg("userMessageText", s))),
                ..Default::default()
            }),
        ))));
        let mut container = Container::new();
        container.add_child(Rc::new(RefCell::new(content)));
        Self { container }
    }
}

impl Component for UserMessageComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        wrap_zone(self.container.render(width))
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}
