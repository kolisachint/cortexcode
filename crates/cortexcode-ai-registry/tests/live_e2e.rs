//! Live provider tests ported from hoocode `packages/ai/test/abort.test.ts` and
//! the basic cases of `stream.test.ts` (text, streaming, tool calling).
//!
//! Ignored by default. Each test returns early (skip) when its provider's key is
//! not set, like `describe.skipIf`. Run with e.g.
//! `ANTHROPIC_API_KEY=... cargo test -p cortexcode-ai-registry --test live_e2e -- --ignored`.
//! The remaining `stream.test.ts` cases (images, thinking, multi-turn, the other
//! providers) are ledger 8.6.

use cortexcode_ai_registry::{complete_simple, stream_simple};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessageEvent, Content, Context, Message, Model, SimpleStreamOptions,
    StopReason, Tool, UserMessage,
};
use futures_util::StreamExt;

fn model(provider: &str, id: &str, api: Option<&str>) -> Model {
    let mut m = cortexcode_ai_models::get_model(provider, id)
        .unwrap_or_else(|| panic!("{provider}/{id} is not in the catalog"))
        .clone();
    if let Some(api) = api {
        m.api = api.to_string();
    }
    m
}

fn key(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: vec![Content::text(text)],
        timestamp: cortexcode_ai_types::now_ms(),
    })
}

fn options(api_key: &str) -> SimpleStreamOptions {
    SimpleStreamOptions {
        api_key: Some(api_key.to_string()),
        ..Default::default()
    }
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect()
}

/// `basicTextGeneration`.
async fn basic_text_generation(llm: Model, api_key: &str) {
    let context = Context::new(
        "You are a helpful assistant. Be concise.".into(),
        vec![user("Reply with exactly: 'Hello test successful'")],
        vec![],
    );
    let msg = complete_simple(llm, context, options(api_key))
        .await
        .unwrap();
    assert_eq!(msg.stop_reason, StopReason::Stop, "{:?}", msg.error_message);
    assert!(msg.usage.input + msg.usage.cache_read > 0);
    assert!(msg.usage.output > 0);
    assert!(text_of(&msg.content).contains("Hello test successful"));
}

/// `handleStreaming`: the deltas add up to the final text.
async fn handle_streaming(llm: Model, api_key: &str) {
    let context = Context::new(
        "You are a helpful assistant.".into(),
        vec![user("Count from 1 to 3")],
        vec![],
    );
    let mut s = stream_simple(llm, context, options(api_key)).unwrap();
    let (mut text_started, mut text_completed, mut text) = (false, false, String::new());
    while let Some(event) = s.next().await {
        match event {
            AssistantMessageEvent::TextStart { .. } => text_started = true,
            AssistantMessageEvent::TextDelta { delta, .. } => text.push_str(&delta),
            AssistantMessageEvent::TextEnd { .. } => text_completed = true,
            _ => {}
        }
    }
    let msg = s.result().await;
    assert!(text_started && text_completed);
    assert!(!text.is_empty());
    assert!(msg.content.iter().any(|c| matches!(c, Content::Text(_))));
}

