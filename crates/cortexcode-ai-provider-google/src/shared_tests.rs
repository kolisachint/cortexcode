//! convertMessages / convertTools tests, incl. ports of
//! `google-shared-*.test.ts` and `google-thinking-signature.test.ts`.

use super::*;
use cortexcode_ai_types::{
    AssistantMessage, ImageContent, TextContent, ToolCallContent, ToolResultMessage, UserMessage,
};

fn model(id: &str, input: &[&str]) -> Model {
    Model {
        compat: None,
        id: id.into(),
        name: id.into(),
        api: "google-generative-ai".into(),
        provider: "google".into(),
        base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
        reasoning: false,
        thinking_level_map: None,
        input: input.iter().map(|s| s.to_string()).collect(),
        cost: cortexcode_ai_types::ModelCost::default(),
        context_window: 1_000_000,
        max_tokens: 8192,
        headers: None,
    }
}

#[test]
fn test_requires_tool_call_id() {
    assert!(requires_tool_call_id("claude-sonnet-5"));
    assert!(requires_tool_call_id("gpt-oss-120b"));
    assert!(!requires_tool_call_id("gemini-2.0-flash"));
}

#[test]
fn test_gemini_major_version() {
    assert_eq!(gemini_major_version("gemini-3-pro"), Some(3));
    assert_eq!(gemini_major_version("gemini-2.5-flash"), Some(2));
    assert_eq!(gemini_major_version("claude-sonnet-5"), None);
}

#[test]
fn test_convert_messages_user_text() {
    let m = model("gemini-2.0-flash", &["text"]);
    let ctx = Context::new(
        "".into(),
        vec![Message::User(UserMessage {
            content: vec![Content::Text(TextContent {
                text_signature: None,
                text: "hi".into(),
            })]
            .into(),
            timestamp: 0,
        })],
        vec![],
    );
    let out = convert_messages(&m, &ctx);
    assert_eq!(out[0]["role"], "user");
    assert_eq!(out[0]["parts"][0]["text"], "hi");
}

#[test]
fn test_convert_messages_user_string_content() {
    let m = model("gemini-2.0-flash", &["text"]);
    let ctx = Context::new(
        "".into(),
        vec![Message::User(UserMessage {
            content: "".into(),
            timestamp: 0,
        })],
        vec![],
    );
    // google-shared.ts sends a string as one text part, even when empty.
    assert_eq!(
        convert_messages(&m, &ctx),
        vec![serde_json::json!({"role": "user", "parts": [{"text": ""}]})]
    );
}

#[test]
fn test_convert_messages_assistant_tool_call() {
    let m = model("gemini-2.0-flash", &["text"]);
    let ctx = Context::new(
        "".into(),
        vec![Message::Assistant(AssistantMessage {
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            content: vec![Content::ToolCall(ToolCallContent {
                thought_signature: None,
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "a.rs"}),
            })],
            stop_reason: StopReason::Stop,

            usage: Default::default(),
            timestamp: 0,
            error_message: None,
        })],
        vec![],
    );
    let out = convert_messages(&m, &ctx);
    assert_eq!(out[0]["role"], "model");
    assert_eq!(out[0]["parts"][0]["functionCall"]["name"], "read_file");
    // Gemini models don't need explicit tool-call IDs on the wire.
    assert!(out[0]["parts"][0]["functionCall"].get("id").is_none());
}

#[test]
fn test_convert_messages_assistant_tool_call_claude_needs_id() {
    let m = model("claude-sonnet-5", &["text"]);
    let ctx = Context::new(
        "".into(),
        vec![Message::Assistant(AssistantMessage {
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            content: vec![Content::ToolCall(ToolCallContent {
                thought_signature: None,
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({}),
            })],
            stop_reason: StopReason::Stop,

            usage: Default::default(),
            timestamp: 0,
            error_message: None,
        })],
        vec![],
    );
    let out = convert_messages(&m, &ctx);
    assert_eq!(out[0]["parts"][0]["functionCall"]["id"], "call_1");
}

#[test]
fn test_convert_messages_tool_result_merges_into_single_user_turn() {
    let m = model("gemini-2.0-flash", &["text"]);
    let messages = vec![
        Message::ToolResult(ToolResultMessage {
            details: None,
            content: vec![Content::Text(TextContent {
                text_signature: None,
                text: "result 1".into(),
            })],
            tool_call_id: "call_1".into(),
            tool_name: "read_file".into(),
            is_error: false,
            timestamp: 0,
        }),
        Message::ToolResult(ToolResultMessage {
            details: None,
            content: vec![Content::Text(TextContent {
                text_signature: None,
                text: "result 2".into(),
            })],
            tool_call_id: "call_2".into(),
            tool_name: "read_file".into(),
            is_error: false,
            timestamp: 0,
        }),
    ];
    let ctx = Context::new("".into(), messages, vec![]);
    let out = convert_messages(&m, &ctx);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["parts"].as_array().unwrap().len(), 2);
}

