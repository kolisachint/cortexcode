//! Port of hoocode `packages/ai/test/cache-retention.test.ts` (v0.5.89).
//!
//! The TS tests capture the payload with `onPayload`, which runs before the
//! request fails; here the providers' payload builders run directly. The
//! key-gated TS cases (env-driven defaults for anthropic and openai-responses)
//! only look at the payload, so they run here without a key too.
//! Retention comes from `HOOCODE_CACHE_RETENTION` (or `CORTEXCODE_…`), so
//! every test holds the env lock.

use cortexcode_ai_provider_anthropic::{build_params as anthropic_params, AnthropicOptions};
use cortexcode_ai_provider_openai::request::{
    build_params as completions_params, get_compat, CompletionsOptions,
};
use cortexcode_ai_provider_openai_responses::{build_params as responses_params, ResponsesOptions};
use cortexcode_ai_types::{CacheRetention, Context, Message, Model, UserMessage};
use serde_json::{json, Value};
use std::sync::{Mutex, MutexGuard};

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `f` with `HOOCODE_CACHE_RETENTION` set to `value` (and the cortex
/// override cleared), restoring both afterwards.
fn with_retention_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _lock = env_lock();
    let vars = ["HOOCODE_CACHE_RETENTION", "CORTEXCODE_CACHE_RETENTION"];
    let saved: Vec<_> = vars.iter().map(|v| (*v, std::env::var(v).ok())).collect();
    std::env::remove_var("CORTEXCODE_CACHE_RETENTION");
    match value {
        Some(v) => std::env::set_var("HOOCODE_CACHE_RETENTION", v),
        None => std::env::remove_var("HOOCODE_CACHE_RETENTION"),
    }
    let out = f();
    for (var, value) in saved {
        match value {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
    }
    out
}

fn context() -> Context {
    Context::new(
        "You are a helpful assistant.".into(),
        vec![Message::User(UserMessage {
            content: "Hello".into(),
            timestamp: 1,
        })],
        vec![],
    )
}

fn catalog(provider: &str, id: &str) -> Model {
    cortexcode_ai_models::get_model(provider, id)
        .unwrap()
        .clone()
}

fn anthropic(model: &Model, retention: Option<CacheRetention>) -> Value {
    let options = AnthropicOptions {
        api_key: Some("fake-key".into()),
        cache_retention: retention,
        ..Default::default()
    };
    anthropic_params(model, &context(), false, &options)
}

fn responses(model: &Model, retention: Option<CacheRetention>, session: Option<&str>) -> Value {
    let options = ResponsesOptions {
        api_key: Some("fake-key".into()),
        cache_retention: retention,
        session_id: session.map(str::to_string),
        ..Default::default()
    };
    responses_params(model, &context(), &options)
}

fn completions(model: &Model, retention: CacheRetention, session: &str) -> Value {
    let options = CompletionsOptions {
        api_key: Some("fake-key".into()),
        cache_retention: Some(retention),
        session_id: Some(session.into()),
        ..Default::default()
    };
    let resolved = cortexcode_ai_util::resolve_cache_retention(options.cache_retention);
    completions_params(model, &context(), &options, &get_compat(model), &resolved)
}

// --- Anthropic Provider ---

#[test]
fn anthropic_proxy_base_urls_get_the_1h_ttl() {
    let mut model = catalog("anthropic", "claude-haiku-4-5");
    model.base_url = "https://my-proxy.example.com/v1".into();
    let payload = with_retention_env(Some("long"), || anthropic(&model, None));
    assert_eq!(
        payload["system"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"})
    );
}

#[test]
fn anthropic_omits_the_ttl_when_long_retention_is_unsupported() {
    let mut model = catalog("anthropic", "claude-haiku-4-5");
    model.base_url = "https://my-proxy.example.com/v1".into();
    model.compat = Some(json!({"supportsLongCacheRetention": false}));
    let payload = with_retention_env(None, || anthropic(&model, Some(CacheRetention::Long)));
    assert_eq!(
        payload["system"][0]["cache_control"],
        json!({"type": "ephemeral"})
    );
}

#[test]
fn anthropic_omits_cache_control_when_retention_is_none() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let payload = with_retention_env(None, || anthropic(&model, Some(CacheRetention::None)));
    assert!(payload["system"][0].get("cache_control").is_none());
}

#[test]
fn anthropic_adds_cache_control_to_string_user_messages() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let payload = with_retention_env(None, || anthropic(&model, None));
    let last = payload["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let blocks = last["content"].as_array().expect("array content");
    assert_eq!(
        blocks.last().unwrap()["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"})
    );
}

