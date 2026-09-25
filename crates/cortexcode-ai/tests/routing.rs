//! Every known provider is routed on `model.api` through the API registry,
//! and openai-completions providers get their compat applied on the wire.
//!
//! Covers the routing half of ledger task 8.2 (hoocode v0.5.89
//! `stream.ts` + `register-builtins.ts` + `openai-completions.ts` `getCompat`).

use cortexcode_ai::registry::{get_api_provider, stream_simple};
use cortexcode_ai::types::{
    Content, Context, Message, Model, SimpleStreamOptions, StopReason, TextContent, ThinkingLevel,
    UserMessage,
};
use cortexcode_ai_stream::testing::serve_script;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// APIs in the catalog whose providers are not ported yet, with the ledger
/// task that ports them. Remove an entry when its task registers the API.
const PENDING_APIS: &[(&str, &str)] = &[
    ("openai-codex-responses", "8.4a"),
    ("google-gemini-cli", "8.4c"),
];

/// One catalog model per (provider, api) pair, in catalog order.
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
            content: vec![Content::Text(TextContent::new("hi"))],
            timestamp: 0,
        })],
        vec![],
    )
}

#[test]
fn all_known_providers_are_in_the_catalog() {
    let providers = cortexcode_ai::models::get_providers();
    assert_eq!(providers.len(), 31, "{providers:?}");
}

#[test]
fn every_catalog_api_is_registered_or_pending_a_task() {
    for model in one_model_per_provider_api() {
        let registered = get_api_provider(&model.api).is_some();
        let pending = PENDING_APIS.iter().any(|(api, _)| *api == model.api);
        assert!(
            registered != pending,
            "{}/{} api {}: registered={registered} pending={pending}",
            model.provider,
            model.id,
            model.api
        );
    }
}

/// Each registered (provider, api) pair reaches its API's endpoint through
/// `streamSimple`, and the failure comes back as an error message stamped
/// with the model's identity.
#[test]
fn every_registered_provider_streams_through_its_api() {
    for mut model in one_model_per_provider_api() {
        if get_api_provider(&model.api).is_none() {
            continue;
        }
        let server = serve_script(vec![(
            "HTTP/1.1 400 Bad Request",
            "application/json",
            r#"{"error":{"message":"routed"}}"#,
        )]);
        model.base_url = server.base_url.clone();
        let options = SimpleStreamOptions {
            api_key: Some("test-key".into()),
            ..Default::default()
        };
        let label = format!("{}/{} ({})", model.provider, model.id, model.api);
        let stream =
            stream_simple(model.clone(), hi(), options).unwrap_or_else(|e| panic!("{label}: {e}"));
        let message = stream.result_blocking();
        assert_eq!(message.stop_reason, StopReason::Error, "{label}");
        assert_eq!(
            (message.api.as_str(), message.provider.as_str()),
            (model.api.as_str(), model.provider.as_str()),
            "{label}"
        );
        let requests = server.requests();
        if model.api == "google-vertex" && requests.is_empty() {
            // Vertex resolves a project and location (or ADC) before any
            // request; without them the error comes from credential setup.
            continue;
        }
        assert_eq!(requests.len(), 1, "{label}");
        let path = &requests[0].path;
        let expected = match model.api.as_str() {
            "openai-completions" => "/chat/completions",
            "anthropic-messages" => "/messages",
            "google-generative-ai" | "google-vertex" => ":streamGenerateContent",
            "openai-responses" | "azure-openai-responses" => "/responses",
            other => panic!("{label}: no expected path for {other}"),
        };
        assert!(path.contains(expected), "{label}: {path}");
    }
}

fn completions_payload(provider: &str, reasoning: Option<ThinkingLevel>) -> (Model, Value) {
    let mut model = cortexcode_ai::models::get_models(provider)
        .into_iter()
        .find(|m| m.api == "openai-completions" && (reasoning.is_none() || m.reasoning))
        .unwrap_or_else(|| panic!("{provider} has an openai-completions model"))
        .clone();
    let server = serve_script(vec![(
        "HTTP/1.1 400 Bad Request",
        "application/json",
        r#"{"error":{"message":"captured"}}"#,
    )]);
    model.base_url = server.base_url.clone();
    let options = SimpleStreamOptions {
        api_key: Some("test-key".into()),
        reasoning,
        cache_retention: Some(cortexcode_ai::types::CacheRetention::Short),
        ..Default::default()
    };
    stream_simple(model.clone(), hi(), options)
        .unwrap()
        .result_blocking();
    let body = server.requests()[0].json();
    (model, body)
}

#[test]
fn openai_completions_providers_get_their_compat_on_the_wire() {
    // Moonshot and Together: `max_tokens`, no `store`.
    for provider in ["moonshotai", "moonshotai-cn", "together"] {
        let (_, body) = completions_payload(provider, None);
        assert!(body.get("max_tokens").is_some(), "{provider}: {body}");
        assert!(body.get("max_completion_tokens").is_none(), "{provider}");
        assert!(body.get("store").is_none(), "{provider}");
    }
    // Standard endpoints: `max_completion_tokens` and `store: false`.
    for provider in ["groq", "huggingface", "nvidia", "openrouter"] {
        let (_, body) = completions_payload(provider, None);
        assert!(
            body.get("max_completion_tokens").is_some(),
            "{provider}: {body}"
        );
        assert_eq!(body["store"], false, "{provider}");
    }
    // Non-standard providers drop `store` (DeepSeek is detected by its
    // base URL only, so it is not in this list).
    for provider in ["cerebras", "xai", "zai"] {
        let (_, body) = completions_payload(provider, None);
        assert!(body.get("store").is_none(), "{provider}: {body}");
    }
    // Thinking formats.
    let (_, body) = completions_payload("deepseek", Some(ThinkingLevel::High));
    assert_eq!(body["thinking"], json!({"type": "enabled"}));
    let (_, body) = completions_payload("zai", Some(ThinkingLevel::High));
    assert_eq!(body["enable_thinking"], true);
    let (_, body) = completions_payload("openrouter", Some(ThinkingLevel::High));
    assert!(body["reasoning"]["effort"].is_string(), "{body}");
    assert!(body.get("reasoning_effort").is_none());
    let (_, body) = completions_payload("together", Some(ThinkingLevel::High));
    assert_eq!(body["reasoning"]["enabled"], true);
    let (_, body) = completions_payload("xai", Some(ThinkingLevel::High));
    assert!(body.get("reasoning_effort").is_none(), "grok: {body}");
}
