//! hoocode messages and tool calls → Codex thread items. Pure functions.

use cortexcode_agent_types::{AgentMessage, AgentToolResult};
use cortexcode_ai_types::{Content, UserContent};
use cortexcode_app_server_protocol::{
    DynamicToolCallStatus, FileUpdateChange, ItemStatus, PatchChangeKind, ThreadItem, Turn,
    TurnStatus, UserInput,
};
use serde_json::{json, Value};

/// How a tool call shows up in the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Command,
    FileChange,
    Dynamic,
}

pub fn tool_kind(tool_name: &str) -> ToolKind {
    match tool_name {
        "bash" => ToolKind::Command,
        "edit" | "write" => ToolKind::FileChange,
        _ => ToolKind::Dynamic,
    }
}

/// Text of a list of content blocks (text blocks only).
pub fn content_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// The item a tool call starts as (status `inProgress`).
pub fn tool_item_started(id: &str, tool_name: &str, args: &Value, cwd: &str) -> ThreadItem {
    tool_item(id, tool_name, args, cwd, None)
}

/// The finished state of a tool item.
pub enum ToolOutcome<'a> {
    Done {
        result: &'a AgentToolResult,
        is_error: bool,
    },
    Declined,
}

pub fn tool_item_completed(
    id: &str,
    tool_name: &str,
    args: &Value,
    cwd: &str,
    outcome: ToolOutcome<'_>,
) -> ThreadItem {
    tool_item(id, tool_name, args, cwd, Some(outcome))
}

fn tool_item(
    id: &str,
    tool_name: &str,
    args: &Value,
    cwd: &str,
    outcome: Option<ToolOutcome<'_>>,
) -> ThreadItem {
    let status = match &outcome {
        None => ItemStatus::InProgress,
        Some(ToolOutcome::Declined) => ItemStatus::Declined,
        Some(ToolOutcome::Done { is_error: true, .. }) => ItemStatus::Failed,
        Some(ToolOutcome::Done { .. }) => ItemStatus::Completed,
    };
    let result = match &outcome {
        Some(ToolOutcome::Done { result, .. }) => Some(*result),
        _ => None,
    };
    match tool_kind(tool_name) {
        ToolKind::Command => ThreadItem::CommandExecution {
            id: id.into(),
            command: args
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            cwd: cwd.into(),
            source: "agent".into(),
            status,
            command_actions: vec![],
            aggregated_output: result.map(|r| content_text(&r.content)),
            exit_code: result.and_then(exit_code),
            duration_ms: None,
        },
        ToolKind::FileChange => ThreadItem::FileChange {
            id: id.into(),
            changes: vec![file_change(tool_name, args, cwd, result)],
            status,
        },
        ToolKind::Dynamic => ThreadItem::DynamicToolCall {
            id: id.into(),
            tool: tool_name.into(),
            arguments: args.clone(),
            status: match status {
                ItemStatus::InProgress => DynamicToolCallStatus::InProgress,
                ItemStatus::Completed => DynamicToolCallStatus::Completed,
                ItemStatus::Failed | ItemStatus::Declined => DynamicToolCallStatus::Failed,
            },
            success: outcome.as_ref().map(|o| {
                matches!(
                    o,
                    ToolOutcome::Done {
                        is_error: false,
                        ..
                    }
                )
            }),
            content_items: result
                .map(|r| vec![json!({"type": "inputText", "text": content_text(&r.content)})]),
            duration_ms: None,
        },
    }
}

fn exit_code(result: &AgentToolResult) -> Option<i32> {
    result
        .details
        .get("exitCode")
        .and_then(Value::as_i64)
        .map(|c| c as i32)
}

