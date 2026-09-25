//! Port of hoocode `packages/ai/test/faux-provider.test.ts` (v0.5.89).
//!
//! `complete`/`stream` are the API-registry entry points
//! (`complete_simple`/`stream_simple`); every test registers its own faux
//! provider under a random api, as in TypeScript.

use cortexcode_ai_provider_faux::{
    faux_assistant_message, faux_text, faux_thinking, faux_tool_call, register_faux_provider,
    FauxMessageOptions, FauxModelDefinition, FauxProviderRegistration, FauxResponseStep,
    FauxTokenSize, RegisterFauxProviderOptions,
};
use cortexcode_ai_registry::{complete_simple, stream_simple};
use cortexcode_ai_types::{
    now_ms, AbortSignal, AssistantMessage, AssistantMessageEvent, CacheRetention, Content, Context,
    ImageContent, Message, Model, SimpleStreamOptions, StopReason, Tool, ToolResultMessage,
    UserMessage,
};
use futures_util::StreamExt;
use serde_json::json;

/// Unregisters on drop (`afterEach(() => registration.unregister())`).
struct Registered(FauxProviderRegistration);

impl Drop for Registered {
    fn drop(&mut self) {
        self.0.unregister();
    }
}

impl std::ops::Deref for Registered {
    type Target = FauxProviderRegistration;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn register(options: RegisterFauxProviderOptions) -> Registered {
    Registered(register_faux_provider(options))
}

fn user(text: &str) -> Message {
    user_at(text, now_ms())
}

fn user_at(text: &str, timestamp: i64) -> Message {
    Message::User(UserMessage {
        content: vec![faux_text(text)].into(),
        timestamp,
    })
}

fn hi() -> Context {
    Context::new(String::new(), vec![user("hi")], vec![])
}

fn msg(content: &str) -> FauxResponseStep {
    faux_assistant_message(content, FauxMessageOptions::default()).into()
}

fn tool_use() -> FauxMessageOptions {
    FauxMessageOptions {
        stop_reason: Some(StopReason::ToolUse),
        ..Default::default()
    }
}

fn sized(min: usize, max: usize, tokens_per_second: Option<f64>) -> RegisterFauxProviderOptions {
    RegisterFauxProviderOptions {
        tokens_per_second,
        token_size: Some(FauxTokenSize {
            min: Some(min),
            max: Some(max),
        }),
        ..Default::default()
    }
}

async fn complete(model: Model, context: Context) -> AssistantMessage {
    complete_with(model, context, SimpleStreamOptions::default()).await
}

async fn complete_with(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> AssistantMessage {
    complete_simple(model, context, options).await.unwrap()
}

async fn collect_events(model: Model, context: Context) -> Vec<AssistantMessageEvent> {
    stream_simple(model, context, SimpleStreamOptions::default())
        .unwrap()
        .collect()
        .await
}

fn event_type(event: &AssistantMessageEvent) -> &'static str {
    match event {
        AssistantMessageEvent::Start { .. } => "start",
        AssistantMessageEvent::TextStart { .. } => "text_start",
        AssistantMessageEvent::TextDelta { .. } => "text_delta",
        AssistantMessageEvent::TextEnd { .. } => "text_end",
        AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
        AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
        AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
        AssistantMessageEvent::ToolCallStart { .. } => "toolcall_start",
        AssistantMessageEvent::ToolCallDelta { .. } => "toolcall_delta",
        AssistantMessageEvent::ToolCallEnd { .. } => "toolcall_end",
        AssistantMessageEvent::Done { .. } => "done",
        AssistantMessageEvent::Error { .. } => "error",
    }
}

fn types(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
    events.iter().map(event_type).collect()
}

fn content_json(message: &AssistantMessage) -> serde_json::Value {
    serde_json::to_value(&message.content).unwrap()
}

#[tokio::test]
async fn registers_a_custom_provider_and_estimates_usage() {
    let registration = register(Default::default());
    registration.set_responses(vec![msg("hello world")]);

    let context = Context::new("Be concise.".into(), vec![user("hi there")], vec![]);
    let response = complete(registration.get_model(), context).await;
    assert_eq!(
        content_json(&response),
        json!([{"type": "text", "text": "hello world"}])
    );
    assert!(response.usage.input > 0);
    assert!(response.usage.output > 0);
    assert_eq!(
        response.usage.total_tokens,
        response.usage.input + response.usage.output
    );
    assert_eq!(registration.state().call_count(), 1);
}

#[tokio::test]
async fn supports_helper_blocks_for_text_thinking_and_tool_calls() {
    let registration = register(Default::default());
    registration.set_responses(vec![faux_assistant_message(
        vec![
            faux_thinking("think"),
            faux_tool_call("echo", json!({"text": "hi"}), None),
            faux_text("done"),
        ],
        tool_use(),
    )
    .into()]);

    let response = complete(registration.get_model(), hi()).await;
    let content = content_json(&response);
    assert!(content[1]["id"].is_string());
    assert_eq!(
        content,
        json!([
            {"type": "thinking", "thinking": "think"},
            {"type": "toolCall", "id": content[1]["id"], "name": "echo", "arguments": {"text": "hi"}},
            {"type": "text", "text": "done"},
        ])
    );
    assert_eq!(response.stop_reason, StopReason::ToolUse);
}

#[tokio::test]
async fn supports_multiple_models_with_per_model_reasoning_and_model_aware_factories() {
    let registration = register(RegisterFauxProviderOptions {
        models: vec![
            FauxModelDefinition {
                name: Some("Faux Fast".into()),
                reasoning: Some(false),
                ..FauxModelDefinition::new("faux-fast")
            },
            FauxModelDefinition {
                name: Some("Faux Thinker".into()),
                reasoning: Some(true),
                ..FauxModelDefinition::new("faux-thinker")
            },
        ],
        ..Default::default()
    });
    let factory = || {
        FauxResponseStep::factory(|_context, _options, _state, model| {
            Ok(faux_assistant_message(
                format!("{}:{}", model.id, model.reasoning),
                Default::default(),
            ))
        })
    };
    registration.set_responses(vec![factory(), factory()]);

    let ids: Vec<&str> = registration
        .models()
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(ids, ["faux-fast", "faux-thinker"]);
    assert_eq!(registration.get_model(), registration.models()[0]);
    assert!(!registration.get_model_by_id("faux-fast").unwrap().reasoning);
    assert!(
        registration
            .get_model_by_id("faux-thinker")
            .unwrap()
            .reasoning
    );

    let fast = complete(registration.get_model_by_id("faux-fast").unwrap(), hi()).await;
    let thinker = complete(registration.get_model_by_id("faux-thinker").unwrap(), hi()).await;
    assert_eq!(
        content_json(&fast),
        json!([{"type": "text", "text": "faux-fast:false"}])
    );
    assert_eq!(
        content_json(&thinker),
        json!([{"type": "text", "text": "faux-thinker:true"}])
    );
}

#[tokio::test]
async fn rewrites_api_provider_and_model_on_returned_messages() {
    let registration = register(RegisterFauxProviderOptions {
        api: Some("faux:test".into()),
        provider: Some("faux-provider".into()),
        models: vec![FauxModelDefinition::new("faux-model")],
        ..Default::default()
    });
    registration.set_responses(vec![msg("hello")]);

    let response = complete(registration.get_model(), hi()).await;
    assert_eq!(response.api, "faux:test");
    assert_eq!(response.provider, "faux-provider");
    assert_eq!(response.model, "faux-model");
}

#[tokio::test]
async fn consumes_queued_responses_in_order_and_errors_when_exhausted() {
    let registration = register(Default::default());
    registration.set_responses(vec![msg("first"), msg("second")]);

    let first = complete(registration.get_model(), hi()).await;
    let second = complete(registration.get_model(), hi()).await;
    let exhausted = complete(registration.get_model(), hi()).await;

    assert_eq!(
        content_json(&first),
        json!([{"type": "text", "text": "first"}])
    );
    assert_eq!(
        content_json(&second),
        json!([{"type": "text", "text": "second"}])
    );
    assert_eq!(exhausted.stop_reason, StopReason::Error);
    assert_eq!(
        exhausted.error_message.as_deref(),
        Some("No more faux responses queued")
    );
    assert_eq!(registration.get_pending_response_count(), 0);
    assert_eq!(registration.state().call_count(), 3);
}

#[tokio::test]
async fn can_replace_and_append_queued_responses() {
    let registration = register(Default::default());
    let text = |m: &AssistantMessage| content_json(m)[0]["text"].as_str().unwrap().to_string();
    registration.set_responses(vec![msg("first")]);

    assert_eq!(
        text(&complete(registration.get_model(), hi()).await),
        "first"
    );
    assert_eq!(registration.get_pending_response_count(), 0);

    registration.set_responses(vec![msg("second")]);
    assert_eq!(registration.get_pending_response_count(), 1);
    assert_eq!(
        text(&complete(registration.get_model(), hi()).await),
        "second"
    );

    registration.append_responses(vec![msg("third"), msg("fourth")]);
    assert_eq!(registration.get_pending_response_count(), 2);
    assert_eq!(
        text(&complete(registration.get_model(), hi()).await),
        "third"
    );
    assert_eq!(
        text(&complete(registration.get_model(), hi()).await),
        "fourth"
    );
    assert_eq!(registration.get_pending_response_count(), 0);
}

#[tokio::test]
async fn supports_async_response_factories() {
    let registration = register(Default::default());
    registration.set_responses(vec![FauxResponseStep::async_factory(
        |context, _options, state, _model| {
            let text = format!("{}:{}", context.messages.len(), state.call_count());
            async move { Ok(faux_assistant_message(text, Default::default())) }
        },
    )]);

    let response = complete(registration.get_model(), hi()).await;
    assert_eq!(
        content_json(&response),
        json!([{"type": "text", "text": "1:1"}])
    );
}

#[tokio::test]
async fn emits_an_error_when_a_response_factory_throws() {
    let registration = register(Default::default());
    registration.set_responses(vec![FauxResponseStep::factory(|_, _, _, _| {
        Err("boom".into())
    })]);

    let events = collect_events(registration.get_model(), hi()).await;
    assert_eq!(events.len(), 1);
    let AssistantMessageEvent::Error { error } = &events[0] else {
        panic!("expected error, got {:?}", events[0]);
    };
    assert_eq!(error.stop_reason, StopReason::Error);
    assert_eq!(error.error_message.as_deref(), Some("boom"));
}

#[tokio::test]
async fn estimates_prompt_and_output_tokens_from_serialized_context() {
    let registration = register(Default::default());
    registration.set_responses(vec![msg("done")]);

    let tool = Tool {
        name: "echo".into(),
        description: "Echo back text".into(),
        parameters: json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        }),
    };
    let context = Context::new(
        "sys".into(),
        vec![
            Message::User(UserMessage {
                content: vec![
                    faux_text("hello"),
                    Content::Image(ImageContent {
                        data: "abcd".into(),
                        media_type: "image/png".into(),
                    }),
                ]
                .into(),
                timestamp: 1,
            }),
            Message::Assistant(faux_assistant_message("prior", Default::default())),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "tool-1".into(),
                tool_name: "echo".into(),
                content: vec![faux_text("tool out")],
                details: None,
                is_error: false,
                timestamp: 2,
            }),
        ],
        vec![tool.clone()],
    );

    let response = complete(registration.get_model(), context).await;
    let tools_json = json!([{
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.parameters,
    }]);
    let prompt_text = [
        "system:sys".to_string(),
        "user:hello\n[image:image/png:4]".to_string(),
        "assistant:prior".to_string(),
        "toolResult:echo\ntool out".to_string(),
        format!("tools:{tools_json}"),
    ]
    .join("\n\n");
    let expected_prompt_tokens = prompt_text.len().div_ceil(4) as u64;
    let expected_output_tokens = "done".len().div_ceil(4) as u64;

    assert_eq!(response.usage.input, expected_prompt_tokens);
    assert_eq!(response.usage.output, expected_output_tokens);
    assert_eq!(response.usage.cache_read, 0);
    assert_eq!(response.usage.cache_write, 0);
    assert_eq!(
        response.usage.total_tokens,
        expected_prompt_tokens + expected_output_tokens
    );
}

