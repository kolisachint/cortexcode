//! Tests for the shared Responses plumbing.
//!
//! Ported from `openai-responses-foreign-toolcall-id.test.ts`,
//! `openai-responses-partial-json-cleanup.test.ts` and the conversion half of
//! `openai-responses-tool-result-images.test.ts` (hoocode v0.5.89), plus the
//! event handling of `processResponsesStream`.

use super::*;
use cortexcode_ai_stream::create_assistant_message_event_stream;
use cortexcode_ai_types::{ImageContent, ToolResultMessage, UserMessage};

pub(crate) fn catalog(provider: &str, id: &str) -> Model {
    cortexcode_ai_models::get_model(provider, id)
        .unwrap_or_else(|| panic!("{provider}/{id} in catalog"))
        .clone()
}

pub(crate) fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: vec![Content::Text(TextContent::new(text))],
        timestamp: 0,
    })
}

fn allowed() -> HashSet<&'static str> {
    ["openai", "openai-codex", "opencode"].into_iter().collect()
}

const COPILOT_RAW_TOOL_CALL_ID: &str = "call_4VnzVawQXPB9MgYib7CiQFEY|I9b95oN1wD/cHXKTw3PpRkL6KkCtzTJhUxMouMWYwHeTo2j3htzfSk7YPx2vifiIM4g3A8XXyOj8q4Bt6SLUG7gqY1E3ELkrkVQNHglRfUmWj84lqxJY+Puieb3VKyX0FB+83TUzn91cDMF/4gzt990IzqVrc+nIb9RRscRD070Du16q1glydVjWR0SBJsE6TbY/esOjFpqplogQqrajm1eI++f3eLi73R6q7hVusY0QbeFySVxABCjhN0lXB04caBe1rzHjYzul6MAXj7uq+0r17VLq+yrtyYhN12wkmFqHeqTyEei6EFPbMy24Nc+IbJlkP0OCg02W+gOnyBFcbi2ctvJFSOhSjt1CqBdqCnnhwUqXjbWiT0wh3DmLScRgTHmGkaI+oAcQQjfic65nxj+TnEkReA==";

fn tool_call_assistant(
    id: &str,
    provider: &str,
    api: &str,
    model: &str,
    content_extra: Vec<Content>,
) -> Message {
    let mut content = content_extra;
    content.push(Content::ToolCall(ToolCallContent {
        id: id.into(),
        name: "edit".into(),
        arguments: json!({"path": "src/styles/app.css"}),
        thought_signature: None,
    }));
    Message::Assistant(AssistantMessage {
        content,
        api: api.into(),
        provider: provider.into(),
        model: model.into(),
        stop_reason: StopReason::ToolUse,
        ..Default::default()
    })
}

fn tool_result(id: &str, content: Vec<Content>) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "edit".into(),
        content,
        details: None,
        is_error: false,
        timestamp: 0,
    })
}

// --- openai-responses-foreign-toolcall-id.test.ts ---

#[test]
fn hashes_foreign_copilot_tool_item_ids_into_bounded_fc_hash() {
    let model = catalog("openai-codex", "gpt-5.3-codex");
    let context = Context::new(
        "You are concise.".into(),
        vec![
            user("Use the tool."),
            tool_call_assistant(
                COPILOT_RAW_TOOL_CALL_ID,
                "github-copilot",
                "openai-responses",
                "gpt-5.3-codex",
                vec![],
            ),
            tool_result(
                COPILOT_RAW_TOOL_CALL_ID,
                vec![Content::Text(TextContent::new("ok"))],
            ),
        ],
        vec![],
    );
    let input = convert_responses_messages(&model, &context, &allowed(), true);
    let call = input
        .iter()
        .find(|i| i["type"] == "function_call")
        .expect("function_call item");
    let item_id = COPILOT_RAW_TOOL_CALL_ID.split('|').nth(1).unwrap();
    let expected = format!("fc_{}", short_hash(item_id));
    assert_eq!(call["id"], json!(expected));
    let id = call["id"].as_str().unwrap();
    assert!(id.len() <= 64);
    assert!(id.starts_with("fc_") && id[3..].chars().all(|c| c.is_ascii_alphanumeric()));
    // The tool result points at the normalized call id.
    let output = input
        .iter()
        .find(|i| i["type"] == "function_call_output")
        .unwrap();
    assert_eq!(output["call_id"], "call_4VnzVawQXPB9MgYib7CiQFEY");
}

