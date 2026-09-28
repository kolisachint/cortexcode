//! Port of the pin's `test/assistant-message.test.ts`.

mod support;

use cortexcode_ai_types::{Content, ThinkingContent, ToolCallContent};
use cortexcode_code_tui_widgets::{
    AssistantMessageComponent, ThinkingDisplay, OSC133_ZONE_END, OSC133_ZONE_FINAL,
    OSC133_ZONE_START,
};
use cortexcode_tui_render::Component;
use support::{assistant, lock};

fn thinking(text: &str) -> Content {
    Content::Thinking(ThinkingContent {
        thinking: text.into(),
        signature: Some(String::new()),
        ..Default::default()
    })
}

fn tool_call() -> Content {
    Content::ToolCall(ToolCallContent {
        id: "tool-1".into(),
        name: "read".into(),
        arguments: serde_json::json!({ "path": "file.txt" }),
        thought_signature: None,
    })
}

#[test]
fn adds_osc_133_zone_markers_to_assistant_messages_without_tool_calls() {
    let _g = lock();
    let mut c = AssistantMessageComponent::new(
        Some(&assistant(vec![Content::text("hello")])),
        ThinkingDisplay::Full,
    );
    let lines = c.render(40);
    assert!(!lines.is_empty());
    assert!(lines[0].contains(OSC133_ZONE_START));
    assert!(lines
        .last()
        .unwrap()
        .starts_with(&format!("{OSC133_ZONE_END}{OSC133_ZONE_FINAL}")));
}

#[test]
fn does_not_add_osc_133_zone_markers_when_assistant_message_contains_tool_calls() {
    let _g = lock();
    let mut c = AssistantMessageComponent::new(
        Some(&assistant(vec![Content::text("calling tool"), tool_call()])),
        ThinkingDisplay::Full,
    );
    let rendered = c.render(60).join("\n");
    assert!(!rendered.contains(OSC133_ZONE_START));
    assert!(!rendered.contains(OSC133_ZONE_END));
    assert!(!rendered.contains(OSC133_ZONE_FINAL));
}

#[test]
fn omit_renders_nothing_for_a_message_that_only_thought_and_called_a_tool() {
    let _g = lock();
    let mut c = AssistantMessageComponent::new(
        Some(&assistant(vec![
            thinking("weighing the options"),
            tool_call(),
        ])),
        ThinkingDisplay::Omit,
    );
    assert_eq!(c.render(60), Vec::<String>::new());
}

#[test]
fn omit_drops_the_thinking_block_but_keeps_the_text_around_it() {
    let _g = lock();
    let mut c = AssistantMessageComponent::new(
        Some(&assistant(vec![
            thinking("weighing the options"),
            Content::text("here is the answer"),
        ])),
        ThinkingDisplay::Omit,
    );
    let rendered = c.render(60).join("\n");
    assert!(rendered.contains("here is the answer"));
    assert!(!rendered.contains("weighing the options"));
    assert!(!rendered.contains("Thinking..."));
}

#[test]
fn label_stands_in_for_the_trace() {
    let _g = lock();
    let mut c = AssistantMessageComponent::new(
        Some(&assistant(vec![thinking("weighing the options")])),
        ThinkingDisplay::Label,
    );
    let rendered = c.render(60).join("\n");
    assert!(rendered.contains("Thinking..."));
    assert!(!rendered.contains("weighing the options"));
}
