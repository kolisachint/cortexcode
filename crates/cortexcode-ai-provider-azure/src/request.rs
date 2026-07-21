//! Request construction for the Azure OpenAI Responses API.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `providers/azure-openai-responses.ts` and the shared
//! `providers/openai-responses-shared.ts` (request-building portion).
//!
//! Reasoning-item ID pairing is ported: a tool call's `ToolCallContent::id`
//! may be pipe-encoded as `call_id|item_id`, where `item_id` is the Responses
//! API `function_call` item id (`fc_...`) that Azure pairs with the preceding
//! `reasoning` item (`rs_...`). On replay we split the two parts, re-emit the
//! reasoning item verbatim from `ThinkingContent::signature`, and attach the
//! preserved `item_id` to the `function_call` so Azure's pairing validation
//! passes. When no `item_id` is present we derive a stable `fc_...` id from the
//! call id. The cross-provider / different-model foreign-id remapping from the
//! TS source is not ported (the Rust `AssistantMessage` carries no originating
//! provider/model).

use cortexcode_ai_types::{Content, Context, Message, Model, SimpleStreamOptions, Tool};

const DEFAULT_AZURE_API_VERSION: &str = "v1";

/// Resolve the Azure OpenAI API key from options, then the environment.
pub fn resolve_credentials(options: &SimpleStreamOptions) -> Result<String, String> {
    if let Some(key) = &options.api_key {
        if !key.is_empty() {
            return Ok(key.clone());
        }
    }
    if let Ok(key) = std::env::var("AZURE_OPENAI_API_KEY") {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err("no Azure OpenAI credentials found (set AZURE_OPENAI_API_KEY)".into())
}

/// Resolve the deployment name for a model: an explicit
/// `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` entry (`model1=deployment1,model2=...`)
/// takes precedence, falling back to the model ID.
pub fn resolve_deployment_name(model_id: &str) -> String {
    if let Ok(map_str) = std::env::var("AZURE_OPENAI_DEPLOYMENT_NAME_MAP") {
        for entry in map_str.split(',') {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((id, deployment)) = trimmed.split_once('=') {
                let (id, deployment) = (id.trim(), deployment.trim());
                if !id.is_empty() && !deployment.is_empty() && id == model_id {
                    return deployment.to_string();
                }
            }
        }
    }
    model_id.to_string()
}

fn normalize_azure_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    let is_azure_host =
        trimmed.contains(".openai.azure.com") || trimmed.contains(".cognitiveservices.azure.com");

    if !is_azure_host {
        return trimmed.to_string();
    }

    // Split off any query string before inspecting the path.
    let (path_part, _query) = trimmed.split_once('?').unwrap_or((trimmed, ""));
    let normalized_path_end = path_part.trim_end_matches('/');

    // Find where the path starts after the scheme+host (first '/' after "://").
    let scheme_end = normalized_path_end.find("://").map(|i| i + 3).unwrap_or(0);
    let path_start = normalized_path_end[scheme_end..]
        .find('/')
        .map(|i| scheme_end + i);

    let path = path_start.map(|i| &normalized_path_end[i..]).unwrap_or("");

    if path.is_empty() || path == "/openai" {
        let host = path_start.map(|i| &trimmed[..i]).unwrap_or(trimmed);
        format!("{host}/openai/v1")
    } else {
        normalized_path_end.to_string()
    }
}

fn build_default_base_url(resource_name: &str) -> String {
    format!("https://{resource_name}.openai.azure.com/openai/v1")
}

/// Resolve the Azure OpenAI base URL and API version from the environment,
/// falling back to `model.base_url`.
pub fn resolve_azure_config(model: &Model) -> Result<(String, String), String> {
    let api_version = std::env::var("AZURE_OPENAI_API_VERSION")
        .unwrap_or_else(|_| DEFAULT_AZURE_API_VERSION.to_string());

    let mut resolved = std::env::var("AZURE_OPENAI_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    if resolved.is_none() {
        if let Ok(resource_name) = std::env::var("AZURE_OPENAI_RESOURCE_NAME") {
            if !resource_name.is_empty() {
                resolved = Some(build_default_base_url(&resource_name));
            }
        }
    }
    if resolved.is_none() && !model.base_url.is_empty() {
        resolved = Some(model.base_url.clone());
    }

    let resolved = resolved.ok_or_else(|| {
        "Azure OpenAI base URL is required (set AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME, \
         or provide model.base_url)"
            .to_string()
    })?;

    Ok((normalize_azure_base_url(&resolved), api_version))
}

