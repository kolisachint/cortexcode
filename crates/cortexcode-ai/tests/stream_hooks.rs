//! `onPayload` / `onResponse` (ledger 8.8): every registered API passes its
//! request body through `onPayload` and sends the replacement; the HTTP
//! providers report the response status and headers to `onResponse` before
//! reading the body. Plus the port of `openrouter-cache-write-repro.test.ts`
//! (live, `#[ignore]`, needs `OPENROUTER_API_KEY`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use cortexcode_ai::registry::{complete_simple, get_api_provider, stream_simple};
use cortexcode_ai::types::{
    Content, Context, Message, Model, OnPayload, OnResponse, ProviderResponse, SimpleStreamOptions,
    StopReason, TextContent, Transport, UserMessage,
};
use cortexcode_ai_stream::testing::serve_script;
use serde_json::{json, Value};

/// One catalog model per (provider, api) pair.
fn one_model_per_provider_api() -> Vec<Model> {
    let mut seen = BTreeMap::new();
    for provider in cortexcode_ai::models::get_providers() {
        for model in cortexcode_ai::models::get_models(provider) {
            seen.entry((model.provider.clone(), model.api.clone()))
                .or_insert_with(|| model.clone());
        }
    }
    seen.into_values().collect()
}

fn hi() -> Context {
    Context::new(
        String::new(),
        vec![Message::User(UserMessage {
            content: vec![Content::Text(TextContent::new("hi"))].into(),
            timestamp: 0,
        })],
        vec![],
    )
}

/// An access token carrying a ChatGPT account id (codex reads it).
fn codex_token() -> String {
    let payload =
        "eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjIn19";
    format!("aaa.{payload}.bbb")
}

fn api_key(api: &str) -> String {
    match api {
        "openai-codex-responses" => codex_token(),
        "google-gemini-cli" => r#"{"token":"t","projectId":"p"}"#.into(),
        _ => "test-key".into(),
    }
}

/// A reply the provider accepts without retrying: Cloud Code Assist and
/// Codex retry failures, so they get one they stop on.
fn reply(api: &str) -> (&'static str, &'static str, &'static str) {
    match api {
        "google-gemini-cli" => (
            "HTTP/1.1 200 OK",
            "text/event-stream",
            "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\"}]}}\n\n",
        ),
        "openai-codex-responses" => (
            "HTTP/1.1 400 Bad Request",
            "application/json",
            r#"{"error":{"code":"usage_limit_reached","message":"limit"}}"#,
        ),
        _ => (
            "HTTP/1.1 400 Bad Request",
            "application/json",
            r#"{"error":{"message":"hooked"}}"#,
        ),
    }
}

fn is_google_sdk(api: &str) -> bool {
    matches!(api, "google-generative-ai" | "google-vertex")
}