fn session(id: &str, retention: CacheRetention) -> SimpleStreamOptions {
    SimpleStreamOptions {
        session_id: Some(id.into()),
        cache_retention: Some(retention),
        ..Default::default()
    }
}

#[tokio::test]
async fn does_not_share_cache_across_sessions_or_requests_without_session_id() {
    let registration = register(Default::default());
    registration.set_responses(vec![msg("first"), msg("second"), msg("third")]);

    let mut context = Context::new(String::new(), vec![user("hello")], vec![]);
    let first = complete_with(
        registration.get_model(),
        context.clone(),
        session("session-1", CacheRetention::Short),
    )
    .await;
    assert!(first.usage.cache_write > 0);
    context.messages.push(Message::Assistant(first));
    context.messages.push(user_at("follow up", now_ms() + 1));

    let second = complete_with(
        registration.get_model(),
        context.clone(),
        session("session-2", CacheRetention::Short),
    )
    .await;
    assert_eq!(second.usage.cache_read, 0);
    assert!(second.usage.cache_write > 0);

    let third = complete(registration.get_model(), context).await;
    assert_eq!(third.usage.cache_read, 0);
    assert_eq!(third.usage.cache_write, 0);
}

#[tokio::test]
async fn simulates_prompt_caching_per_session_id() {
    let registration = register(Default::default());
    registration.set_responses(vec![msg("first"), msg("second")]);

    let mut context = Context::new("Be concise.".into(), vec![user("hello")], vec![]);
    let first = complete_with(
        registration.get_model(),
        context.clone(),
        session("session-1", CacheRetention::Short),
    )
    .await;
    assert_eq!(first.usage.cache_read, 0);
    assert!(first.usage.cache_write > 0);

    context.messages.push(Message::Assistant(first));
    context.messages.push(user_at("follow up", now_ms() + 1));

    let second = complete_with(
        registration.get_model(),
        context,
        session("session-1", CacheRetention::Short),
    )
    .await;
    assert!(second.usage.cache_read > 0);
    assert!(second.usage.input + second.usage.cache_read > second.usage.input);
}

