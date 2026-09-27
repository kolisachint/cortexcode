//! Shared compaction helpers: hoocode `harness/compaction/utils.ts`
//! (v0.5.89): file-operation tracking, conversation serialization for the
//! summarizer, and its system prompt.

use std::collections::HashSet;

use cortexcode_agent_harness::utils::output_compression::compress_general;
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{Content, Message, UserContent};
use serde_json::Value;

/// `SUMMARIZATION_SYSTEM_PROMPT`.
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI coding assistant, then produce a structured summary following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

/// Maximum characters of a tool result in a serialized summary.
const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// `FileOperations`: paths touched by `read` / `write` / `edit` calls.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileOperations {
    pub read: HashSet<String>,
    pub written: HashSet<String>,
    pub edited: HashSet<String>,
}

/// `createFileOps`.
pub fn create_file_ops() -> FileOperations {
    FileOperations::default()
}

/// `extractFileOpsFromMessage`: the `path` argument of an assistant's
/// `read`, `write` and `edit` tool calls.
pub fn extract_file_ops_from_message(message: &AgentMessage, file_ops: &mut FileOperations) {
    let AgentMessage::Assistant(assistant) = message else {
        return;
    };
    for block in &assistant.content {
        let Content::ToolCall(call) = block else {
            continue;
        };
        let Some(path) = call.arguments["path"].as_str().filter(|p| !p.is_empty()) else {
            continue;
        };
        let set = match call.name.as_str() {
            "read" => &mut file_ops.read,
            "write" => &mut file_ops.written,
            "edit" => &mut file_ops.edited,
            _ => continue,
        };
        set.insert(path.to_string());
    }
}

/// JS `Array.prototype.sort()` order: UTF-16 code units.
fn js_sort(values: &mut [String]) {
    values.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
}

/// `computeFileLists`: `(read_files, modified_files)`, where read files
/// leave out anything modified; both sorted.
pub fn compute_file_lists(file_ops: &FileOperations) -> (Vec<String>, Vec<String>) {
    let modified: HashSet<&String> = file_ops.edited.iter().chain(&file_ops.written).collect();
    let mut read_files: Vec<String> = file_ops
        .read
        .iter()
        .filter(|f| !modified.contains(f))
        .cloned()
        .collect();
    let mut modified_files: Vec<String> = modified.into_iter().cloned().collect();
    js_sort(&mut read_files);
    js_sort(&mut modified_files);
    (read_files, modified_files)
}

/// `formatFileOperations`: `<read-files>` / `<modified-files>` sections,
/// prefixed by a blank line; empty when there are none.
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

/// `str.length` in UTF-16 code units.
pub(crate) fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `truncateForSummary`: compress losslessly, then keep the first
/// `max_chars` (UTF-16 units).
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let compressed = compress_general(text);
    let units: Vec<u16> = compressed.encode_utf16().collect();
    if units.len() <= max_chars {
        return compressed;
    }
    format!(
        "{}\n\n[... {} more characters truncated]",
        String::from_utf16_lossy(&units[..max_chars]),
        units.len() - max_chars
    )
}

fn text_blocks(content: &[Content], separator: &str) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(separator)
}

/// `name(k=<json>, ...)` for a tool call (`JSON.stringify` of each value;
/// serde prints an integral float as `1.0` where JS prints `1`).
fn format_tool_call(name: &str, arguments: &Value) -> String {
    let args = arguments
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    format!("{name}({args})")
}

/// `serializeConversation`: messages as `[User]:` / `[Assistant]:` text so
/// the summarizer does not continue the conversation. Tool results are
/// capped at 2000 characters. Run [`cortexcode_agent_harness::convert_to_llm`]
/// first for harness message types.
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for message in messages {
        match message {
            Message::User(user) => {
                let content = match &user.content {
                    UserContent::Text(text) => text.clone(),
                    UserContent::Blocks(blocks) => text_blocks(blocks, ""),
                };
                if !content.is_empty() {
                    parts.push(format!("[User]: {content}"));
                }
            }
            Message::Assistant(assistant) => {
                let mut text_parts = Vec::new();
                let mut thinking_parts = Vec::new();
                let mut tool_calls = Vec::new();
                for block in &assistant.content {
                    match block {
                        Content::Text(t) => text_parts.push(t.text.as_str()),
                        Content::Thinking(t) => thinking_parts.push(t.thinking.as_str()),
                        Content::ToolCall(call) => {
                            tool_calls.push(format_tool_call(&call.name, &call.arguments))
                        }
                        Content::Image(_) => {}
                    }
                }
                if !thinking_parts.is_empty() {
                    parts.push(format!(
                        "[Assistant thinking]: {}",
                        thinking_parts.join("\n")
                    ));
                }
                if !text_parts.is_empty() {
                    parts.push(format!("[Assistant]: {}", text_parts.join("\n")));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            Message::ToolResult(result) => {
                let content = text_blocks(&result.content, "");
                if !content.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&content, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
        }
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::{AssistantMessage, ToolCallContent};
    use serde_json::json;

    fn call(name: &str, arguments: Value) -> Content {
        Content::ToolCall(ToolCallContent {
            id: "c".into(),
            name: name.into(),
            arguments,
            thought_signature: None,
        })
    }

    #[test]
    fn tracks_read_write_edit_paths_and_splits_read_only_files() {
        let message = AgentMessage::Assistant(AssistantMessage {
            content: vec![
                call("read", json!({"path": "b.ts"})),
                call("read", json!({"path": "a.ts"})),
                call("edit", json!({"path": "b.ts"})),
                call("write", json!({"path": "c.ts"})),
                call("bash", json!({"path": "ignored"})),
                call("read", json!({"path": ""})),
            ],
            ..Default::default()
        });
        let mut ops = create_file_ops();
        extract_file_ops_from_message(&message, &mut ops);
        let (read, modified) = compute_file_lists(&ops);
        assert_eq!(read, ["a.ts"]);
        assert_eq!(modified, ["b.ts", "c.ts"]);
        assert_eq!(
            format_file_operations(&read, &modified),
            "\n\n<read-files>\na.ts\n</read-files>\n\n<modified-files>\nb.ts\nc.ts\n</modified-files>"
        );
        assert_eq!(format_file_operations(&[], &[]), "");
    }

    #[test]
    fn serializes_roles_and_tool_calls() {
        let messages = vec![
            Message::User(cortexcode_ai_types::UserMessage {
                content: "hi".into(),
                timestamp: 0,
            }),
            Message::Assistant(AssistantMessage {
                content: vec![
                    Content::Thinking(cortexcode_ai_types::ThinkingContent {
                        thinking: "plan".into(),
                        ..Default::default()
                    }),
                    Content::text("ok"),
                    call("read", json!({"path": "a", "limit": 5})),
                    call("ls", json!({})),
                ],
                ..Default::default()
            }),
        ];
        assert_eq!(
            serialize_conversation(&messages),
            "[User]: hi\n\n[Assistant thinking]: plan\n\n[Assistant]: ok\n\n[Assistant tool calls]: read(path=\"a\", limit=5); ls()"
        );
    }
}