#[test]
fn on_payload_sees_and_replaces_the_request_body_for_every_api() {
    for mut model in one_model_per_provider_api() {
        if get_api_provider(&model.api).is_none() {
            continue;
        }
        let api = model.api.clone();
        let label = format!("{}/{} ({api})", model.provider, model.id);
        let server = serve_script(vec![reply(&api)]);
        model.base_url = server.base_url.clone();

        let seen: Arc<Mutex<Vec<(Value, String)>>> = Default::default();
        let record = seen.clone();
        let google_sdk = is_google_sdk(&api);
        let hook = OnPayload::sync(move |payload: &Value, model: &Model| {
            record
                .lock()
                .unwrap()
                .push((payload.clone(), model.id.clone()));
            let mut replaced = payload.clone();
            if google_sdk {
                // The SDK params are mapped onto the REST body field by field.
                replaced["contents"] = json!([{"role": "user", "parts": [{"text": "hookMarker"}]}]);
            } else {
                replaced["hookMarker"] = json!(true);
            }
            Some(replaced)
        });
        let options = SimpleStreamOptions {
            api_key: Some(api_key(&api)),
            transport: Some(Transport::Sse),
            on_payload: Some(hook),
            ..Default::default()
        };
        let message = stream_simple(model.clone(), hi(), options)
            .unwrap_or_else(|e| panic!("{label}: {e}"))
            .result_blocking();
        let requests = server.requests();
        if api == "google-vertex" && requests.is_empty() {
            // Vertex needs a project and location before any request.
            continue;
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{label}: {:?}", message.error_message);
        assert_eq!(seen[0].1, model.id, "{label}");
        assert!(seen[0].0.is_object(), "{label}");
        assert!(!requests.is_empty(), "{label}");
        assert!(
            requests[0].body.contains("hookMarker"),
            "{label}: {}",
            requests[0].body
        );
    }
}

#[test]
fn on_payload_returning_none_keeps_the_payload() {
    let mut model = cortexcode_ai::models::get_model("openai", "gpt-4o-mini")
        .or_else(|| {
            cortexcode_ai::models::get_models("openai")
                .into_iter()
                .find(|m| m.api == "openai-completions")
        })
        .expect("an openai-completions model")
        .clone();
    model.api = "openai-completions".into();
    let server = serve_script(vec![reply("openai-completions")]);
    model.base_url = server.base_url.clone();
    let options = SimpleStreamOptions {
        api_key: Some("k".into()),
        on_payload: Some(OnPayload::sync(|_: &Value, _: &Model| None)),
        ..Default::default()
    };
    stream_simple(model, hi(), options)
        .unwrap()
        .result_blocking();
    let body = server.requests()[0].json();
    assert_eq!(body["messages"][0]["role"], "user");
    assert!(body.get("hookMarker").is_none());
}

/// Record every `onResponse` call.
fn recorder() -> (OnResponse, Arc<Mutex<Vec<ProviderResponse>>>) {
    let seen: Arc<Mutex<Vec<ProviderResponse>>> = Default::default();
    let record = seen.clone();
    let hook = OnResponse::new(move |response: ProviderResponse, _: Model| {
        let record = record.clone();
        async move {
            record.lock().unwrap().push(response);
        }
    });
    (hook, seen)
}

fn first_model(api: &str) -> Model {
    one_model_per_provider_api()
        .into_iter()
        .find(|m| m.api == api)
        .unwrap_or_else(|| panic!("a model on {api}"))
}

#[test]
fn on_response_gets_the_status_and_headers_before_the_body() {
    // The SDK-backed providers call it once the response succeeded.
    for api in [
        "openai-completions",
        "openai-responses",
        "azure-openai-responses",
        "anthropic-messages",
    ] {
        let mut model = first_model(api);
        let server = serve_script(vec![("HTTP/1.1 200 OK", "text/event-stream", "")]);
        model.base_url = server.base_url.clone();
        let (hook, seen) = recorder();
        let options = SimpleStreamOptions {
            api_key: Some("k".into()),
            on_response: Some(hook),
            ..Default::default()
        };
        stream_simple(model, hi(), options)
            .unwrap()
            .result_blocking();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{api}");
        assert_eq!(seen[0].status, 200, "{api}");
        assert_eq!(
            seen[0].headers.get("content-type").map(String::as_str),
            Some("text/event-stream"),
            "{api}"
        );
        assert_eq!(
            seen[0].headers.get("content-length").map(String::as_str),
            Some("0"),
            "{api}"
        );

        // A failed request never reaches the hook.
        let mut model = first_model(api);
        let server = serve_script(vec![reply(api)]);
        model.base_url = server.base_url.clone();
        let (hook, seen) = recorder();
        let options = SimpleStreamOptions {
            api_key: Some("k".into()),
            max_retries: Some(0),
            on_response: Some(hook),
            ..Default::default()
        };
        let message = stream_simple(model, hi(), options)
            .unwrap()
            .result_blocking();
        assert_eq!(message.stop_reason, StopReason::Error, "{api}");
        assert!(seen.lock().unwrap().is_empty(), "{api}");
    }
}

#[test]
fn codex_reports_every_sse_response_including_failures() {
    let mut model = first_model("openai-codex-responses");
    let server = serve_script(vec![reply("openai-codex-responses")]);
    model.base_url = server.base_url.clone();
    let (hook, seen) = recorder();
    let options = SimpleStreamOptions {
        api_key: Some(codex_token()),
        transport: Some(Transport::Sse),
        on_response: Some(hook),
        ..Default::default()
    };
    let message = stream_simple(model, hi(), options)
        .unwrap()
        .result_blocking();
    assert_eq!(message.stop_reason, StopReason::Error);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].status, 400);
}