#[tokio::test]
async fn does_not_simulate_caching_when_cache_retention_is_none() {
    let registration = register(Default::default());
    registration.set_responses(vec![msg("first"), msg("second")]);

    let mut context = Context::new(String::new(), vec![user("hello")], vec![]);
    complete_with(
        registration.get_model(),
        context.clone(),
        session("session-1", CacheRetention::None),
    )
    .await;
    context
        .messages
        .push(Message::Assistant(faux_assistant_message(
            "first",
            Default::default(),
        )));
    context.messages.push(user_at("follow up", now_ms() + 1));
    let second = complete_with(
        registration.get_model(),
        context,
        session("session-1", CacheRetention::None),
    )
    .await;
    assert_eq!(second.usage.cache_read, 0);
    assert_eq!(second.usage.cache_write, 0);
}

#[tokio::test]
async fn streams_thinking_text_and_partial_tool_call_deltas() {
    let registration = register(Default::default());
    registration.set_responses(vec![faux_assistant_message(
        vec![
            faux_thinking("thinking text"),
            faux_text("answer text"),
            faux_tool_call(
                "echo",
                json!({"text": "hi", "count": 12}),
                Some("tool-1".into()),
            ),
        ],
        tool_use(),
    )
    .into()]);

    let events = collect_events(registration.get_model(), hi()).await;
    let names = types(&events);
    for expected in [
        "thinking_start",
        "thinking_delta",
        "text_start",
        "text_delta",
        "toolcall_start",
        "toolcall_delta",
        "toolcall_end",
    ] {
        assert!(names.contains(&expected), "{expected} in {names:?}");
    }
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(deltas.len() > 1);
    let joined: serde_json::Value = serde_json::from_str(&deltas.concat()).unwrap();
    assert_eq!(joined, json!({"text": "hi", "count": 12}));
}

