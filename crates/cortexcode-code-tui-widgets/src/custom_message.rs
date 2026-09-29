//! `components/custom-message.ts` and `components/branch-summary-message.ts`:
//! the message kinds drawn on the custom-message fill.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_agent_types::{BranchSummaryMessage, CustomMessage};
use cortexcode_ai_types::{Content, UserContent};
use cortexcode_code_tui_keybindings::key_text;
use cortexcode_code_tui_theme::{apply_block_fill, message_label, theme, BlockFill};
use cortexcode_tui_components::{
    BoxComponent, DefaultTextStyle, Markdown, MarkdownTheme, Spacer, Text,
};
use cortexcode_tui_render::{Component, Container};

fn custom_text_markdown(text: &str, markdown_theme: MarkdownTheme) -> Markdown {
    Markdown::new(
        text,
        0,
        0,
        markdown_theme,
        Some(DefaultTextStyle {
            color: Some(Box::new(|s: &str| theme().fg("customMessageText", s))),
            ..Default::default()
        }),
    )
}

/// `CustomMessageComponent`: a displayed custom message, as its label over
/// its markdown. (Extension message renderers arrive with 12.3.)
pub struct CustomMessageComponent {
    message: CustomMessage,
    markdown_theme: Rc<dyn Fn() -> MarkdownTheme>,
    container: Container,
}

impl CustomMessageComponent {
    pub fn new(message: CustomMessage, markdown_theme: Rc<dyn Fn() -> MarkdownTheme>) -> Self {
        let mut this = Self {
            message,
            markdown_theme,
            container: Container::new(),
        };
        this.rebuild();
        this
    }

    /// Nothing folds in the default rendering.
    pub fn set_expanded(&mut self, _expanded: bool) {}

    fn rebuild(&mut self) {
        let mut sheet = BoxComponent::new(1, 1, None);
        apply_block_fill(&mut sheet, BlockFill::CustomMessageBg);
        sheet.add_child(Rc::new(RefCell::new(Text::new(
            message_label(&self.message.custom_type),
            0,
            0,
        ))));
        sheet.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        let text = match &self.message.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|c| match c {
                    Content::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };
        sheet.add_child(Rc::new(RefCell::new(custom_text_markdown(
            &text,
            (self.markdown_theme)(),
        ))));
        self.container.clear();
        self.container
            .add_child(Rc::new(RefCell::new(Spacer::new(1))));
        self.container.add_child(Rc::new(RefCell::new(sheet)));
    }
}

impl Component for CustomMessageComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.rebuild();
    }
}

/// `BranchSummaryMessageComponent`: the summary of an abandoned branch,
/// folded to one line until expanded.
pub struct BranchSummaryMessageComponent {
    message: BranchSummaryMessage,
    markdown_theme: Rc<dyn Fn() -> MarkdownTheme>,
    expanded: bool,
    sheet: BoxComponent,
}

impl BranchSummaryMessageComponent {
    pub fn new(
        message: BranchSummaryMessage,
        markdown_theme: Rc<dyn Fn() -> MarkdownTheme>,
    ) -> Self {
        let mut this = Self {
            message,
            markdown_theme,
            expanded: false,
            sheet: BoxComponent::new(1, 1, None),
        };
        this.update_display();
        this
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.update_display();
    }

    fn update_display(&mut self) {
        let t = theme();
        let mut sheet = BoxComponent::new(1, 1, None);
        apply_block_fill(&mut sheet, BlockFill::CustomMessageBg);
        sheet.add_child(Rc::new(RefCell::new(Text::new(
            message_label("branch"),
            0,
            0,
        ))));
        sheet.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        if self.expanded {
            let text = format!("**Branch Summary**\n\n{}", self.message.summary);
            sheet.add_child(Rc::new(RefCell::new(custom_text_markdown(
                &text,
                (self.markdown_theme)(),
            ))));
        } else {
            sheet.add_child(Rc::new(RefCell::new(Text::new(
                format!(
                    "{}{}{}",
                    t.fg("customMessageText", "Branch summary ("),
                    t.fg("dim", &key_text("app.tools.expand")),
                    t.fg("customMessageText", " to expand)")
                ),
                0,
                0,
            ))));
        }
        self.sheet = sheet;
    }
}

impl Component for BranchSummaryMessageComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.sheet.render(width)
    }

    fn invalidate(&mut self) {
        self.update_display();
    }
}
