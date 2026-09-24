//! Ports of hoocode `test/model-registry.test.ts` (pinned v0.5.89). Test names match
//! the TS `test(...)` titles. Dynamic provider registration and auth.json cases are
//! not ported yet (ledger 10.4b / Phase 12).

use super::*;
use serde_json::json;

struct Env {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    Env { _dir: dir, path }
}

impl Env {
    fn write(&self, providers: Value) {
        std::fs::write(&self.path, json!({ "providers": providers }).to_string()).unwrap();
    }
    fn registry(&self) -> ModelRegistry {
        ModelRegistry::create(&self.path)
    }
}

fn provider_config(base_url: &str, ids: &[&str], api: &str) -> Value {
    json!({
        "baseUrl": base_url,
        "apiKey": "TEST_KEY",
        "api": api,
        "models": ids.iter().map(|id| json!({
            "id": id, "name": id, "reasoning": false, "input": ["text"],
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
            "contextWindow": 100000, "maxTokens": 8000
        })).collect::<Vec<_>>()
    })
}

fn for_provider<'a>(r: &'a ModelRegistry, provider: &str) -> Vec<&'a Model> {
    r.get_all()
        .iter()
        .filter(|m| m.provider == provider)
        .collect()
}

fn other_openrouter_anthropic() -> String {
    cortexcode_ai_models::get_models("openrouter")
        .iter()
        .map(|m| m.id.clone())
        .find(|id| id.starts_with("anthropic/") && id != "anthropic/claude-sonnet-4")
        .expect("catalog has a second OpenRouter Anthropic model")
}

// baseUrl override (no custom models)

#[test]
fn overriding_base_url_keeps_all_built_in_models() {
    let e = env();
    e.write(json!({"anthropic": {"baseUrl": "https://my-proxy.example.com/v1"}}));
    let r = e.registry();
    let models = for_provider(&r, "anthropic");
    assert!(models.len() > 1);
    assert!(models.iter().any(|m| m.id.contains("claude")));
}

#[test]
fn overriding_base_url_changes_url_on_all_built_in_models() {
    let e = env();
    e.write(json!({"anthropic": {"baseUrl": "https://my-proxy.example.com/v1"}}));
    let r = e.registry();
    for m in for_provider(&r, "anthropic") {
        assert_eq!(m.base_url, "https://my-proxy.example.com/v1");
    }
}

#[test]
fn base_url_only_override_does_not_affect_other_providers() {
    let e = env();
    e.write(json!({"anthropic": {"baseUrl": "https://my-proxy.example.com/v1"}}));
    let r = e.registry();
    let google = for_provider(&r, "google");
    assert!(!google.is_empty());
    assert_ne!(google[0].base_url, "https://my-proxy.example.com/v1");
}

#[test]
fn overriding_headers_resolves_at_request_time() {
    let e = env();
    e.write(json!({"anthropic": {"headers": {"X-Proxy": "literal-value"}}}));
    let r = e.registry();
    let model = for_provider(&r, "anthropic")[0].clone();
    let auth = r.get_api_key_and_headers(&model, &NoAuth).unwrap();
    assert_eq!(auth.headers.unwrap()["X-Proxy"], "literal-value");
}

#[test]
fn refresh_picks_up_base_url_override_changes() {
    let e = env();
    e.write(json!({"anthropic": {"baseUrl": "https://first-proxy.example.com/v1"}}));
    let mut r = e.registry();
    assert_eq!(
        for_provider(&r, "anthropic")[0].base_url,
        "https://first-proxy.example.com/v1"
    );
    e.write(json!({"anthropic": {"baseUrl": "https://second-proxy.example.com/v1"}}));
    r.refresh();
    assert_eq!(
        for_provider(&r, "anthropic")[0].base_url,
        "https://second-proxy.example.com/v1"
    );
}

// custom models merge behavior

#[test]
fn built_in_provider_custom_models_inherit_api_and_base_url() {
    let e = env();
    e.write(json!({"openrouter": {"models": [
        {"id": "fake-provider/fake-model", "name": "Fake model", "reasoning": true, "input": ["text"]}
    ]}}));
    let r = e.registry();
    assert!(r.error().is_none(), "{:?}", r.error());
    let m = r.find("openrouter", "fake-provider/fake-model").unwrap();
    assert_eq!(m.api, "openai-completions");
    assert_eq!(m.base_url, "https://openrouter.ai/api/v1");
}

#[test]
fn non_built_in_provider_custom_models_still_require_base_url_and_api_key() {
    let e = env();
    e.write(json!({"my-custom-provider": {"models": [
        {"id": "my-model", "api": "openai-completions", "reasoning": false, "input": ["text"]}
    ]}}));
    assert!(e.registry().error().unwrap().contains("baseUrl"));
}