#[tokio::test]
async fn streams_an_exact_event_order_for_fixed_size_chunks() {
    let registration = register(sized(1, 1, None));
    registration.set_responses(vec![faux_assistant_message(
        vec![
            faux_thinking("go"),
            faux_text("ok"),
            faux_tool_call("echo", json!({}), Some("tool-1".into())),
        ],
        tool_use(),
    )
    .into()]);

    let events = collect_events(registration.get_model(), hi()).await;
    assert_eq!(
        types(&events),
        [
            "start",
            "thinking_start",
            "thinking_delta",
            "thinking_end",
            "text_start",
            "text_delta",
            "text_end",
            "toolcall_start",
            "toolcall_delta",
            "toolcall_end",
            "done",
        ]
    );
}

#[tokio::test]
async fn streams_multiple_tool_calls_in_one_message() {
    let registration = register(Default::default());
    registration.set_responses(vec![faux_assistant_message(
        vec![
            faux_tool_call("echo", json!({"text": "one"}), Some("tool-1".into())),
            faux_tool_call("echo", json!({"text": "two"}), Some("tool-2".into())),
        ],
        tool_use(),
    )
    .into()]);

    let events = collect_events(registration.get_model(), hi()).await;
    let names = types(&events);
    assert_eq!(names.iter().filter(|t| **t == "toolcall_start").count(), 2);
    assert_eq!(names.iter().filter(|t| **t == "toolcall_end").count(), 2);
}