#[test]
fn test_convert_messages_tool_result_error() {
    let m = model("gemini-2.0-flash", &["text"]);
    let messages = vec![Message::ToolResult(ToolResultMessage {
        details: None,
        content: vec![Content::Text(TextContent {
            text_signature: None,
            text: "boom".into(),
        })],
        tool_call_id: "call_1".into(),
        tool_name: "read_file".into(),
        is_error: true,
        timestamp: 0,
    })];
    let ctx = Context::new("".into(), messages, vec![]);
    let out = convert_messages(&m, &ctx);
    assert_eq!(
        out[0]["parts"][0]["functionResponse"]["response"]["error"],
        "boom"
    );
}

#[test]
fn test_convert_messages_tool_result_with_image_gemini3_inline() {
    let m = model("gemini-3-pro", &["text", "image"]);
    let messages = vec![Message::ToolResult(ToolResultMessage {
        details: None,
        content: vec![Content::Image(ImageContent {
            data: "abc".into(),
            media_type: "image/png".into(),
        })],
        tool_call_id: "call_1".into(),
        tool_name: "read_file".into(),
        is_error: false,
        timestamp: 0,
    })];
    let ctx = Context::new("".into(), messages, vec![]);
    let out = convert_messages(&m, &ctx);
    assert_eq!(
        out.len(),
        1,
        "gemini 3 inlines images in functionResponse.parts"
    );
    assert!(out[0]["parts"][0]["functionResponse"]["parts"][0]["inlineData"].is_object());
}

#[test]
fn test_convert_messages_tool_result_with_image_gemini2_separate_turn() {
    let m = model("gemini-2.0-flash", &["text", "image"]);
    let messages = vec![Message::ToolResult(ToolResultMessage {
        details: None,
        content: vec![Content::Image(ImageContent {
            data: "abc".into(),
            media_type: "image/png".into(),
        })],
        tool_call_id: "call_1".into(),
        tool_name: "read_file".into(),
        is_error: false,
        timestamp: 0,
    })];
    let ctx = Context::new("".into(), messages, vec![]);
    let out = convert_messages(&m, &ctx);
    assert_eq!(
        out.len(),
        2,
        "gemini < 3 sends images in a separate user turn"
    );
    assert!(out[1]["parts"][1]["inlineData"].is_object());
}

#[test]
fn test_convert_tools() {
    let tools = vec![Tool {
        defer_loading: None,
        name: "read_file".into(),
        description: "reads a file".into(),
        parameters: serde_json::json!({"type": "object", "$schema": "x"}),
    }];
    let v = convert_tools(&tools, false).unwrap();
    assert_eq!(v[0]["functionDeclarations"][0]["name"], "read_file");
    assert!(v[0]["functionDeclarations"][0]["parametersJsonSchema"]["$schema"].is_string());
}

#[test]
fn test_convert_tools_use_parameters_strips_schema_meta() {
    let tools = vec![Tool {
        defer_loading: None,
        name: "read_file".into(),
        description: "reads a file".into(),
        parameters: serde_json::json!({"type": "object", "$schema": "x"}),
    }];
    let v = convert_tools(&tools, true).unwrap();
    assert!(v[0]["functionDeclarations"][0]["parameters"]
        .get("$schema")
        .is_none());
}

#[test]
fn test_convert_tools_empty() {
    assert!(convert_tools(&[], false).is_none());
}

#[test]
fn test_map_stop_reason() {
    assert_eq!(map_stop_reason("STOP"), Ok(StopReason::Stop));
    assert_eq!(map_stop_reason("MAX_TOKENS"), Ok(StopReason::Length));
    assert_eq!(map_stop_reason("SAFETY"), Ok(StopReason::Error));
    assert_eq!(
        map_stop_reason("NEW"),
        Err("Unhandled stop reason: NEW".to_string())
    );
    assert_eq!(map_stop_reason_string("NEW"), StopReason::Error);
}

// --- google-thinking-signature.test.ts ---