#[test]
fn custom_provider_with_same_name_as_built_in_merges_with_built_in_models() {
    let e = env();
    e.write(json!({"anthropic": provider_config("https://my-proxy.example.com/v1", &["claude-custom"], "anthropic-messages")}));
    let r = e.registry();
    let models = for_provider(&r, "anthropic");
    assert!(models.len() > 1);
    assert!(models.iter().any(|m| m.id == "claude-custom"));
    assert!(models.iter().any(|m| m.id.contains("claude")));
}

#[test]
fn custom_model_with_same_id_replaces_built_in_model_by_id() {
    let e = env();
    e.write(json!({"openrouter": provider_config("https://my-proxy.example.com/v1", &["anthropic/claude-sonnet-4"], "openai-completions")}));
    let r = e.registry();
    let sonnets: Vec<_> = for_provider(&r, "openrouter")
        .into_iter()
        .filter(|m| m.id == "anthropic/claude-sonnet-4")
        .collect();
    assert_eq!(sonnets.len(), 1);
    assert_eq!(sonnets[0].base_url, "https://my-proxy.example.com/v1");
}

#[test]
fn custom_provider_with_same_name_as_built_in_does_not_affect_other_built_in_providers() {
    let e = env();
    e.write(json!({"anthropic": provider_config("https://my-proxy.example.com/v1", &["claude-custom"], "anthropic-messages")}));
    let r = e.registry();
    assert!(!for_provider(&r, "google").is_empty());
    assert!(!for_provider(&r, "openai").is_empty());
}

#[test]
fn provider_level_base_url_applies_to_both_built_in_and_custom_models() {
    let e = env();
    e.write(json!({"anthropic": provider_config("https://merged-proxy.example.com/v1", &["claude-custom"], "anthropic-messages")}));
    let r = e.registry();
    for m in for_provider(&r, "anthropic") {
        assert_eq!(m.base_url, "https://merged-proxy.example.com/v1");
    }
}

fn demo_provider(model_compat: Option<Value>) -> Value {
    let mut model = json!({
        "id": "demo-model", "reasoning": false, "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 1000, "maxTokens": 100
    });
    if let Some(c) = model_compat {
        model["compat"] = c;
    }
    json!({"demo": {
        "baseUrl": "https://example.com/v1", "apiKey": "DEMO_KEY", "api": "openai-completions",
        "compat": {"supportsUsageInStreaming": false, "maxTokensField": "max_tokens"},
        "models": [model]
    }})
}

#[test]
fn provider_level_compat_applies_to_custom_models() {
    let e = env();
    e.write(demo_provider(None));
    let r = e.registry();
    let compat = r
        .find("demo", "demo-model")
        .unwrap()
        .compat
        .clone()
        .unwrap();
    assert_eq!(compat["supportsUsageInStreaming"], false);
    assert_eq!(compat["maxTokensField"], "max_tokens");
}

#[test]
fn model_level_compat_overrides_provider_level_compat_for_custom_models() {
    let e = env();
    e.write(demo_provider(Some(
        json!({"supportsUsageInStreaming": true, "maxTokensField": "max_completion_tokens"}),
    )));
    let r = e.registry();
    let compat = r
        .find("demo", "demo-model")
        .unwrap()
        .compat
        .clone()
        .unwrap();
    assert_eq!(compat["supportsUsageInStreaming"], true);
    assert_eq!(compat["maxTokensField"], "max_completion_tokens");
}

#[test]
fn provider_level_compat_applies_to_built_in_models() {
    let e = env();
    e.write(json!({"openrouter": {"compat": {"supportsUsageInStreaming": false, "supportsStrictMode": false}}}));
    let r = e.registry();
    let models = for_provider(&r, "openrouter");
    assert!(!models.is_empty());
    for m in models {
        let c = m.compat.clone().unwrap();
        assert_eq!(c["supportsUsageInStreaming"], false);
        assert_eq!(c["supportsStrictMode"], false);
    }
}

#[test]
fn model_level_base_url_overrides_provider_level_base_url_for_custom_models() {
    let e = env();
    let mut p = provider_config(
        "https://provider.example.com/v1",
        &["m1", "m2"],
        "openai-completions",
    );
    p["models"][1]["baseUrl"] = json!("https://model.example.com/v1");
    e.write(json!({"demo": p}));
    let r = e.registry();
    assert_eq!(
        r.find("demo", "m1").unwrap().base_url,
        "https://provider.example.com/v1"
    );
    assert_eq!(
        r.find("demo", "m2").unwrap().base_url,
        "https://model.example.com/v1"
    );
}