#[test]
fn anthropic_long_retention_sets_the_1h_ttl() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let payload = with_retention_env(None, || anthropic(&model, Some(CacheRetention::Long)));
    assert_eq!(
        payload["system"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"})
    );
}

#[test]
fn anthropic_env_retention_defaults() {
    // The live-gated TS cases, checked on the payload: unset and `long` -> 1h,
    // `short` -> no ttl.
    let model = catalog("anthropic", "claude-haiku-4-5");
    for (env, expected) in [
        (None, json!({"type": "ephemeral", "ttl": "1h"})),
        (Some("long"), json!({"type": "ephemeral", "ttl": "1h"})),
        (Some("short"), json!({"type": "ephemeral"})),
    ] {
        let payload = with_retention_env(env, || anthropic(&model, None));
        assert_eq!(payload["system"][0]["cache_control"], expected, "{env:?}");
    }
}

// --- OpenAI Responses Provider ---

#[test]
fn responses_proxy_base_urls_get_24h_retention() {
    let mut model = catalog("openai", "gpt-4o-mini");
    model.base_url = "https://my-proxy.example.com/v1".into();
    let payload = with_retention_env(Some("long"), || responses(&model, None, None));
    assert_eq!(payload["prompt_cache_retention"], "24h");
}

#[test]
fn responses_omit_retention_when_long_retention_is_unsupported() {
    let mut model = catalog("openai", "gpt-4o-mini");
    model.compat = Some(json!({"supportsLongCacheRetention": false}));
    let payload = with_retention_env(None, || {
        responses(
            &model,
            Some(CacheRetention::Long),
            Some("session-compat-false"),
        )
    });
    assert!(payload.get("prompt_cache_retention").is_none());
}

#[test]
fn responses_omit_the_cache_key_when_retention_is_none() {
    let model = catalog("openai", "gpt-4o-mini");
    let payload = with_retention_env(None, || {
        responses(&model, Some(CacheRetention::None), Some("session-1"))
    });
    assert!(payload.get("prompt_cache_key").is_none());
    assert!(payload.get("prompt_cache_retention").is_none());
}

#[test]
fn responses_long_retention_sets_key_and_24h() {
    let model = catalog("openai", "gpt-4o-mini");
    let payload = with_retention_env(None, || {
        responses(&model, Some(CacheRetention::Long), Some("session-2"))
    });
    assert_eq!(payload["prompt_cache_key"], "session-2");
    assert_eq!(payload["prompt_cache_retention"], "24h");
}

#[test]
fn responses_env_retention_defaults() {
    let model = catalog("openai", "gpt-4o-mini");
    for (env, expected) in [
        (None, Some("24h")),
        (Some("long"), Some("24h")),
        (Some("short"), None),
    ] {
        let payload = with_retention_env(env, || responses(&model, None, None));
        assert_eq!(
            payload
                .get("prompt_cache_retention")
                .and_then(Value::as_str),
            expected,
            "{env:?}"
        );
    }
}

// --- OpenAI Completions Provider ---

fn completions_model(compat: Option<Value>) -> Model {
    Model {
        id: "test-model".into(),
        name: "Test Model".into(),
        api: "openai-completions".into(),
        provider: "test-openai-completions".into(),
        base_url: "https://my-proxy.example.com/v1".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: Default::default(),
        context_window: 128_000,
        max_tokens: 4096,
        headers: None,
        compat,
    }
}

#[test]
fn completions_proxy_base_urls_get_key_and_24h() {
    let payload = with_retention_env(None, || {
        completions(
            &completions_model(None),
            CacheRetention::Long,
            "session-completions",
        )
    });
    assert_eq!(payload["prompt_cache_key"], "session-completions");
    assert_eq!(payload["prompt_cache_retention"], "24h");
}

#[test]
fn completions_omit_key_and_retention_when_long_retention_is_unsupported() {
    let model = completions_model(Some(json!({"supportsLongCacheRetention": false})));
    let payload = with_retention_env(None, || {
        completions(&model, CacheRetention::Long, "session-completions-false")
    });
    assert!(payload.get("prompt_cache_key").is_none());
    assert!(payload.get("prompt_cache_retention").is_none());
}