#[test]
fn only_thought_true_marks_thinking() {
    use serde_json::json;
    assert!(is_thinking_part(&json!({"thought": true})));
    assert!(is_thinking_part(
        &json!({"thought": true, "thoughtSignature": "opaque-signature"})
    ));
    assert!(!is_thinking_part(
        &json!({"thoughtSignature": "opaque-signature"})
    ));
    assert!(!is_thinking_part(
        &json!({"thought": false, "thoughtSignature": "opaque-signature"})
    ));
    assert!(!is_thinking_part(&json!({})));
    assert!(!is_thinking_part(
        &json!({"thought": false, "thoughtSignature": ""})
    ));
}

#[test]
fn retain_thought_signature_keeps_the_last_non_empty_one() {
    let first = retain_thought_signature(None, Some("sig-1"));
    assert_eq!(first.as_deref(), Some("sig-1"));
    let second = retain_thought_signature(first, None);
    assert_eq!(second.as_deref(), Some("sig-1"));
    let third = retain_thought_signature(second, Some(""));
    assert_eq!(third.as_deref(), Some("sig-1"));
    assert_eq!(
        retain_thought_signature(Some("sig-1".into()), Some("sig-2")).as_deref(),
        Some("sig-2")
    );
}

// --- google-shared-convert-tools.test.ts ---

fn schema_tool(parameters: serde_json::Value) -> Tool {
    Tool {
        name: "test_tool".into(),
        description: "A test tool".into(),
        parameters,
        defer_loading: None,
    }
}

fn first_declaration(tools: &[Tool], use_parameters: bool) -> serde_json::Value {
    convert_tools(tools, use_parameters).unwrap()[0]["functionDeclarations"][0].clone()
}

#[test]
fn strips_meta_keys_from_parameters_recursively_but_keeps_refs() {
    use serde_json::json;
    let tools = [schema_tool(json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "urn:bash-tool",
        "$comment": "A bash tool for demonstration",
        "$defs": {"commandDef": {"type": "string"}},
        "definitions": {"legacyDef": {"type": "number"}},
        "type": "object",
        "properties": {
            "command": {"type": "string"},
            "deep": {"$schema": "x", "$id": "urn:nested", "type": "string"},
            "refProp": {"$ref": "#/$defs/someDef", "type": "string"},
        },
        "required": ["command"],
    }))];
    let original = tools[0].parameters.clone();
    assert_eq!(
        first_declaration(&tools, true)["parameters"],
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "deep": {"type": "string"},
                "refProp": {"$ref": "#/$defs/someDef", "type": "string"},
            },
            "required": ["command"],
        })
    );
    // The tool's own schema is untouched.
    assert_eq!(tools[0].parameters, original);
}

#[test]
fn parameters_json_schema_keeps_the_schema_as_is() {
    use serde_json::json;
    let parameters = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "properties": {"command": {"type": "string"}},
        "required": ["command"],
    });
    let decl = first_declaration(&[schema_tool(parameters.clone())], false);
    assert_eq!(decl["parametersJsonSchema"], parameters);
    assert!(decl.get("parameters").is_none());
}

// --- google-shared-gemini3-unsigned-tool-call.test.ts ---

fn gemini3(api: &str, provider: &str, id: &str) -> Model {
    Model {
        api: api.into(),
        provider: provider.into(),
        reasoning: true,
        base_url: "https://example.com".into(),
        ..model(id, &["text"])
    }
}

fn two_tool_calls(api: &str, provider: &str, model_id: &str, signature: Option<&str>) -> Context {
    let call = |id: &str, command: &str, sig: Option<&str>| {
        Content::ToolCall(ToolCallContent {
            id: id.into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": command}),
            thought_signature: sig.map(str::to_string),
        })
    };
    Context::new(
        String::new(),
        vec![
            Message::User(UserMessage {
                content: "Hi".into(),
                timestamp: 1,
            }),
            Message::Assistant(AssistantMessage {
                content: vec![
                    call("call_1", "echo hi", signature),
                    call("call_2", "ls -la", None),
                ],
                api: api.into(),
                provider: provider.into(),
                model: model_id.into(),
                stop_reason: StopReason::ToolUse,
                ..Default::default()
            }),
        ],
        vec![],
    )
}