// --- openai-responses-tool-result-images.test.ts (conversion) ---

#[test]
fn tool_result_images_stay_in_function_call_output() {
    let model = catalog("openai", "gpt-5-mini");
    let id = "call_1|fc_1";
    let context = Context::new(
        String::new(),
        vec![
            user("Call the tool"),
            tool_call_assistant(id, "openai", "openai-responses", "gpt-5-mini", vec![]),
            tool_result(
                id,
                vec![
                    Content::Text(TextContent::new(
                        "A red circle with a diameter of 100 pixels.",
                    )),
                    Content::Image(ImageContent {
                        data: "ZmFrZQ==".into(),
                        media_type: "image/png".into(),
                    }),
                ],
            ),
        ],
        vec![],
    );
    let input = convert_responses_messages(&model, &context, &allowed(), true);
    let idx = input
        .iter()
        .position(|i| i["type"] == "function_call_output")
        .unwrap();
    let output = input[idx]["output"].as_array().expect("content array");
    assert_eq!(
        output[0],
        json!({"type": "input_text", "text": "A red circle with a diameter of 100 pixels."})
    );
    assert_eq!(output[1]["type"], "input_image");
    assert!(output[1]["image_url"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    assert!(input[idx + 1..].iter().all(|i| i["role"] != "user"));
}

// --- convertResponsesMessages behaviour ---

#[test]
fn converts_system_user_assistant_and_results() {
    let model = catalog("openai", "gpt-5-mini");
    let reasoning_item = json!({"type": "reasoning", "id": "rs_1", "summary": []});
    let context = Context::new(
        "sys".into(),
        vec![
            user("hi"),
            tool_call_assistant(
                "call_1|fc_1",
                "openai",
                "openai-responses",
                "gpt-5-mini",
                vec![
                    Content::Thinking(ThinkingContent {
                        thinking: "t".into(),
                        signature: Some(reasoning_item.to_string()),
                        redacted: false,
                    }),
                    Content::Text(TextContent {
                        text: "answer".into(),
                        text_signature: Some(encode_text_signature_v1(
                            "msg_abc",
                            Some("commentary"),
                        )),
                    }),
                ],
            ),
            tool_result("call_1|fc_1", vec![]),
        ],
        vec![],
    );
    let input = convert_responses_messages(&model, &context, &allowed(), true);
    assert_eq!(
        input,
        vec![
            json!({"role": "developer", "content": "sys"}),
            json!({"role": "user", "content": [{"type": "input_text", "text": "hi"}]}),
            reasoning_item,
            json!({"type": "message", "role": "assistant",
                   "content": [{"type": "output_text", "text": "answer", "annotations": []}],
                   "status": "completed", "id": "msg_abc", "phase": "commentary"}),
            json!({"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "edit",
                   "arguments": "{\"path\":\"src/styles/app.css\"}"}),
            json!({"type": "function_call_output", "call_id": "call_1", "output": "(see attached image)"}),
        ]
    );
    let without_system = convert_responses_messages(&model, &context, &allowed(), false);
    assert_eq!(without_system[0]["role"], "user");
}

#[test]
fn different_model_drops_fc_item_ids_and_text_ids_default_to_msg_index() {
    let model = catalog("openai", "gpt-5-mini");
    let context = Context::new(
        String::new(),
        vec![
            user("hi"),
            tool_call_assistant(
                "call_1|fc_1",
                "openai",
                "openai-responses",
                "gpt-5",
                vec![Content::Text(TextContent::new("legacy"))],
            ),
        ],
        vec![],
    );
    let input = convert_responses_messages(&model, &context, &allowed(), true);
    assert_eq!(input[1]["id"], "msg_1");
    assert!(input[2].get("id").is_none());
    assert_eq!(input[2]["call_id"], "call_1");
}

#[test]
fn providers_outside_the_allowed_set_flatten_pipe_ids() {
    let mut model = catalog("openai", "gpt-5-mini");
    model.provider = "github-copilot".into();
    let context = Context::new(
        String::new(),
        vec![tool_call_assistant(
            "call_1|fc+x/y",
            "openai",
            "openai-responses",
            "gpt-5-mini",
            vec![],
        )],
        vec![],
    );
    let input = convert_responses_messages(&model, &context, &allowed(), true);
    assert_eq!(input[0]["call_id"], "call_1_fc_x_y");
    assert!(input[0].get("id").is_none());
}

#[test]
fn tools_are_strict_false_unless_constrained() {
    let tools = vec![Tool {
        name: "t".into(),
        description: "d".into(),
        parameters: json!({"type": "object", "properties": {"a": {"type": "string"}}}),
    }];
    assert_eq!(
        convert_responses_tools(&tools, None, false),
        vec![json!({"type": "function", "name": "t", "description": "d",
                    "parameters": {"type": "object", "properties": {"a": {"type": "string"}}},
                    "strict": false})]
    );
    let strict = convert_responses_tools(&tools, None, true);
    assert_eq!(strict[0]["strict"], true);
    assert_eq!(strict[0]["parameters"]["additionalProperties"], false);
}

// --- processResponsesStream ---

fn process(
    model: &Model,
    events: &[Value],
) -> (
    Vec<AssistantMessageEvent>,
    AssistantMessage,
    Result<(), String>,
) {
    let sender = create_assistant_message_event_stream();
    let mut state = ResponsesStreamState::new(model);
    let mut result = Ok(());
    for e in events {
        result = state.handle_event(e, model, &ResponsesStreamOptions::default(), &sender);
        if result.is_err() {
            break;
        }
    }
    sender.end(Some(state.output.clone()));
    let mut s = sender;
    let mut out = Vec::new();
    while let Some(e) = s.next_blocking() {
        out.push(e);
    }
    (out, state.output, result)
}

fn responses_model() -> Model {
    Model {
        id: "gpt-5-mini".into(),
        name: "GPT-5 Mini".into(),
        api: "openai-responses".into(),
        provider: "openai".into(),
        base_url: "https://api.openai.com/v1".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: Default::default(),
        context_window: 400_000,
        max_tokens: 128_000,
        headers: None,
        compat: None,
    }
}

#[test]
fn removes_partial_json_from_persisted_tool_call_blocks() {
    let args = r#"{"path":"README.md","content":"updated"}"#;
    let item = json!({"type": "function_call", "id": "fc_test", "call_id": "call_test", "name": "edit", "arguments": ""});
    let mut done = item.clone();
    done["arguments"] = json!(args);
    let (events, output, result) = process(
        &responses_model(),
        &[
            json!({"type": "response.output_item.added", "item": item}),
            json!({"type": "response.function_call_arguments.delta", "delta": "{\"path\":\"README.md\""}),
            json!({"type": "response.function_call_arguments.delta", "delta": ",\"content\":\"updated\"}"}),
            json!({"type": "response.function_call_arguments.done", "arguments": args}),
            json!({"type": "response.output_item.done", "item": done}),
        ],
    );
    assert!(result.is_ok());
    assert_eq!(output.content.len(), 1);
    let Content::ToolCall(tc) = &output.content[0] else {
        panic!("tool call")
    };
    assert_eq!(tc.id, "call_test|fc_test");
    assert_eq!(
        tc.arguments,
        json!({"path": "README.md", "content": "updated"})
    );
    let serialized = serde_json::to_value(&output.content[0]).unwrap();
    assert!(serialized.get("partialJson").is_none());
    let end = events
        .iter()
        .find_map(|e| match e {
            AssistantMessageEvent::ToolCallEnd { partial, index } => Some((partial, *index)),
            _ => None,
        })
        .expect("toolcall_end");
    assert_eq!(end.1, 0);
    assert_eq!(end.0.content[0], output.content[0]);
    // The `done` arguments add nothing beyond the deltas: no extra delta event.
    let deltas = events
        .iter()
        .filter(|e| matches!(e, AssistantMessageEvent::ToolCallDelta { .. }))
        .count();
    assert_eq!(deltas, 2);
}

#[test]
fn reasoning_summaries_text_and_completion() {
    let reasoning = json!({"type": "reasoning", "id": "rs_1", "summary": []});
    let reasoning_done = json!({"type": "reasoning", "id": "rs_1", "summary": [
        {"type": "summary_text", "text": "Plan A"}, {"type": "summary_text", "text": "Plan B"}
    ], "encrypted_content": "enc"});
    let message = json!({"type": "message", "id": "msg_1", "role": "assistant", "content": []});
    let message_done = json!({"type": "message", "id": "msg_1", "role": "assistant", "phase": "final_answer",
        "content": [{"type": "output_text", "text": "Hello", "annotations": []}]});
    let (events, output, result) = process(
        &responses_model(),
        &[
            json!({"type": "response.created", "response": {"id": "resp_1"}}),
            json!({"type": "response.output_item.added", "item": reasoning}),
            json!({"type": "response.reasoning_summary_part.added", "part": {"type": "summary_text", "text": ""}}),
            json!({"type": "response.reasoning_summary_text.delta", "delta": "Plan A"}),
            json!({"type": "response.reasoning_summary_part.done"}),
            json!({"type": "response.reasoning_summary_part.added", "part": {"type": "summary_text", "text": ""}}),
            json!({"type": "response.reasoning_summary_text.delta", "delta": "Plan B"}),
            json!({"type": "response.output_item.done", "item": reasoning_done}),
            json!({"type": "response.output_item.added", "item": message}),
            json!({"type": "response.output_text.delta", "delta": "ignored before a part"}),
            json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": ""}}),
            json!({"type": "response.output_text.delta", "delta": "Hel"}),
            json!({"type": "response.output_text.delta", "delta": "lo"}),
            json!({"type": "response.output_item.done", "item": message_done}),
            json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed",
                "usage": {"input_tokens": 100, "output_tokens": 20, "total_tokens": 120,
                          "input_tokens_details": {"cached_tokens": 40}}}}),
        ],
    );
    assert!(result.is_ok());
    assert_eq!(output.response_id.as_deref(), Some("resp_1"));
    let Content::Thinking(t) = &output.content[0] else {
        panic!("thinking")
    };
    assert_eq!(t.thinking, "Plan A\n\nPlan B");
    assert_eq!(
        t.signature.as_deref(),
        Some(reasoning_done.to_string().as_str())
    );
    let Content::Text(text) = &output.content[1] else {
        panic!("text")
    };
    assert_eq!(text.text, "Hello");
    assert_eq!(
        text.text_signature.as_deref(),
        Some(r#"{"v":1,"id":"msg_1","phase":"final_answer"}"#)
    );
    let thinking_deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ThinkingDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thinking_deltas, ["Plan A", "\n\n", "Plan B"]);
    let text_deltas = events
        .iter()
        .filter(|e| matches!(e, AssistantMessageEvent::TextDelta { .. }))
        .count();
    assert_eq!(text_deltas, 2);
    assert_eq!(
        (
            output.usage.input,
            output.usage.output,
            output.usage.cache_read,
            output.usage.total_tokens
        ),
        (60, 20, 40, 120)
    );
    assert_eq!(output.stop_reason, StopReason::Stop);
}