#[test]
fn custom_model_defaults_match_parse_models() {
    let e = env();
    e.write(
        json!({"mock": {"baseUrl": "http://127.0.0.1:1/v1", "api": "openai-completions",
        "apiKey": "mock-key", "models": [{"id": "mock-model"}]}}),
    );
    let r = e.registry();
    let m = r.find("mock", "mock-model").unwrap();
    assert_eq!(m.name, "mock-model");
    assert_eq!(m.input, vec!["text".to_string()]);
    assert_eq!((m.context_window, m.max_tokens), (128_000, 16_384));
    assert!(!m.reasoning);
    assert_eq!(m.cost, ModelCost::default());
}

// modelOverrides (per-model customization)

#[test]
fn model_override_applies_to_a_single_built_in_model() {
    let e = env();
    e.write(json!({"openrouter": {"modelOverrides": {"anthropic/claude-sonnet-4": {"name": "Custom Sonnet Name"}}}}));
    let r = e.registry();
    assert_eq!(
        r.find("openrouter", "anthropic/claude-sonnet-4")
            .unwrap()
            .name,
        "Custom Sonnet Name"
    );
    let other = r.find("openrouter", &other_openrouter_anthropic()).unwrap();
    assert_ne!(other.name, "Custom Sonnet Name");
}

#[test]
fn model_override_with_compat_open_router_routing() {
    let e = env();
    e.write(
        json!({"openrouter": {"modelOverrides": {"anthropic/claude-sonnet-4": {
        "compat": {"openRouterRouting": {"only": ["amazon-bedrock"]}}}}}}),
    );
    let r = e.registry();
    let c = r
        .find("openrouter", "anthropic/claude-sonnet-4")
        .unwrap()
        .compat
        .clone()
        .unwrap();
    assert_eq!(c["openRouterRouting"], json!({"only": ["amazon-bedrock"]}));
}

#[test]
fn multiple_model_overrides_on_same_provider() {
    let other = other_openrouter_anthropic();
    let e = env();
    let mut overrides = serde_json::Map::new();
    overrides.insert(
        "anthropic/claude-sonnet-4".into(),
        json!({"compat": {"openRouterRouting": {"only": ["amazon-bedrock"]}}}),
    );
    overrides.insert(
        other.clone(),
        json!({"compat": {"openRouterRouting": {"only": ["anthropic"]}}}),
    );
    e.write(json!({"openrouter": {"modelOverrides": overrides}}));
    let r = e.registry();
    let c1 = r
        .find("openrouter", "anthropic/claude-sonnet-4")
        .unwrap()
        .compat
        .clone()
        .unwrap();
    let c2 = r
        .find("openrouter", &other)
        .unwrap()
        .compat
        .clone()
        .unwrap();
    assert_eq!(c1["openRouterRouting"], json!({"only": ["amazon-bedrock"]}));
    assert_eq!(c2["openRouterRouting"], json!({"only": ["anthropic"]}));
}

#[test]
fn model_override_combined_with_base_url_override() {
    let e = env();
    e.write(
        json!({"openrouter": {"baseUrl": "https://my-proxy.example.com/v1",
        "modelOverrides": {"anthropic/claude-sonnet-4": {"name": "Proxied Sonnet"}}}}),
    );
    let r = e.registry();
    let s = r.find("openrouter", "anthropic/claude-sonnet-4").unwrap();
    assert_eq!(s.base_url, "https://my-proxy.example.com/v1");
    assert_eq!(s.name, "Proxied Sonnet");
    let o = r.find("openrouter", &other_openrouter_anthropic()).unwrap();
    assert_eq!(o.base_url, "https://my-proxy.example.com/v1");
    assert_ne!(o.name, "Proxied Sonnet");
}

#[test]
fn model_override_for_non_existent_model_id_is_ignored() {
    let e = env();
    e.write(json!({"openrouter": {"modelOverrides": {"nonexistent/model-id": {"name": "x"}}}}));
    let r = e.registry();
    assert!(r.find("openrouter", "nonexistent/model-id").is_none());
    assert!(r.error().is_none());
}

#[test]
fn model_override_can_change_cost_fields_partially() {
    let e = env();
    e.write(json!({"openrouter": {"modelOverrides": {"anthropic/claude-sonnet-4": {"cost": {"input": 99}}}}}));
    let r = e.registry();
    let base = cortexcode_ai_models::get_model("openrouter", "anthropic/claude-sonnet-4").unwrap();
    let m = r.find("openrouter", "anthropic/claude-sonnet-4").unwrap();
    assert_eq!(m.cost.input, 99.0);
    assert_eq!(m.cost.output, base.cost.output);
}

#[test]
fn model_override_can_add_headers_at_request_time() {
    let e = env();
    e.write(json!({"openrouter": {"modelOverrides": {"anthropic/claude-sonnet-4": {"headers": {"X-Model": "v"}}}}}));
    let r = e.registry();
    let m = r
        .find("openrouter", "anthropic/claude-sonnet-4")
        .unwrap()
        .clone();
    let auth = r.get_api_key_and_headers(&m, &NoAuth).unwrap();
    assert_eq!(auth.headers.unwrap()["X-Model"], "v");
}