#[test]
fn provider_response_joins_repeated_headers_and_lowercases_names() {
    let response = ProviderResponse::from_pairs(
        201,
        [
            ("X-Trace".to_string(), "a".to_string()),
            ("Content-Type".to_string(), "text/plain".to_string()),
            ("x-trace".to_string(), "b".to_string()),
        ],
    );
    assert_eq!(response.status, 201);
    let names: Vec<&str> = response.headers.keys().map(String::as_str).collect();
    assert_eq!(names, ["content-type", "x-trace"]);
    assert_eq!(response.headers["x-trace"], "a, b");
}

// --- openrouter-cache-write-repro.test.ts ---

fn long_system_prompt() -> String {
    let nonce = format!("{}-{}", cortexcode_ai::types::now_ms(), std::process::id());
    let block = "Prompt-caching probe content. Keep this exact text stable across requests so the provider can reuse prefix tokens and report cache read and cache write usage.";
    format!(
        "You are a concise assistant.\nCache nonce: {nonce}\n\n{}",
        vec![block; 80].join("\n\n")
    )
}

/// Mark the last user text part with `cache_control: ephemeral`.
fn mark_last_user_message(payload: &Value) -> Option<Value> {
    let mut payload = payload.clone();
    let messages = payload.get_mut("messages")?.as_array_mut()?;
    let message = messages.iter_mut().rev().find(|m| m["role"] == "user")?;
    if let Some(text) = message["content"].as_str().map(str::to_string) {
        message["content"] =
            json!([{"type": "text", "text": text, "cache_control": {"type": "ephemeral"}}]);
    } else if let Some(parts) = message["content"].as_array_mut() {
        if let Some(part) = parts.iter_mut().rev().find(|p| p["type"] == "text") {
            part["cache_control"] = json!({"type": "ephemeral"});
        }
    }
    Some(payload)
}

#[tokio::test]
#[ignore = "live: needs OPENROUTER_API_KEY"]
async fn openrouter_preserves_cache_write_tokens_on_the_completions_stream() {
    let Ok(api_key) = std::env::var("OPENROUTER_API_KEY") else {
        return;
    };
    let model = cortexcode_ai::models::get_model("openrouter", "google/gemini-2.5-flash")
        .expect("openrouter google/gemini-2.5-flash")
        .clone();
    let mut context = hi();
    context.system_prompt = long_system_prompt();
    context.messages = vec![Message::User(UserMessage {
        content: "Reply with exactly: OK".into(),
        timestamp: cortexcode_ai::types::now_ms(),
    })];
    let options = SimpleStreamOptions {
        api_key: Some(api_key),
        max_tokens: Some(32),
        temperature: Some(0.0),
        on_payload: Some(OnPayload::sync(|payload: &Value, _: &Model| {
            mark_last_user_message(payload).or_else(|| Some(payload.clone()))
        })),
        ..Default::default()
    };
    // `{ retry: 2 }`: up to three runs.
    let mut last = String::new();
    for _ in 0..3 {
        let first = complete_simple(model.clone(), context.clone(), options.clone())
            .await
            .unwrap();
        let second = complete_simple(model.clone(), context.clone(), options.clone())
            .await
            .unwrap();
        if first.stop_reason == StopReason::Stop
            && second.stop_reason == StopReason::Stop
            && (first.usage.cache_write > 0 || second.usage.cache_write > 0)
        {
            return;
        }
        last = format!(
            "first: {:?} {:?} {:?}; second: {:?} {:?} {:?}",
            first.stop_reason,
            first.error_message,
            first.usage,
            second.stop_reason,
            second.error_message,
            second.usage
        );
    }
    panic!("no cache write reported: {last}");
}

#[test]
fn the_cache_marker_goes_on_the_last_user_text() {
    let payload = json!({"messages": [
        {"role": "system", "content": "s"},
        {"role": "user", "content": "first"},
        {"role": "assistant", "content": "a"},
        {"role": "user", "content": [{"type": "image_url"}, {"type": "text", "text": "q"}]},
    ]});
    let marked = mark_last_user_message(&payload).unwrap();
    assert_eq!(marked["messages"][1]["content"], "first");
    assert_eq!(
        marked["messages"][3]["content"][1]["cache_control"],
        json!({"type": "ephemeral"})
    );
    let plain =
        mark_last_user_message(&json!({"messages": [{"role": "user", "content": "x"}]})).unwrap();
    assert_eq!(
        plain["messages"][0]["content"],
        json!([{"type": "text", "text": "x", "cache_control": {"type": "ephemeral"}}])
    );
}
