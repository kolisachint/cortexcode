//! Shared OpenAI Responses API plumbing: message and tool conversion, the
//! stream-event processor and the HTTP driver used by the `openai-responses`
//! and `azure-openai-responses` providers.
//!
//! Port of hoocode `providers/openai-responses-shared.ts` (v0.5.89).

use std::collections::HashSet;

use cortexcode_ai_stream::{
    create_assistant_message_event_stream, spawn_producer, AssistantMessageEventStream,
};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, AssistantMessageEvent, Content, Context, Message, Model,
    StopReason, TextContent, ThinkingContent, Tool, ToolCallContent, Usage,
};
use cortexcode_ai_util::{
    describe_provider_error, openai_api_error_message, parse_streaming_json,
    post_json_with_sdk_retries, response_headers, short_hash, to_strict_json_schema,
    transform_messages,
};
use futures_util::StreamExt;
use serde_json::{json, Map, Value};

// =============================================================================
// Utilities
// =============================================================================

/// `encodeTextSignatureV1`: `{"v":1,"id":...,"phase"?:...}`.
pub fn encode_text_signature_v1(id: &str, phase: Option<&str>) -> String {
    let mut payload = Map::new();
    payload.insert("v".into(), json!(1));
    payload.insert("id".into(), json!(id));
    if let Some(phase) = phase {
        payload.insert("phase".into(), json!(phase));
    }
    Value::Object(payload).to_string()
}

/// `parseTextSignature`: a V1 JSON signature, else the legacy plain id.
pub fn parse_text_signature(signature: Option<&str>) -> Option<(String, Option<String>)> {
    let signature = signature.filter(|s| !s.is_empty())?;
    if signature.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<Value>(signature) {
            if parsed["v"] == 1 {
                if let Some(id) = parsed["id"].as_str() {
                    let phase = parsed["phase"]
                        .as_str()
                        .filter(|p| *p == "commentary" || *p == "final_answer")
                        .map(str::to_string);
                    return Some((id.to_string(), phase));
                }
            }
        }
    }
    Some((signature.to_string(), None))
}