fn function_call_parts(contents: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let turn = contents
        .iter()
        .find(|c| c["role"] == "model")
        .expect("model turn");
    assert!(!turn
        .to_string()
        .contains("skip_thought_signature_validator"));
    turn["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p.get("functionCall").is_some())
        .cloned()
        .collect()
}

#[test]
fn unsigned_tool_calls_get_no_signature_or_validator() {
    let google = gemini3("google-generative-ai", "google", "gemini-3-pro-preview");
    let parts = function_call_parts(&convert_messages(
        &google,
        &two_tool_calls("google-generative-ai", "google", "other-model", None),
    ));
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|p| p.get("thoughtSignature").is_none()));

    let vertex = gemini3("google-vertex", "google-vertex", "gemini-3-pro-preview");
    let parts = function_call_parts(&convert_messages(
        &vertex,
        &two_tool_calls(
            "google-vertex",
            "google-vertex",
            "gemini-3-pro-preview",
            None,
        ),
    ));
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|p| p.get("thoughtSignature").is_none()));

    let older = gemini3("google-generative-ai", "google", "gemini-2.5-flash");
    let parts = function_call_parts(&convert_messages(
        &older,
        &two_tool_calls("google-generative-ai", "google", "other-model", None),
    ));
    assert!(parts[0].get("thoughtSignature").is_none());
}

#[test]
fn a_valid_signature_is_kept_for_the_same_provider_and_model() {
    let google = gemini3("google-generative-ai", "google", "gemini-3-pro-preview");
    let signature = "AAAAAAAAAAAAAAAAAAAAAA==";
    let parts = function_call_parts(&convert_messages(
        &google,
        &two_tool_calls(
            "google-generative-ai",
            "google",
            "gemini-3-pro-preview",
            Some(signature),
        ),
    ));
    assert_eq!(parts[0]["thoughtSignature"], signature);
    assert!(parts[1].get("thoughtSignature").is_none());
    // Not base64 (or from another model): dropped.
    for (sig, model_id) in [
        ("not base64!", "gemini-3-pro-preview"),
        (signature, "other"),
    ] {
        let parts = function_call_parts(&convert_messages(
            &google,
            &two_tool_calls("google-generative-ai", "google", model_id, Some(sig)),
        ));
        assert!(
            parts[0].get("thoughtSignature").is_none(),
            "{sig} {model_id}"
        );
    }
}

// --- google-shared-image-tool-result-routing.test.ts ---

fn image_routing_context(model_id: &str) -> Context {
    let read = |id: &str, path: &str| {
        Content::ToolCall(ToolCallContent {
            id: id.into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": path}),
            thought_signature: None,
        })
    };
    let result = |id: &str, content: Content| {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "read".into(),
            content: vec![content],
            details: None,
            is_error: false,
            timestamp: 1,
        })
    };
    Context::new(
        String::new(),
        vec![
            Message::User(UserMessage {
                content: "read the files".into(),
                timestamp: 1,
            }),
            Message::Assistant(AssistantMessage {
                content: vec![
                    read("call_a", "a.txt"),
                    read("call_img", "image.png"),
                    read("call_b", "b.txt"),
                ],
                api: "google-generative-ai".into(),
                provider: "google".into(),
                model: model_id.into(),
                stop_reason: StopReason::ToolUse,
                ..Default::default()
            }),
            result("call_a", Content::text("alpha text")),
            result(
                "call_img",
                Content::Image(ImageContent {
                    data: "abc".into(),
                    media_type: "image/png".into(),
                }),
            ),
            result("call_b", Content::text("beta text")),
        ],
        vec![],
    )
}

#[test]
fn gemini_2_keeps_a_separate_synthetic_image_turn() {
    let m = model("gemini-2.5-flash", &["text", "image"]);
    let contents = convert_messages(&m, &image_routing_context("gemini-2.5-flash"));
    assert_eq!(contents.len(), 5);
    assert!(contents[2]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p.get("functionResponse").is_some()));
    assert_eq!(contents[3]["parts"][0]["text"], "Tool result image:");
    assert!(contents[3]["parts"][1].get("inlineData").is_some());
    assert!(contents[4]["parts"][0].get("functionResponse").is_some());
}

#[test]
fn gemini_3_nests_image_tool_results() {
    let m = model("gemini-3-pro-preview", &["text", "image"]);
    let contents = convert_messages(&m, &image_routing_context("gemini-3-pro-preview"));
    assert_eq!(contents.len(), 3);
    let parts = contents[2]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 3);
    let image_response = &parts[1]["functionResponse"];
    assert_eq!(
        image_response["response"],
        serde_json::json!({"output": "(see attached image)"})
    );
    assert_eq!(image_response["parts"].as_array().unwrap().len(), 1);
    assert!(image_response["parts"][0].get("inlineData").is_some());
}