/// Sanitize an id part to the Responses API's allowed character set, cap it at
/// 64 characters, and strip trailing underscores. Mirrors `normalizeIdPart`
/// from the TS source.
fn normalize_id_part(part: &str) -> String {
    let sanitized: String = part
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let capped = if sanitized.len() > 64 {
        &sanitized[..64]
    } else {
        &sanitized
    };
    capped.trim_end_matches('_').to_string()
}

/// Derive a stable `fc_...` Responses item id from a tool call id, used when
/// the tool call carries no explicit item id.
fn fc_item_id(tool_call_id: &str) -> String {
    format!("fc_{}", cortexcode_ai_util::short_hash(tool_call_id))
}

/// Split a possibly pipe-encoded tool call id into its `(call_id, item_id)`
/// parts. The `item_id` is `None` when the id is not pipe-encoded.
fn split_tool_call_id(id: &str) -> (&str, Option<&str>) {
    match id.split_once('|') {
        Some((call_id, item_id)) => (call_id, Some(item_id)),
        None => (id, None),
    }
}

/// Resolve the Responses `function_call` item id for a tool call: prefer the
/// preserved `item_id` from the pipe-encoding (normalized, forced to start with
/// `fc_`), otherwise derive one from the wire `call_id`.
fn resolve_fc_item_id(call_id: &str, item_id: Option<&str>) -> String {
    match item_id.filter(|s| !s.is_empty()) {
        Some(raw) => {
            let normalized = normalize_id_part(raw);
            if normalized.starts_with("fc_") {
                normalized
            } else {
                normalize_id_part(&format!("fc_{normalized}"))
            }
        }
        None => fc_item_id(call_id),
    }
}

fn convert_messages(model: &Model, context: &Context) -> Vec<serde_json::Value> {
    let mut items = Vec::new();

    if !context.system_prompt.is_empty() {
        let role = if model.reasoning {
            "developer"
        } else {
            "system"
        };
        items.push(serde_json::json!({"role": role, "content": context.system_prompt}));
    }

    for msg in &context.messages {
        match msg {
            Message::User(m) => {
                let content: Vec<serde_json::Value> = m
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text(t) => {
                            Some(serde_json::json!({"type": "input_text", "text": t.text}))
                        }
                        Content::Image(img) => Some(serde_json::json!({
                            "type": "input_image",
                            "detail": "auto",
                            "image_url": format!("data:{};base64,{}", img.media_type, img.data),
                        })),
                        Content::Thinking(_) | Content::ToolCall(_) => None,
                    })
                    .collect();
                if content.is_empty() {
                    continue;
                }
                items.push(serde_json::json!({"role": "user", "content": content}));
            }
            Message::Assistant(m) => {
                for block in &m.content {
                    match block {
                        Content::Text(t) => {
                            if t.text.trim().is_empty() {
                                continue;
                            }
                            items.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "status": "completed",
                                "content": [{"type": "output_text", "text": t.text, "annotations": []}],
                            }));
                        }
                        Content::ToolCall(tc) => {
                            let (call_id, item_id) = split_tool_call_id(&tc.id);
                            items.push(serde_json::json!({
                                "type": "function_call",
                                "id": resolve_fc_item_id(call_id, item_id),
                                "call_id": call_id,
                                "name": tc.name,
                                "arguments": tc.arguments.to_string(),
                            }));
                        }
                        // Replay the paired `reasoning` item verbatim when a
                        // signature is present (Azure pairs each `rs_...`
                        // reasoning item with the `fc_...` function call that
                        // follows it). Signatures that are not valid Responses
                        // items are dropped rather than resent.
                        Content::Thinking(t) => {
                            if let Some(sig) = &t.signature {
                                if let Ok(item) = serde_json::from_str::<serde_json::Value>(sig) {
                                    if item.get("type").and_then(|v| v.as_str())
                                        == Some("reasoning")
                                    {
                                        items.push(item);
                                    }
                                }
                            }
                        }
                        Content::Image(_) => {}
                    }
                }
            }
            Message::ToolResult(m) => {
                let text_result: String = m
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let has_text = !text_result.is_empty();
                let has_images = m.content.iter().any(|c| matches!(c, Content::Image(_)));

                let output = if has_images && model.input.iter().any(|s| s == "image") {
                    let mut parts = Vec::new();
                    if has_text {
                        parts.push(serde_json::json!({"type": "input_text", "text": text_result}));
                    }
                    for c in &m.content {
                        if let Content::Image(img) = c {
                            parts.push(serde_json::json!({
                                "type": "input_image",
                                "detail": "auto",
                                "image_url": format!("data:{};base64,{}", img.media_type, img.data),
                            }));
                        }
                    }
                    serde_json::Value::Array(parts)
                } else {
                    let text = if has_text {
                        text_result
                    } else {
                        "(see attached image)".to_string()
                    };
                    serde_json::json!(text)
                };

                let (call_id, _) = split_tool_call_id(&m.tool_call_id);
                items.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
            }
        }
    }

    items
}