fn sanitize_id(part: &str) -> String {
    part.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `normalizeIdPart`: allowed characters only, at most 64, no trailing `_`.
fn normalize_id_part(part: &str) -> String {
    let sanitized: String = sanitize_id(part).chars().take(64).collect();
    sanitized.trim_end_matches('_').to_string()
}

/// `buildForeignResponsesItemId`: a bounded `fc_<hash>` for another
/// provider's item id.
fn build_foreign_responses_item_id(item_id: &str) -> String {
    format!("fc_{}", short_hash(item_id))
        .chars()
        .take(64)
        .collect()
}

// =============================================================================
// Message conversion
// =============================================================================

fn split_pipe(id: &str) -> (&str, Option<&str>) {
    let mut parts = id.split('|');
    let first = parts.next().unwrap_or_default();
    (first, parts.next())
}

/// `convertResponsesMessages`: context messages to Responses API input items.
/// `allowed_tool_call_providers` are the providers whose `call_id|item_id`
/// tool call ids are kept as pairs.
pub fn convert_responses_messages(
    model: &Model,
    context: &Context,
    allowed_tool_call_providers: &HashSet<&str>,
    include_system_prompt: bool,
) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();

    let normalize = |id: &str, target: &Model, source: &AssistantMessage| -> String {
        if !allowed_tool_call_providers.contains(target.provider.as_str()) || !id.contains('|') {
            return normalize_id_part(id);
        }
        let (call_id, item_id) = split_pipe(id);
        let item_id = item_id.unwrap_or_default();
        let normalized_call_id = normalize_id_part(call_id);
        let is_foreign = source.provider != target.provider || source.api != target.api;
        let mut normalized_item_id = if is_foreign {
            build_foreign_responses_item_id(item_id)
        } else {
            normalize_id_part(item_id)
        };
        // The Responses API requires item ids to start with "fc".
        if !normalized_item_id.starts_with("fc_") {
            normalized_item_id = normalize_id_part(&format!("fc_{normalized_item_id}"));
        }
        format!("{normalized_call_id}|{normalized_item_id}")
    };
    let transformed = transform_messages(&context.messages, model, Some(&normalize));

    if include_system_prompt && !context.system_prompt.is_empty() {
        let role = if model.reasoning {
            "developer"
        } else {
            "system"
        };
        messages.push(json!({"role": role, "content": context.system_prompt}));
    }

    let mut msg_index = 0usize;
    for msg in &transformed {
        match msg {
            Message::User(m) => {
                let content: Vec<Value> = m
                    .content
                    .iter()
                    .filter_map(|item| match item {
                        Content::Text(t) => Some(json!({"type": "input_text", "text": t.text})),
                        Content::Image(img) => Some(json!({
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
                messages.push(json!({"role": "user", "content": content}));
            }
            Message::Assistant(a) => {
                let is_different_model =
                    a.model != model.id && a.provider == model.provider && a.api == model.api;
                let mut output: Vec<Value> = Vec::new();
                for block in &a.content {
                    match block {
                        Content::Thinking(t) => {
                            // The signature is the reasoning item, replayed verbatim.
                            if let Some(item) = t
                                .signature
                                .as_deref()
                                .filter(|s| !s.is_empty())
                                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            {
                                output.push(item);
                            }
                        }
                        Content::Text(t) => {
                            let parsed = parse_text_signature(t.text_signature.as_deref());
                            let msg_id = match parsed.as_ref().map(|(id, _)| id.as_str()) {
                                None | Some("") => format!("msg_{msg_index}"),
                                // OpenAI caps ids at 64 characters.
                                Some(id) if id.chars().count() > 64 => {
                                    format!("msg_{}", short_hash(id))
                                }
                                Some(id) => id.to_string(),
                            };
                            let mut item = json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{"type": "output_text", "text": t.text, "annotations": []}],
                                "status": "completed",
                                "id": msg_id,
                            });
                            if let Some(phase) = parsed.and_then(|(_, phase)| phase) {
                                item["phase"] = json!(phase);
                            }
                            output.push(item);
                        }
                        Content::ToolCall(tc) => {
                            let (call_id, item_id) = split_pipe(&tc.id);
                            // A different model's fc_ ids would trip OpenAI's
                            // reasoning-item pairing validation; omit them.
                            let item_id =
                                item_id.filter(|id| !(is_different_model && id.starts_with("fc_")));
                            let mut item = Map::new();
                            item.insert("type".into(), json!("function_call"));
                            if let Some(id) = item_id {
                                item.insert("id".into(), json!(id));
                            }
                            item.insert("call_id".into(), json!(call_id));
                            item.insert("name".into(), json!(tc.name));
                            item.insert("arguments".into(), json!(tc.arguments.to_string()));
                            output.push(Value::Object(item));
                        }
                        Content::Image(_) => {}
                    }
                }
                if output.is_empty() {
                    continue;
                }
                messages.extend(output);
            }
            Message::ToolResult(r) => {
                let text = r
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let has_images = r.content.iter().any(|c| matches!(c, Content::Image(_)));
                let (call_id, _) = split_pipe(&r.tool_call_id);
                let output = if has_images && model.input.iter().any(|i| i == "image") {
                    let mut parts = Vec::new();
                    if !text.is_empty() {
                        parts.push(json!({"type": "input_text", "text": text}));
                    }
                    for c in &r.content {
                        if let Content::Image(img) = c {
                            parts.push(json!({
                                "type": "input_image",
                                "detail": "auto",
                                "image_url": format!("data:{};base64,{}", img.media_type, img.data),
                            }));
                        }
                    }
                    Value::Array(parts)
                } else if text.is_empty() {
                    json!("(see attached image)")
                } else {
                    json!(text)
                };
                messages.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
            }
        }
        msg_index += 1;
    }
    messages
}

// =============================================================================
// Tool conversion
// =============================================================================

/// `convertResponsesTools`. `constrain_tool_calls` sends `strict: true` with
/// closed schemas; otherwise `strict` is the given value (default false).
pub fn convert_responses_tools(
    tools: &[Tool],
    strict: Option<Value>,
    constrain_tool_calls: bool,
) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            if constrain_tool_calls {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": to_strict_json_schema(&tool.parameters),
                    "strict": true,
                })
            } else {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                    "strict": strict.clone().unwrap_or(json!(false)),
                })
            }
        })
        .collect()
}

// =============================================================================
// Stream processing
// =============================================================================

/// `applyServiceTierPricing(usage, serviceTier)` for the resolved tier.
pub type ServiceTierPricing = fn(&mut Usage, Option<&str>, &Model);

/// `OpenAIResponsesStreamOptions`.
#[derive(Clone, Default)]
pub struct ResponsesStreamOptions {
    pub service_tier: Option<String>,
    pub apply_service_tier_pricing: Option<ServiceTierPricing>,
}