/// `handleToolCall`.
async fn handle_tool_call(llm: Model, api_key: &str) {
    let calculator = Tool {
        name: "math_operation".into(),
        description: "Perform basic arithmetic operations".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "number", "description": "First number"},
                "b": {"type": "number", "description": "Second number"},
                "operation": {"type": "string", "enum": ["add", "subtract", "multiply", "divide"]}
            },
            "required": ["a", "b", "operation"]
        }),
    };
    let context = Context::new(
        "You are a helpful assistant that uses tools when asked.".into(),
        vec![user(
            "Calculate 15 + 27 using the math_operation tool. You MUST use the tool.",
        )],
        vec![calculator],
    );
    let msg = complete_simple(llm, context, options(api_key))
        .await
        .unwrap();
    assert_eq!(
        msg.stop_reason,
        StopReason::ToolUse,
        "{:?}",
        msg.error_message
    );
    let call = msg
        .content
        .iter()
        .find_map(|c| match c {
            Content::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .expect("a tool call");
    assert_eq!(call.name, "math_operation");
    assert_eq!(call.arguments["a"], 15);
    assert_eq!(call.arguments["b"], 27);
    assert_eq!(call.arguments["operation"], "add");
}

/// `testAbortSignal`.
async fn abort_mid_stream(llm: Model, api_key: &str) {
    let mut context = Context::new(
        "You are a helpful assistant.".into(),
        vec![user(
            "What is 15 + 27? Think step by step. Then list 50 first names.",
        )],
        vec![],
    );
    let signal = AbortSignal::new();
    let mut opts = options(api_key);
    opts.signal = Some(signal.clone());
    let mut s = stream_simple(llm.clone(), context.clone(), opts).unwrap();
    let mut text = String::new();
    while let Some(event) = s.next().await {
        if signal.aborted() {
            break;
        }
        match event {
            AssistantMessageEvent::TextDelta { delta, .. }
            | AssistantMessageEvent::ThinkingDelta { delta, .. } => text.push_str(&delta),
            _ => {}
        }
        if text.len() >= 50 {
            signal.abort();
        }
    }
    let msg = s.result().await;
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert!(!msg.content.is_empty());

    context.messages.push(Message::Assistant(msg));
    context
        .messages
        .push(user("Please continue, but only generate 5 names."));
    let follow_up = complete_simple(llm, context, options(api_key))
        .await
        .unwrap();
    assert_eq!(follow_up.stop_reason, StopReason::Stop);
    assert!(!follow_up.content.is_empty());
}

/// `testImmediateAbort`.
async fn immediate_abort(llm: Model, api_key: &str) {
    let signal = AbortSignal::new();
    signal.abort();
    let mut opts = options(api_key);
    opts.signal = Some(signal);
    let context = Context::new(String::new(), vec![user("Hello")], vec![]);
    let msg = complete_simple(llm, context, opts).await.unwrap();
    assert_eq!(msg.stop_reason, StopReason::Aborted);
}

macro_rules! live_suite {
    ($suite:ident, $env:literal, $provider:literal, $id:literal, $api:expr) => {
        mod $suite {
            use super::*;

            fn setup() -> Option<(Model, String)> {
                let key = key($env)?;
                Some((model($provider, $id, $api), key))
            }

            #[tokio::test]
            #[ignore = "live: needs the provider key and network"]
            async fn basic_text() {
                let Some((llm, key)) = setup() else { return };
                basic_text_generation(llm, &key).await;
            }

            #[tokio::test]
            #[ignore = "live: needs the provider key and network"]
            async fn streaming() {
                let Some((llm, key)) = setup() else { return };
                handle_streaming(llm, &key).await;
            }

            #[tokio::test]
            #[ignore = "live: needs the provider key and network"]
            async fn tool_call() {
                let Some((llm, key)) = setup() else { return };
                handle_tool_call(llm, &key).await;
            }

            #[tokio::test]
            #[ignore = "live: needs the provider key and network"]
            async fn abort_mid_stream_then_follow_up() {
                let Some((llm, key)) = setup() else { return };
                abort_mid_stream(llm, &key).await;
            }

            #[tokio::test]
            #[ignore = "live: needs the provider key and network"]
            async fn abort_immediately() {
                let Some((llm, key)) = setup() else { return };
                immediate_abort(llm, &key).await;
            }
        }
    };
}

live_suite!(
    anthropic,
    "ANTHROPIC_API_KEY",
    "anthropic",
    "claude-haiku-4-5",
    None
);
live_suite!(
    openai_completions,
    "OPENAI_API_KEY",
    "openai",
    "gpt-4o-mini",
    Some("openai-completions")
);
live_suite!(google, "GEMINI_API_KEY", "google", "gemini-2.5-flash", None);