fn convert_tools(tools: &[Tool]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
                "strict": false,
            })
        })
        .collect()
}

/// Build the JSON request body for the Responses API.
pub fn build_request_body(
    model: &Model,
    context: &Context,
    deployment_name: &str,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": deployment_name,
        "input": convert_messages(model, context),
        "stream": true,
        "max_output_tokens": model.max_tokens,
    });

    if !context.tools.is_empty() {
        body["tools"] = serde_json::Value::Array(convert_tools(&context.tools));
    }

    if model.reasoning {
        if let Some(map) = &model.thinking_level_map {
            if let Some(off) = map.get("off") {
                if !off.is_null() {
                    body["reasoning"] = serde_json::json!({"effort": off});
                }
            }
        }
    }

    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::{
        AssistantMessage, TextContent, ThinkingContent, ToolCallContent, ToolResultMessage,
        UserMessage,
    };
    use std::sync::{LazyLock, Mutex};

    /// Serializes tests that mutate `AZURE_OPENAI_*` env vars — they're
    /// process-global, and `cargo test` runs tests in this module in
    /// parallel threads by default.
    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn model(base_url: &str) -> Model {
        Model {
            id: "gpt-5".into(),
            name: "GPT-5".into(),
            api: "azure-openai-responses".into(),
            provider: "azure-openai-responses".into(),
            base_url: base_url.into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: cortexcode_ai_types::ModelCost::default(),
            context_window: 200_000,
            max_tokens: 8192,
            headers: None,
        }
    }

    #[test]
    fn test_resolve_credentials_explicit() {
        let options = SimpleStreamOptions {
            api_key: Some("azkey".into()),
            ..Default::default()
        };
        assert_eq!(resolve_credentials(&options).unwrap(), "azkey");
    }

    #[test]
    fn test_resolve_deployment_name_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("AZURE_OPENAI_DEPLOYMENT_NAME_MAP");
        assert_eq!(resolve_deployment_name("gpt-5"), "gpt-5");
    }

    #[test]
    fn test_resolve_deployment_name_from_map() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "AZURE_OPENAI_DEPLOYMENT_NAME_MAP",
            "gpt-5=my-gpt5-deployment,gpt-4=my-gpt4",
        );
        assert_eq!(resolve_deployment_name("gpt-5"), "my-gpt5-deployment");
        assert_eq!(resolve_deployment_name("other"), "other");
        std::env::remove_var("AZURE_OPENAI_DEPLOYMENT_NAME_MAP");
    }

    #[test]
    fn test_normalize_azure_base_url_adds_openai_v1() {
        assert_eq!(
            normalize_azure_base_url("https://myres.openai.azure.com"),
            "https://myres.openai.azure.com/openai/v1"
        );
        assert_eq!(
            normalize_azure_base_url("https://myres.openai.azure.com/openai"),
            "https://myres.openai.azure.com/openai/v1"
        );
    }

    #[test]
    fn test_normalize_azure_base_url_leaves_custom_path() {
        assert_eq!(
            normalize_azure_base_url("https://myres.openai.azure.com/openai/v1"),
            "https://myres.openai.azure.com/openai/v1"
        );
    }

    #[test]
    fn test_normalize_azure_base_url_leaves_non_azure_host() {
        assert_eq!(
            normalize_azure_base_url("https://my-proxy.example.com/v1"),
            "https://my-proxy.example.com/v1"
        );
    }

    #[test]
    fn test_resolve_azure_config_from_resource_name() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("AZURE_OPENAI_BASE_URL");
        std::env::set_var("AZURE_OPENAI_RESOURCE_NAME", "myres");
        std::env::remove_var("AZURE_OPENAI_API_VERSION");
        let (base_url, api_version) = resolve_azure_config(&model("")).unwrap();
        assert_eq!(base_url, "https://myres.openai.azure.com/openai/v1");
        assert_eq!(api_version, "v1");
        std::env::remove_var("AZURE_OPENAI_RESOURCE_NAME");
    }

    #[test]
    fn test_resolve_azure_config_falls_back_to_model_base_url() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("AZURE_OPENAI_BASE_URL");
        std::env::remove_var("AZURE_OPENAI_RESOURCE_NAME");
        let (base_url, _) =
            resolve_azure_config(&model("https://custom.example.com/openai/v1")).unwrap();
        assert_eq!(base_url, "https://custom.example.com/openai/v1");
    }

    #[test]
    fn test_resolve_azure_config_missing() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("AZURE_OPENAI_BASE_URL");
        std::env::remove_var("AZURE_OPENAI_RESOURCE_NAME");
        assert!(resolve_azure_config(&model("")).is_err());
    }

    #[test]
    fn test_convert_messages_tool_call_and_result() {
        let m = model("https://myres.openai.azure.com");
        let messages = vec![
            Message::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCallContent {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "a.rs"}),
                })],
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                timestamp: None,
                error_message: None,
            }),
            Message::ToolResult(ToolResultMessage {
                content: vec![Content::Text(TextContent {
                    text: "contents".into(),
                    cache_control: None,
                })],
                tool_call_id: "call_1".into(),
                tool_name: "read_file".into(),
                is_error: false,
                timestamp: None,
            }),
        ];
        let ctx = Context::new("".into(), messages, vec![]);
        let items = convert_messages(&m, &ctx);
        assert_eq!(items[0]["type"], "function_call");
        assert_eq!(items[0]["call_id"], "call_1");
        assert!(items[0]["id"].as_str().unwrap().starts_with("fc_"));
        assert_eq!(items[1]["type"], "function_call_output");
        assert_eq!(items[1]["call_id"], "call_1");
        assert_eq!(items[1]["output"], "contents");
    }

    #[test]
    fn test_convert_messages_tool_call_preserves_paired_item_id() {
        // A pipe-encoded `call_id|item_id` from a previous Azure turn must be
        // split back apart: the wire `call_id` goes to `call_id`, and the
        // preserved `fc_...` item id goes to `id` so pairing validation passes.
        let m = model("https://myres.openai.azure.com");
        let messages = vec![
            Message::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCallContent {
                    id: "call_abc|fc_xyz789".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "a.rs"}),
                })],
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                timestamp: None,
                error_message: None,
            }),
            Message::ToolResult(ToolResultMessage {
                content: vec![Content::Text(TextContent {
                    text: "contents".into(),
                    cache_control: None,
                })],
                tool_call_id: "call_abc|fc_xyz789".into(),
                tool_name: "read_file".into(),
                is_error: false,
                timestamp: None,
            }),
        ];
        let ctx = Context::new("".into(), messages, vec![]);
        let items = convert_messages(&m, &ctx);
        assert_eq!(items[0]["type"], "function_call");
        assert_eq!(items[0]["call_id"], "call_abc");
        assert_eq!(items[0]["id"], "fc_xyz789");
        // The tool result strips the item id from the pipe-encoded id too.
        assert_eq!(items[1]["type"], "function_call_output");
        assert_eq!(items[1]["call_id"], "call_abc");
    }

    #[test]
    fn test_convert_messages_forces_fc_prefix_on_item_id() {
        let m = model("https://myres.openai.azure.com");
        let messages = vec![Message::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCallContent {
                id: "call_1|rs_weird".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({}),
            })],
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            timestamp: None,
            error_message: None,
        })];
        let ctx = Context::new("".into(), messages, vec![]);
        let items = convert_messages(&m, &ctx);
        // An item id that does not already start with `fc_` gets the prefix.
        assert_eq!(items[0]["id"], "fc_rs_weird");
    }

    #[test]
    fn test_convert_messages_replays_reasoning_signature() {
        let m = model("https://myres.openai.azure.com");
        let reasoning_item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_123",
            "summary": [{"type": "summary_text", "text": "thinking"}]
        });
        let messages = vec![Message::Assistant(AssistantMessage {
            content: vec![
                Content::Thinking(ThinkingContent {
                    thinking: "thinking".into(),
                    signature: Some(reasoning_item.to_string()),
                }),
                Content::ToolCall(ToolCallContent {
                    id: "call_1|fc_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({}),
                }),
            ],
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            timestamp: None,
            error_message: None,
        })];
        let ctx = Context::new("".into(), messages, vec![]);
        let items = convert_messages(&m, &ctx);
        // The reasoning item is replayed verbatim, ahead of its function call.
        assert_eq!(items[0]["type"], "reasoning");
        assert_eq!(items[0]["id"], "rs_123");
        assert_eq!(items[1]["type"], "function_call");
        assert_eq!(items[1]["id"], "fc_1");
    }

    #[test]
    fn test_convert_messages_drops_non_reasoning_signature() {
        let m = model("https://myres.openai.azure.com");
        let messages = vec![Message::Assistant(AssistantMessage {
            content: vec![Content::Thinking(ThinkingContent {
                thinking: "thinking".into(),
                // Not a Responses reasoning item — must not be resent.
                signature: Some("not json".into()),
            })],
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            timestamp: None,
            error_message: None,
        })];
        let ctx = Context::new("".into(), messages, vec![]);
        let items = convert_messages(&m, &ctx);
        assert!(items.is_empty());
    }

    #[test]
    fn test_normalize_id_part_sanitizes_and_caps() {
        assert_eq!(normalize_id_part("abc!def"), "abc_def");
        assert_eq!(normalize_id_part("trail___"), "trail");
        assert_eq!(normalize_id_part(&"x".repeat(80)).len(), 64);
    }

    #[test]
    fn test_convert_messages_user_text() {
        let m = model("https://myres.openai.azure.com");
        let ctx = Context::new(
            "".into(),
            vec![Message::User(UserMessage {
                content: vec![Content::Text(TextContent {
                    text: "hi".into(),
                    cache_control: None,
                })],
                timestamp: None,
            })],
            vec![],
        );
        let items = convert_messages(&m, &ctx);
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[0]["content"][0]["type"], "input_text");
    }

    #[test]
    fn test_build_request_body_basic() {
        let m = model("https://myres.openai.azure.com");
        let ctx = Context::new("be nice".into(), vec![], vec![]);
        let body = build_request_body(&m, &ctx, "my-deployment");
        assert_eq!(body["model"], "my-deployment");
        assert_eq!(body["input"][0]["role"], "system");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn test_build_request_body_developer_role_when_reasoning() {
        let mut m = model("https://myres.openai.azure.com");
        m.reasoning = true;
        let ctx = Context::new("be nice".into(), vec![], vec![]);
        let body = build_request_body(&m, &ctx, "my-deployment");
        assert_eq!(body["input"][0]["role"], "developer");
    }
}