fn file_change(
    tool_name: &str,
    args: &Value,
    cwd: &str,
    result: Option<&AgentToolResult>,
) -> FileUpdateChange {
    let raw = cortexcode_code_permissions::mutation_path(args).unwrap_or_default();
    let path = if raw.is_empty() || std::path::Path::new(&raw).is_absolute() {
        raw
    } else {
        std::path::Path::new(cwd)
            .join(&raw)
            .to_string_lossy()
            .into_owned()
    };
    let diff = result
        .and_then(|r| r.details.get("diff"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            (tool_name == "write").then(|| {
                args.get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
        })
        .unwrap_or_default();
    let kind = if tool_name == "write" {
        PatchChangeKind::Add
    } else {
        PatchChangeKind::Update { move_path: None }
    };
    FileUpdateChange { path, kind, diff }
}

/// User input → hoocode prompt text and image URLs (data URLs stay as they are).
pub fn input_text(input: &[UserInput]) -> String {
    input
        .iter()
        .filter_map(|i| match i {
            UserInput::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The user's prompt as a protocol item.
pub fn user_message_item(id: &str, content: &UserContent) -> ThreadItem {
    let text = match content {
        UserContent::Text(t) => t.clone(),
        UserContent::Blocks(blocks) => content_text(blocks),
    };
    ThreadItem::UserMessage {
        id: id.into(),
        content: vec![UserInput::Text { text }],
    }
}

/// Rebuild turns from a saved session's messages. A turn starts at each user
/// message; item ids are derived from positions so they are stable across reads.
pub fn turns_from_messages(messages: &[AgentMessage], cwd: &str) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut pending_tools: Vec<(String, String, Value, usize)> = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        match message {
            AgentMessage::User(user) => {
                let turn_id = format!("turn-{index}");
                turns.push(Turn {
                    id: turn_id.clone(),
                    items: vec![user_message_item(&format!("{turn_id}-user"), &user.content)],
                    status: TurnStatus::Completed,
                    error: None,
                    started_at: Some(user.timestamp / 1000),
                    completed_at: None,
                    duration_ms: None,
                });
                pending_tools.clear();
            }
            AgentMessage::Assistant(assistant) => {
                let Some(turn) = turns.last_mut() else {
                    continue;
                };
                let mut thinking = Vec::new();
                let mut text = String::new();
                for block in &assistant.content {
                    match block {
                        Content::Thinking(t) => thinking.push(t.thinking.clone()),
                        Content::Text(t) => text.push_str(&t.text),
                        Content::ToolCall(call) => {
                            let item =
                                tool_item_started(&call.id, &call.name, &call.arguments, cwd);
                            pending_tools.push((
                                call.id.clone(),
                                call.name.clone(),
                                call.arguments.clone(),
                                turn.items.len(),
                            ));
                            turn.items.push(item);
                        }
                        _ => {}
                    }
                }
                if !thinking.is_empty() {
                    turn.items.push(ThreadItem::Reasoning {
                        id: format!("msg-{index}-reasoning"),
                        summary: vec![],
                        content: thinking,
                    });
                }
                if !text.is_empty() {
                    turn.items.push(ThreadItem::AgentMessage {
                        id: format!("msg-{index}"),
                        text,
                    });
                }
                if let Some(error) = &assistant.error_message {
                    turn.error = Some(cortexcode_app_server_protocol::TurnError {
                        message: error.clone(),
                        additional_details: None,
                    });
                }
                turn.status = match assistant.stop_reason {
                    cortexcode_ai_types::StopReason::Error => TurnStatus::Failed,
                    cortexcode_ai_types::StopReason::Aborted => TurnStatus::Interrupted,
                    _ => TurnStatus::Completed,
                };
                turn.completed_at = Some(assistant.timestamp / 1000);
            }
            AgentMessage::ToolResult(tool_result) => {
                let Some(turn) = turns.last_mut() else {
                    continue;
                };
                let Some(pos) = pending_tools
                    .iter()
                    .position(|(id, ..)| *id == tool_result.tool_call_id)
                else {
                    continue;
                };
                let (id, name, args, slot) = pending_tools.remove(pos);
                let result = AgentToolResult {
                    content: tool_result.content.clone(),
                    details: tool_result.details.clone().unwrap_or(Value::Null),
                    terminate: false,
                };
                turn.items[slot] = tool_item_completed(
                    &id,
                    &name,
                    &args,
                    cwd,
                    ToolOutcome::Done {
                        result: &result,
                        is_error: tool_result.is_error,
                    },
                );
            }
            _ => {}
        }
    }
    turns
}

/// The first user message's text, for previews.
pub fn preview(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .find_map(|m| match m {
            AgentMessage::User(u) => Some(match &u.content {
                UserContent::Text(t) => t.clone(),
                UserContent::Blocks(b) => content_text(b),
            }),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::{AssistantMessage, ToolCallContent, ToolResultMessage, UserMessage};

    fn user(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: UserContent::Text(text.into()),
            timestamp: 1_000,
        })
    }

    #[test]
    fn bash_maps_to_command_execution() {
        let args = json!({"command": "ls -la"});
        let started = tool_item_started("c1", "bash", &args, "/w");
        assert!(matches!(
            &started,
            ThreadItem::CommandExecution { command, status: ItemStatus::InProgress, .. } if command == "ls -la"
        ));
        let result = AgentToolResult {
            content: vec![Content::text("a\nb")],
            details: json!({}),
            terminate: false,
        };
        let done = tool_item_completed(
            "c1",
            "bash",
            &args,
            "/w",
            ToolOutcome::Done {
                result: &result,
                is_error: false,
            },
        );
        assert!(matches!(
            done,
            ThreadItem::CommandExecution { status: ItemStatus::Completed, aggregated_output: Some(ref o), .. } if o == "a\nb"
        ));
        let declined = tool_item_completed("c1", "bash", &args, "/w", ToolOutcome::Declined);
        assert!(matches!(
            declined,
            ThreadItem::CommandExecution {
                status: ItemStatus::Declined,
                ..
            }
        ));
    }

    #[test]
    fn edit_maps_to_file_change_with_absolute_path() {
        let item = tool_item_started("e", "edit", &json!({"path": "src/a.rs"}), "/w");
        let ThreadItem::FileChange { changes, .. } = item else {
            panic!("not a file change")
        };
        assert_eq!(changes[0].path, "/w/src/a.rs");
        let item = tool_item_started(
            "w",
            "write",
            &json!({"path": "/x/b", "content": "hi"}),
            "/w",
        );
        let ThreadItem::FileChange { changes, .. } = item else {
            panic!("not a file change")
        };
        assert_eq!(changes[0].kind, PatchChangeKind::Add);
        assert_eq!(changes[0].diff, "hi");
    }

    #[test]
    fn other_tools_are_dynamic() {
        let item = tool_item_completed(
            "r",
            "read",
            &json!({"path": "a"}),
            "/w",
            ToolOutcome::Declined,
        );
        assert!(matches!(
            item,
            ThreadItem::DynamicToolCall {
                status: DynamicToolCallStatus::Failed,
                success: Some(false),
                ..
            }
        ));
    }

    #[test]
    fn rebuilds_turns_from_messages() {
        let messages = vec![
            user("hello"),
            AgentMessage::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCallContent {
                    id: "t1".into(),
                    name: "bash".into(),
                    arguments: json!({"command": "ls"}),
                    thought_signature: None,
                })],
                stop_reason: cortexcode_ai_types::StopReason::ToolUse,
                timestamp: 2_000,
                ..Default::default()
            }),
            AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id: "t1".into(),
                tool_name: "bash".into(),
                content: vec![Content::text("out")],
                details: None,
                is_error: false,
                timestamp: 3_000,
            }),
            AgentMessage::Assistant(AssistantMessage {
                content: vec![Content::text("done")],
                timestamp: 4_000,
                ..Default::default()
            }),
            user("again"),
        ];
        let turns = turns_from_messages(&messages, "/w");
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].items.len(), 3);
        assert!(matches!(
            &turns[0].items[1],
            ThreadItem::CommandExecution {
                status: ItemStatus::Completed,
                ..
            }
        ));
        assert!(
            matches!(&turns[0].items[2], ThreadItem::AgentMessage { text, .. } if text == "done")
        );
        assert_eq!(preview(&messages), "hello");
        assert_eq!(
            turns_from_messages(&messages, "/w"),
            turns,
            "ids are stable"
        );
    }
}