async fn assert_terminal_error(stop_reason: StopReason, error_message: &str) {
    let registration = register(sized(2, 2, None));
    registration.set_responses(vec![AssistantMessage {
        stop_reason,
        error_message: Some(error_message.into()),
        ..faux_assistant_message("partial", Default::default())
    }
    .into()]);

    let events = collect_events(registration.get_model(), hi()).await;
    assert_eq!(
        types(&events),
        ["start", "text_start", "text_delta", "text_end", "error"]
    );
    let Some(AssistantMessageEvent::Error { error }) = events.last() else {
        panic!("expected terminal error");
    };
    // `terminal.reason` is the error message's stop reason.
    assert_eq!(error.stop_reason, stop_reason);
    assert_eq!(error.error_message.as_deref(), Some(error_message));
}

#[tokio::test]
async fn streams_an_explicit_assistant_error_message_as_a_terminal_error() {
    assert_terminal_error(StopReason::Error, "upstream failed").await;
}

#[tokio::test]
async fn streams_an_explicit_assistant_aborted_message_as_a_terminal_error() {
    assert_terminal_error(StopReason::Aborted, "Request was aborted").await;
}

#[tokio::test]
async fn supports_aborting_before_the_first_chunk() {
    let registration = register(sized(3, 3, Some(50.0)));
    registration.set_responses(vec![msg("abcdefghijklmnopqrstuvwxyz")]);

    let signal = AbortSignal::new();
    signal.abort();
    let events: Vec<_> = stream_simple(
        registration.get_model(),
        hi(),
        SimpleStreamOptions {
            signal: Some(signal),
            ..Default::default()
        },
    )
    .unwrap()
    .collect()
    .await;

    assert_eq!(events.len(), 1);
    let AssistantMessageEvent::Error { error } = &events[0] else {
        panic!("expected error");
    };
    assert_eq!(error.stop_reason, StopReason::Aborted);
}

/// Abort on the first `<kind>_delta` of a paced stream: exactly one delta,
/// then an error and no `<kind>_end`.
async fn assert_aborts_mid_stream(response: AssistantMessage, kind: &str) {
    let registration = register(sized(3, 3, Some(100.0)));
    registration.set_responses(vec![response.into()]);

    let signal = AbortSignal::new();
    let mut stream = stream_simple(
        registration.get_model(),
        hi(),
        SimpleStreamOptions {
            signal: Some(signal.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let mut names = Vec::new();
    let mut delta_count = 0;
    let delta = format!("{kind}_delta");
    while let Some(event) = stream.next().await {
        names.push(event_type(&event));
        if event_type(&event) == delta {
            delta_count += 1;
            signal.abort();
        }
    }

    assert_eq!(delta_count, 1);
    assert!(names.contains(&format!("{kind}_start").as_str()));
    assert!(names.contains(&delta.as_str()));
    assert!(names.contains(&"error"));
    assert!(!names.contains(&format!("{kind}_end").as_str()));
}

#[tokio::test]
async fn supports_aborting_mid_text_stream_when_paced() {
    assert_aborts_mid_stream(
        faux_assistant_message("abcdefghijklmnopqrstuvwxyz", Default::default()),
        "text",
    )
    .await;
}

#[tokio::test]
async fn supports_aborting_mid_thinking_stream_when_paced() {
    assert_aborts_mid_stream(
        AssistantMessage {
            content: vec![faux_thinking("abcdefghijklmnopqrstuvwxyz")],
            ..faux_assistant_message("ignored", Default::default())
        },
        "thinking",
    )
    .await;
}

#[tokio::test]
async fn supports_aborting_mid_toolcall_stream_when_paced() {
    assert_aborts_mid_stream(
        AssistantMessage {
            content: vec![faux_tool_call(
                "echo",
                json!({"text": "abcdefghijklmnopqrstuvwxyz", "count": 123456789}),
                Some("tool-1".into()),
            )],
            stop_reason: StopReason::ToolUse,
            ..faux_assistant_message("done", Default::default())
        },
        "toolcall",
    )
    .await;
}

#[tokio::test]
async fn unregisters_the_provider() {
    let registration = register_faux_provider(Default::default());
    registration.set_responses(vec![msg("hello")]);
    registration.unregister();

    let err = complete_simple(registration.get_model(), hi(), Default::default())
        .await
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        format!("No API provider registered for api: {}", registration.api())
    );
}
