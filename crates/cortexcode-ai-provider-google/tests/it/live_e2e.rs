//! Live Google tests: the Gemini and Vertex cases of hoocode's
//! `google-thinking-disable.test.ts` (v0.5.89). Ignored by default; each
//! returns early without credentials. Run with
//! `GEMINI_API_KEY=... cargo test -p cortexcode-ai-provider-google --test live_e2e -- --ignored`.

use cortexcode_ai_provider_google::{stream, stream_vertex};
use cortexcode_ai_types::{
    AssistantMessageEvent, Content, Context, Message, SimpleStreamOptions, StopReason, UserMessage,
};
use futures_util::StreamExt;

fn context() -> Context {
    Context::new(
        "You are a precise assistant. Follow the requested output format exactly.".into(),
        vec![Message::User(UserMessage {
            content: "Before replying, carefully solve 36863 * 5279 internally. Then reply with the word pong repeated exactly 40 times, separated by single spaces. Do not add any other text.".into(),
            timestamp: cortexcode_ai_types::now_ms(),
        })],
        vec![],
    )
}

/// `expectThinkingDisabledE2E`.
async fn expect_thinking_disabled(
    s: cortexcode_ai_stream::AssistantMessageEventStream,
    min_pongs: usize,
) {
    let mut s = s;
    let mut thinking_events = 0;
    while let Some(event) = s.next().await {
        if matches!(
            event,
            AssistantMessageEvent::ThinkingStart { .. }
                | AssistantMessageEvent::ThinkingDelta { .. }
                | AssistantMessageEvent::ThinkingEnd { .. }
        ) {
            thinking_events += 1;
        }
    }
    let response = s.result().await;
    assert_eq!(
        response.stop_reason,
        StopReason::Stop,
        "{:?}",
        response.error_message
    );
    assert_eq!(thinking_events, 0);
    let text: String = response
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            Content::Thinking(_) => panic!("thinking block"),
            _ => None,
        })
        .collect();
    let pongs = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.eq_ignore_ascii_case("pong"))
        .count();
    assert!(pongs >= min_pongs, "{pongs} pongs in {text:?}");
}

fn options(max_tokens: u64) -> SimpleStreamOptions {
    SimpleStreamOptions {
        max_tokens: Some(max_tokens),
        temperature: Some(0.0),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "live: needs GEMINI_API_KEY and network"]
async fn gemini_thinking_can_be_disabled() {
    if std::env::var("GEMINI_API_KEY").is_err() {
        return;
    }
    for (id, max_tokens, min_pongs) in [
        ("gemini-2.5-flash", 160, 35),
        ("gemini-3-flash-preview", 160, 35),
        ("gemini-3.1-pro-preview", 512, 20),
    ] {
        let model = cortexcode_ai_models::get_model("google", id)
            .unwrap()
            .clone();
        let mut opts = options(max_tokens);
        if id == "gemini-3.1-pro-preview" {
            opts.temperature = None;
        }
        expect_thinking_disabled(stream(model, context(), opts).unwrap(), min_pongs).await;
    }
}

#[tokio::test]
#[ignore = "live: needs Vertex credentials and network"]
async fn vertex_thinking_can_be_disabled() {
    let has_key = std::env::var("GOOGLE_CLOUD_API_KEY").is_ok();
    let has_project = (std::env::var("GOOGLE_CLOUD_PROJECT").is_ok()
        || std::env::var("GCLOUD_PROJECT").is_ok())
        && std::env::var("GOOGLE_CLOUD_LOCATION").is_ok();
    if !has_key && !has_project {
        return;
    }
    for id in ["gemini-2.5-flash", "gemini-3-flash-preview"] {
        let model = cortexcode_ai_models::get_model("google-vertex", id)
            .unwrap()
            .clone();
        expect_thinking_disabled(stream_vertex(model, context(), options(160)).unwrap(), 35).await;
    }
}