/// State of `processResponsesStream`: the message being built, the current
/// output item (kept up to date with summary/content parts) and block.
pub struct ResponsesStreamState {
    pub output: AssistantMessage,
    current_item: Option<Value>,
    /// Content index of the open block.
    current_block: Option<usize>,
    /// `partialJson` of the open tool-call block.
    partial_json: String,
}

impl ResponsesStreamState {
    pub fn new(model: &Model) -> Self {
        let mut output = AssistantMessage::for_model(model);
        output.stop_reason = StopReason::Stop;
        Self {
            output,
            current_item: None,
            current_block: None,
            partial_json: String::new(),
        }
    }

    fn last_index(&self) -> usize {
        self.output.content.len().saturating_sub(1)
    }

    fn item_type(&self) -> Option<&str> {
        self.current_item.as_ref().and_then(|i| i["type"].as_str())
    }

    fn block(&mut self) -> Option<&mut Content> {
        self.current_block
            .and_then(move |i| self.output.content.get_mut(i))
    }

    fn block_is(&self, kind: &str) -> bool {
        match self.current_block.and_then(|i| self.output.content.get(i)) {
            Some(Content::Thinking(_)) => kind == "thinking",
            Some(Content::Text(_)) => kind == "text",
            Some(Content::ToolCall(_)) => kind == "toolCall",
            _ => false,
        }
    }

    fn push_thinking_delta(&mut self, delta: &str, sender: &AssistantMessageEventStream) {
        if let Some(Content::Thinking(t)) = self.block() {
            t.thinking.push_str(delta);
        }
        sender.push(AssistantMessageEvent::ThinkingDelta {
            index: self.last_index(),
            delta: delta.to_string(),
            partial: self.output.clone(),
        });
    }

    /// Append to the last summary part of the current reasoning item.
    fn append_summary(&mut self, delta: &str) -> bool {
        let Some(summary) = self
            .current_item
            .as_mut()
            .and_then(|i| i.get_mut("summary"))
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        let Some(last) = summary.last_mut() else {
            return false;
        };
        let text = last["text"].as_str().unwrap_or_default().to_string();
        last["text"] = json!(format!("{text}{delta}"));
        true
    }

    /// Append to the last content part of the current message item when it
    /// has type `kind`, under `field` (`text` or `refusal`).
    fn append_content(&mut self, kind: &str, field: &str, delta: &str) -> bool {
        let Some(parts) = self
            .current_item
            .as_mut()
            .and_then(|i| i.get_mut("content"))
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        let Some(last) = parts.last_mut() else {
            return false;
        };
        if last["type"] != kind {
            return false;
        }
        let text = last[field].as_str().unwrap_or_default().to_string();
        last[field] = json!(format!("{text}{delta}"));
        true
    }

