//! GitHub Copilot per-request headers.
//!
//! Port of hoocode `providers/github-copilot-headers.ts` (v0.5.89).

use cortexcode_ai_types::{Content, Message};

/// `inferCopilotInitiator`: `agent` when the last message is not from the user.
pub fn infer_copilot_initiator(messages: &[Message]) -> &'static str {
    match messages.last() {
        Some(Message::User(_)) | None => "user",
        Some(_) => "agent",
    }
}

/// `hasCopilotVisionInput`: any user or tool-result message carries an image.
pub fn has_copilot_vision_input(messages: &[Message]) -> bool {
    messages.iter().any(|msg| match msg {
        Message::User(m) => m.content.iter().any(|c| matches!(c, Content::Image(_))),
        Message::ToolResult(m) => m.content.iter().any(|c| matches!(c, Content::Image(_))),
        Message::Assistant(_) => false,
    })
}

/// `buildCopilotDynamicHeaders`.
pub fn build_copilot_dynamic_headers(
    messages: &[Message],
    has_images: bool,
) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "X-Initiator".to_string(),
            infer_copilot_initiator(messages).to_string(),
        ),
        (
            "Openai-Intent".to_string(),
            "conversation-edits".to_string(),
        ),
    ];
    if has_images {
        headers.push(("Copilot-Vision-Request".to_string(), "true".to_string()));
    }
    headers
}