// Parsing

#[test]
fn comments_and_trailing_commas_are_accepted() {
    let e = env();
    std::fs::write(
        &e.path,
        "{\n // local models\n \"providers\": {\"mock\": {\"baseUrl\": \"http://x//y\", \"api\": \"openai-completions\",\n \"apiKey\": \"k\", \"models\": [{\"id\": \"m\",},],},},\n}",
    )
    .unwrap();
    let r = e.registry();
    assert!(r.error().is_none(), "{:?}", r.error());
    assert_eq!(r.find("mock", "m").unwrap().base_url, "http://x//y");
}

#[test]
fn invalid_json_reports_error_and_keeps_built_ins() {
    let e = env();
    std::fs::write(&e.path, "{ not json").unwrap();
    let r = e.registry();
    assert!(r
        .error()
        .unwrap()
        .starts_with("Failed to parse models.json"));
    assert!(!for_provider(&r, "anthropic").is_empty());
}

// API key resolution

fn key_registry(e: &Env, api_key: &str) -> ModelRegistry {
    e.write(
        json!({"custom-provider": {"baseUrl": "https://example.com/v1", "apiKey": api_key,
        "api": "anthropic-messages", "models": [{"id": "test-model"}]}}),
    );
    e.registry()
}

fn key_of(r: &ModelRegistry) -> Option<String> {
    let m = r.find("custom-provider", "test-model").unwrap().clone();
    r.get_api_key_and_headers(&m, &NoAuth)
        .ok()
        .and_then(|a| a.api_key)
}

#[cfg(unix)]
#[test]
fn api_key_with_bang_prefix_executes_command_and_uses_trimmed_stdout() {
    let e = env();
    assert_eq!(
        key_of(&key_registry(&e, "!echo '  test-api-key-from-command  '")).as_deref(),
        Some("test-api-key-from-command")
    );
    assert_eq!(
        key_of(&key_registry(&e, "!printf 'line1\\nline2'")).as_deref(),
        Some("line1\nline2")
    );
    assert_eq!(
        key_of(&key_registry(&e, "!echo 'hello world' | tr ' ' '-'")).as_deref(),
        Some("hello-world")
    );
}

#[cfg(unix)]
#[test]
fn api_key_with_bang_prefix_fails_on_command_failure_or_empty_output() {
    let e = env();
    for cmd in ["!exit 1", "!nonexistent-command-12345", "!printf ''"] {
        let r = key_registry(&e, cmd);
        let m = r.find("custom-provider", "test-model").unwrap().clone();
        let err = r.get_api_key_and_headers(&m, &NoAuth).unwrap_err();
        assert!(err.contains("from shell command"), "{cmd}: {err}");
    }
}

#[test]
fn api_key_as_environment_variable_name_resolves_to_env_value() {
    let e = env();
    std::env::set_var("CORTEX_TEST_MODEL_REGISTRY_KEY", "value-from-env");
    assert_eq!(
        key_of(&key_registry(&e, "CORTEX_TEST_MODEL_REGISTRY_KEY")).as_deref(),
        Some("value-from-env")
    );
    std::env::remove_var("CORTEX_TEST_MODEL_REGISTRY_KEY");
}

#[test]
fn api_key_as_literal_value_is_used_directly_when_not_an_env_var() {
    let e = env();
    assert_eq!(
        key_of(&key_registry(&e, "sk-literal-key-123")).as_deref(),
        Some("sk-literal-key-123")
    );
}

#[test]
fn auth_header_adds_bearer_authorization() {
    let e = env();
    e.write(
        json!({"custom-provider": {"baseUrl": "https://example.com/v1", "apiKey": "k1",
        "authHeader": true, "api": "openai-completions", "models": [{"id": "test-model"}]}}),
    );
    let r = e.registry();
    let m = r.find("custom-provider", "test-model").unwrap().clone();
    let auth = r.get_api_key_and_headers(&m, &NoAuth).unwrap();
    assert_eq!(auth.headers.unwrap()["Authorization"], "Bearer k1");
}

#[test]
fn stored_credentials_take_precedence_over_models_json_key() {
    struct Stored;
    impl AuthLookup for Stored {
        fn api_key(&self, _p: &str) -> Option<String> {
            Some("stored".into())
        }
    }
    let e = env();
    let r = key_registry(&e, "sk-literal");
    let m = r.find("custom-provider", "test-model").unwrap().clone();
    assert_eq!(
        r.get_api_key_and_headers(&m, &Stored)
            .unwrap()
            .api_key
            .as_deref(),
        Some("stored")
    );
}