#[test]
fn refusals_become_text_and_tool_calls_force_tool_use() {
    let message = json!({"type": "message", "id": "m", "content": []});
    let (_, output, _) = process(
        &responses_model(),
        &[
            json!({"type": "response.output_item.added", "item": message}),
            json!({"type": "response.content_part.added", "part": {"type": "refusal", "refusal": ""}}),
            json!({"type": "response.refusal.delta", "delta": "No."}),
            json!({"type": "response.output_item.done", "item": {"type": "message", "id": "m",
                "content": [{"type": "refusal", "refusal": "No."}]}}),
            json!({"type": "response.output_item.added", "item": {"type": "function_call", "id": "fc_1", "call_id": "c", "name": "x", "arguments": ""}}),
            json!({"type": "response.output_item.done", "item": {"type": "function_call", "id": "fc_1", "call_id": "c", "name": "x", "arguments": "{\"a\":1}"}}),
            json!({"type": "response.completed", "response": {"status": "completed"}}),
        ],
    );
    assert_eq!(
        output.content[0],
        Content::Text(TextContent {
            text: "No.".into(),
            text_signature: Some(r#"{"v":1,"id":"m"}"#.into()),
        })
    );
    let Content::ToolCall(tc) = &output.content[1] else {
        panic!("tool call")
    };
    assert_eq!(tc.arguments, json!({"a": 1}));
    assert_eq!(output.stop_reason, StopReason::ToolUse);
}

#[test]
fn error_and_failed_events_throw() {
    let (_, _, r) = process(
        &responses_model(),
        &[json!({"type": "error", "code": "rate_limit", "message": "slow down"})],
    );
    assert_eq!(r, Err("Error Code rate_limit: slow down".to_string()));
    let (_, _, r) = process(
        &responses_model(),
        &[
            json!({"type": "response.failed", "response": {"error": {"code": "server_error", "message": "boom"}}}),
        ],
    );
    assert_eq!(r, Err("server_error: boom".to_string()));
    let (_, _, r) = process(
        &responses_model(),
        &[
            json!({"type": "response.failed", "response": {"incomplete_details": {"reason": "max_output_tokens"}}}),
        ],
    );
    assert_eq!(r, Err("incomplete: max_output_tokens".to_string()));
    let (_, _, r) = process(
        &responses_model(),
        &[json!({"type": "response.failed", "response": {}})],
    );
    assert_eq!(
        r,
        Err("Unknown error (no error details in response)".to_string())
    );
}

#[test]
fn stop_reason_mapping() {
    assert_eq!(map_stop_reason(None), StopReason::Stop);
    assert_eq!(map_stop_reason(Some("completed")), StopReason::Stop);
    assert_eq!(map_stop_reason(Some("incomplete")), StopReason::Length);
    assert_eq!(map_stop_reason(Some("failed")), StopReason::Error);
    assert_eq!(map_stop_reason(Some("cancelled")), StopReason::Error);
    assert_eq!(map_stop_reason(Some("in_progress")), StopReason::Stop);
    assert_eq!(map_stop_reason(Some("queued")), StopReason::Stop);
}

#[test]
fn text_signatures_round_trip() {
    assert_eq!(
        parse_text_signature(Some(&encode_text_signature_v1("msg_1", Some("commentary")))),
        Some(("msg_1".into(), Some("commentary".into())))
    );
    assert_eq!(
        parse_text_signature(Some(r#"{"v":1,"id":"x","phase":"other"}"#)),
        Some(("x".into(), None))
    );
    assert_eq!(
        parse_text_signature(Some("legacy_id")),
        Some(("legacy_id".into(), None))
    );
    assert_eq!(parse_text_signature(None), None);
}