    /// Handle one stream event. `Err` is a thrown error (`error` and
    /// `response.failed` events).
    pub fn handle_event(
        &mut self,
        event: &Value,
        model: &Model,
        options: &ResponsesStreamOptions,
        sender: &AssistantMessageEventStream,
    ) -> Result<(), String> {
        let kind = event["type"].as_str().unwrap_or_default();
        match kind {
            "response.created" => {
                if let Some(id) = event["response"]["id"].as_str() {
                    self.output.response_id = Some(id.to_string());
                }
            }
            "response.output_item.added" => {
                let item = &event["item"];
                match item["type"].as_str() {
                    Some("reasoning") => {
                        self.current_item = Some(item.clone());
                        self.output.content.push(Content::Thinking(ThinkingContent {
                            thinking: String::new(),
                            signature: None,
                            redacted: false,
                        }));
                        self.current_block = Some(self.last_index());
                        sender.push(AssistantMessageEvent::ThinkingStart {
                            index: self.last_index(),
                            partial: self.output.clone(),
                        });
                    }
                    Some("message") => {
                        self.current_item = Some(item.clone());
                        self.output
                            .content
                            .push(Content::Text(TextContent::new("")));
                        self.current_block = Some(self.last_index());
                        sender.push(AssistantMessageEvent::TextStart {
                            index: self.last_index(),
                            partial: self.output.clone(),
                        });
                    }
                    Some("function_call") => {
                        self.current_item = Some(item.clone());
                        self.output.content.push(Content::ToolCall(ToolCallContent {
                            id: format!(
                                "{}|{}",
                                item["call_id"].as_str().unwrap_or_default(),
                                item["id"].as_str().unwrap_or_default()
                            ),
                            name: item["name"].as_str().unwrap_or_default().to_string(),
                            arguments: json!({}),
                            thought_signature: None,
                        }));
                        self.partial_json =
                            item["arguments"].as_str().unwrap_or_default().to_string();
                        self.current_block = Some(self.last_index());
                        sender.push(AssistantMessageEvent::ToolCallStart {
                            index: self.last_index(),
                            partial: self.output.clone(),
                        });
                    }
                    _ => {}
                }
            }
            "response.reasoning_summary_part.added" => {
                if let Some(item) = self.current_item.as_mut() {
                    if item["type"] == "reasoning" {
                        if !item["summary"].is_array() {
                            item["summary"] = json!([]);
                        }
                        if let Some(summary) = item["summary"].as_array_mut() {
                            summary.push(event["part"].clone());
                        }
                    }
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_summary_part.done" => {
                if self.item_type() == Some("reasoning") && self.block_is("thinking") {
                    let delta = if kind == "response.reasoning_summary_part.done" {
                        "\n\n".to_string()
                    } else {
                        event["delta"].as_str().unwrap_or_default().to_string()
                    };
                    if self.append_summary(&delta) {
                        self.push_thinking_delta(&delta, sender);
                    }
                }
            }
            "response.reasoning_text.delta" => {
                if self.item_type() == Some("reasoning") && self.block_is("thinking") {
                    let delta = event["delta"].as_str().unwrap_or_default().to_string();
                    self.push_thinking_delta(&delta, sender);
                }
            }
            "response.content_part.added" => {
                if let Some(item) = self.current_item.as_mut() {
                    if item["type"] == "message" {
                        if !item["content"].is_array() {
                            item["content"] = json!([]);
                        }
                        // Only output_text and refusal parts (not reasoning text).
                        let part = &event["part"];
                        if part["type"] == "output_text" || part["type"] == "refusal" {
                            if let Some(content) = item["content"].as_array_mut() {
                                content.push(part.clone());
                            }
                        }
                    }
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                if self.item_type() == Some("message") && self.block_is("text") {
                    let delta = event["delta"].as_str().unwrap_or_default().to_string();
                    let (part_kind, field) = if kind == "response.output_text.delta" {
                        ("output_text", "text")
                    } else {
                        ("refusal", "refusal")
                    };
                    if self.append_content(part_kind, field, &delta) {
                        if let Some(Content::Text(t)) = self.block() {
                            t.text.push_str(&delta);
                        }
                        sender.push(AssistantMessageEvent::TextDelta {
                            index: self.last_index(),
                            delta,
                            partial: self.output.clone(),
                        });
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                if self.item_type() == Some("function_call") && self.block_is("toolCall") {
                    let delta = event["delta"].as_str().unwrap_or_default();
                    self.partial_json.push_str(delta);
                    let args = parse_arguments(&self.partial_json);
                    if let Some(Content::ToolCall(tc)) = self.block() {
                        tc.arguments = args;
                    }
                    sender.push(AssistantMessageEvent::ToolCallDelta {
                        index: self.last_index(),
                        delta: delta.to_string(),
                        partial: self.output.clone(),
                    });
                }
            }
            "response.function_call_arguments.done" => {
                if self.item_type() == Some("function_call") && self.block_is("toolCall") {
                    let arguments = event["arguments"].as_str().unwrap_or_default().to_string();
                    let previous = std::mem::replace(&mut self.partial_json, arguments.clone());
                    let args = parse_arguments(&self.partial_json);
                    if let Some(Content::ToolCall(tc)) = self.block() {
                        tc.arguments = args;
                    }
                    if let Some(delta) = arguments.strip_prefix(previous.as_str()) {
                        if !delta.is_empty() {
                            sender.push(AssistantMessageEvent::ToolCallDelta {
                                index: self.last_index(),
                                delta: delta.to_string(),
                                partial: self.output.clone(),
                            });
                        }
                    }
                }
            }
            "response.output_item.done" => {
                let item = &event["item"];
                match item["type"].as_str() {
                    Some("reasoning") if self.block_is("thinking") => {
                        let join = |key: &str| {
                            item[key]
                                .as_array()
                                .map(|parts| {
                                    parts
                                        .iter()
                                        .map(|p| p["text"].as_str().unwrap_or_default())
                                        .collect::<Vec<_>>()
                                        .join("\n\n")
                                })
                                .unwrap_or_default()
                        };
                        let summary = join("summary");
                        let content = join("content");
                        let signature = item.to_string();
                        if let Some(Content::Thinking(t)) = self.block() {
                            if !summary.is_empty() {
                                t.thinking = summary;
                            } else if !content.is_empty() {
                                t.thinking = content;
                            }
                            t.signature = Some(signature);
                        }
                        sender.push(AssistantMessageEvent::ThinkingEnd {
                            index: self.last_index(),
                            partial: self.output.clone(),
                        });
                        self.current_block = None;
                    }
                    Some("message") if self.block_is("text") => {
                        let text: String = item["content"]
                            .as_array()
                            .map(|parts| {
                                parts
                                    .iter()
                                    .map(|p| {
                                        if p["type"] == "output_text" {
                                            p["text"].as_str().unwrap_or_default()
                                        } else {
                                            p["refusal"].as_str().unwrap_or_default()
                                        }
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let signature = encode_text_signature_v1(
                            item["id"].as_str().unwrap_or_default(),
                            item["phase"].as_str(),
                        );
                        if let Some(Content::Text(t)) = self.block() {
                            t.text = text;
                            t.text_signature = Some(signature);
                        }
                        sender.push(AssistantMessageEvent::TextEnd {
                            index: self.last_index(),
                            partial: self.output.clone(),
                        });
                        self.current_block = None;
                    }
                    Some("function_call") => {
                        if self.block_is("toolCall") {
                            let args = if self.partial_json.is_empty() {
                                parse_arguments(item["arguments"].as_str().unwrap_or("{}"))
                            } else {
                                parse_arguments(&self.partial_json)
                            };
                            if let Some(Content::ToolCall(tc)) = self.block() {
                                tc.arguments = args;
                            }
                        }
                        self.partial_json.clear();
                        self.current_block = None;
                        sender.push(AssistantMessageEvent::ToolCallEnd {
                            index: self.last_index(),
                            partial: self.output.clone(),
                        });
                    }
                    _ => {}
                }
            }
            "response.completed" => {
                let response = &event["response"];
                if let Some(id) = response["id"].as_str().filter(|s| !s.is_empty()) {
                    self.output.response_id = Some(id.to_string());
                }
                if let Some(usage) = response.get("usage").filter(|u| u.is_object()) {
                    let cached = usage["input_tokens_details"]["cached_tokens"]
                        .as_u64()
                        .unwrap_or(0);
                    // input_tokens includes cached tokens.
                    self.output.usage = Usage {
                        input: usage["input_tokens"]
                            .as_u64()
                            .unwrap_or(0)
                            .saturating_sub(cached),
                        output: usage["output_tokens"].as_u64().unwrap_or(0),
                        cache_read: cached,
                        cache_write: 0,
                        total_tokens: usage["total_tokens"].as_u64().unwrap_or(0),
                        cost: Default::default(),
                    };
                }
                self.output.usage.cost =
                    cortexcode_ai_models::calculate_cost(model, &self.output.usage);
                if let Some(apply) = options.apply_service_tier_pricing {
                    let tier = response["service_tier"]
                        .as_str()
                        .or(options.service_tier.as_deref());
                    apply(&mut self.output.usage, tier, model);
                }
                self.output.stop_reason = map_stop_reason(response["status"].as_str());
                if self.output.stop_reason == StopReason::Stop
                    && self
                        .output
                        .content
                        .iter()
                        .any(|b| matches!(b, Content::ToolCall(_)))
                {
                    self.output.stop_reason = StopReason::ToolUse;
                }
            }
            "error" => {
                return Err(format!(
                    "Error Code {}: {}",
                    js_string(&event["code"]),
                    js_string(&event["message"])
                ));
            }
            "response.failed" => {
                let response = &event["response"];
                let error = response.get("error").filter(|e| e.is_object());
                let reason = response["incomplete_details"]["reason"]
                    .as_str()
                    .filter(|r| !r.is_empty());
                let msg = match (error, reason) {
                    (Some(e), _) => format!(
                        "{}: {}",
                        e["code"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("unknown"),
                        e["message"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("no message")
                    ),
                    (None, Some(reason)) => format!("incomplete: {reason}"),
                    (None, None) => "Unknown error (no error details in response)".to_string(),
                };
                return Err(msg);
            }
            _ => {}
        }
        Ok(())
    }
}

/// `${value}` in a JS template literal.
fn js_string(v: &Value) -> String {
    match v {
        Value::Null => "undefined".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn parse_arguments(raw: &str) -> Value {
    match parse_streaming_json::<Value>(Some(raw)) {
        Value::Null => json!({}),
        v => v,
    }
}

/// `mapStopReason` over `ResponseStatus`.
pub fn map_stop_reason(status: Option<&str>) -> StopReason {
    match status {
        Some("incomplete") => StopReason::Length,
        Some("failed") | Some("cancelled") => StopReason::Error,
        _ => StopReason::Stop,
    }
}

// =============================================================================
// HTTP driver
// =============================================================================

/// A resolved Responses API request.
pub struct ResponsesRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
    pub timeout_ms: Option<u64>,
    /// SDK client retries (default 2).
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
}

/// Run a Responses API request as the TS providers' `stream*` functions do:
/// `start` once the response arrives, the processed events, then `done`, or
/// an `error` event for any failure (including one from building the request,
/// which `request` defers so it surfaces on the stream).
pub fn run_responses_stream(
    model: Model,
    request: Result<ResponsesRequest, String>,
    signal: Option<AbortSignal>,
    options: ResponsesStreamOptions,
) -> AssistantMessageEventStream {
    let stream = create_assistant_message_event_stream();
    let sender = stream.clone();
    spawn_producer(&stream, async move {
        let mut state = ResponsesStreamState::new(&model);
        let outcome = match (&signal, request) {
            (_, Err(message)) => Err(message),
            (Some(signal), Ok(request)) => tokio::select! {
                biased;
                _ = signal.cancelled() => Err("Request was aborted".to_string()),
                r = drive(&model, &request, &options, &mut state, &sender) => r,
            },
            (None, Ok(request)) => drive(&model, &request, &options, &mut state, &sender).await,
        };
        let outcome = outcome.and_then(|()| {
            if signal.as_ref().is_some_and(AbortSignal::aborted) {
                return Err("Request was aborted".to_string());
            }
            if matches!(
                state.output.stop_reason,
                StopReason::Aborted | StopReason::Error
            ) {
                return Err("An unknown error occurred".to_string());
            }
            Ok(())
        });
        let mut output = state.output;
        match outcome {
            Ok(()) => {
                sender.push(AssistantMessageEvent::Done {
                    message: output.clone(),
                });
                sender.end(Some(output));
            }
            Err(message) => {
                output.stop_reason = if signal.as_ref().is_some_and(AbortSignal::aborted) {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                };
                output.error_message = Some(message);
                sender.push(AssistantMessageEvent::Error {
                    error: output.clone(),
                });
                sender.end(Some(output));
            }
        }
    });
    stream
}

async fn drive(
    model: &Model,
    request: &ResponsesRequest,
    options: &ResponsesStreamOptions,
    state: &mut ResponsesStreamState,
    sender: &AssistantMessageEventStream,
) -> Result<(), String> {
    let mut builder = reqwest::Client::builder();
    if let Some(ms) = request.timeout_ms {
        builder = builder.timeout(std::time::Duration::from_millis(ms));
    }
    let client = builder
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let response = post_json_with_sdk_retries(
        &client,
        &request.url,
        &request.headers,
        &request.body,
        request.max_retries,
        request.max_retry_delay_ms,
    )
    .await
    .map_err(|failure| failure.message().to_string())?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let headers = response_headers(&response);
        let body = response.text().await.unwrap_or_default();
        return Err(describe_provider_error(
            &openai_api_error_message(status, &body),
            Some(&headers),
            request.max_retry_delay_ms,
        ));
    }
    sender.push(AssistantMessageEvent::Start {
        partial: state.output.clone(),
    });

    let mut events = cortexcode_ai_sse::sse_events(response.bytes_stream());
    while let Some(frame) = events.next().await {
        let frame = frame.map_err(|e| format!("error reading response stream: {e}"))?;
        let payload = frame.data.trim();
        if payload.is_empty() {
            continue;
        }
        if payload == "[DONE]" {
            break;
        }
        let Ok(event) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        state.handle_event(&event, model, options, sender)?;
    }
    Ok(())
}

/// Set `name` in an ordered header list, replacing a case-insensitive match
/// (later sources win, as with `Object.assign` over a record).
pub fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some(slot) = headers
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
    {
        *slot = (name.to_string(), value.to_string());
    } else {
        headers.push((name.to_string(), value.to_string()));
    }
}

#[cfg(test)]
#[path = "shared_tests.rs"]
pub(crate) mod tests;
