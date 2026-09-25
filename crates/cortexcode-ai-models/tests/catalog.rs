//! Catalog assertions from hoocode `packages/ai/test/claude-5-models.test.ts`,
//! `fireworks-models.test.ts` and `together-models.test.ts`. Their request-format
//! cases are provider work (ledger 8.5) and the env-key cases are 8.2.

use cortexcode_ai_models::{get_model, get_models, get_providers, get_supported_thinking_levels};
use cortexcode_ai_types::{
    AnthropicMessagesCompat, Model, ModelCost, OpenAICompletionsCompat, ThinkingLevel,
};
use serde_json::json;

fn model(provider: &str, id: &str) -> &'static Model {
    get_model(provider, id).unwrap_or_else(|| panic!("{provider}/{id} is in the catalog"))
}

fn cost(input: f64, output: f64, cache_read: f64, cache_write: f64) -> ModelCost {
    ModelCost {
        input,
        output,
        cache_read,
        cache_write,
    }
}

fn supports_xhigh(m: &Model) -> bool {
    get_supported_thinking_levels(m).contains(&ThinkingLevel::XHigh)
}

#[test]
fn catalog_matches_the_pin() {
    let providers = get_providers();
    let total: usize = providers.iter().map(|p| get_models(p).len()).sum();
    assert_eq!(total, 1224);
    // hoocode's provider order (Object.entries(MODELS)).
    assert_eq!(
        &providers[..3],
        ["anthropic", "azure-openai-responses", "cerebras"]
    );
    assert_eq!(providers.last(), Some(&"zai"));
}

#[test]
fn claude_5_models_on_anthropic() {
    for (id, c) in [
        ("claude-opus-5", cost(5.0, 25.0, 0.5, 6.25)),
        ("claude-sonnet-5", cost(2.0, 10.0, 0.2, 2.5)),
        ("claude-fable-5", cost(10.0, 50.0, 1.0, 12.5)),
    ] {
        let m = model("anthropic", id);
        assert_eq!(m.api, "anthropic-messages");
        assert_eq!(m.provider, "anthropic");
        assert_eq!(m.base_url, "https://api.anthropic.com");
        assert!(m.reasoning);
        assert_eq!(m.input, ["text", "image"]);
        assert_eq!(m.context_window, 1_000_000);
        assert_eq!(m.max_tokens, 128_000);
        assert_eq!(m.cost, c, "{id}");
        assert!(supports_xhigh(m), "{id}");
    }
}

#[test]
fn claude_5_models_on_github_copilot_use_anthropic_messages() {
    for (id, max_tokens, c) in [
        ("claude-opus-5", 64_000, cost(5.0, 25.0, 0.5, 6.25)),
        ("claude-sonnet-5", 128_000, cost(2.0, 10.0, 0.2, 2.5)),
        ("claude-fable-5", 128_000, cost(10.0, 50.0, 1.0, 12.5)),
    ] {
        let m = model("github-copilot", id);
        assert_eq!(m.api, "anthropic-messages", "{id}");
        assert_eq!(m.base_url, "https://api.individual.githubcopilot.com");
        assert!(m.reasoning);
        assert_eq!(m.input, ["text", "image"]);
        assert_eq!(m.context_window, 1_000_000);
        assert_eq!(m.max_tokens, max_tokens, "{id}");
        assert_eq!(m.cost, c, "{id}");
        assert_eq!(
            m.headers
                .as_ref()
                .and_then(|h| h.get("Copilot-Integration-Id"))
                .map(String::as_str),
            Some("vscode-chat")
        );
        // openai-completions compat must not leak onto an Anthropic Messages model.
        assert!(m.compat.is_none(), "{id}");
        assert!(supports_xhigh(m), "{id}");
    }
    for provider in ["anthropic", "github-copilot"] {
        assert!(get_models(provider).iter().any(|m| m.id == "claude-opus-5"));
    }
}

#[test]
fn openrouter_rolling_claude_aliases() {
    for id in [
        "~anthropic/claude-opus-latest",
        "~anthropic/claude-sonnet-latest",
        "~anthropic/claude-fable-latest",
    ] {
        assert!(supports_xhigh(model("openrouter", id)), "{id}");
    }
    assert!(!supports_xhigh(model(
        "openrouter",
        "~anthropic/claude-haiku-latest"
    )));
}

#[test]
fn fireworks_models() {
    let m = model("fireworks", "accounts/fireworks/models/kimi-k2p6");
    assert_eq!(m.api, "anthropic-messages");
    assert_eq!(m.provider, "fireworks");
    assert_eq!(m.base_url, "https://api.fireworks.ai/inference");
    assert!(m.reasoning);
    assert_eq!(m.input, ["text", "image"]);
    assert_eq!(m.context_window, 262_000);
    assert_eq!(m.max_tokens, 262_000);
    assert_eq!(m.cost, cost(0.95, 4.0, 0.16, 0.0));

    let turbo = model("fireworks", "accounts/fireworks/routers/kimi-k3-fast");
    assert_eq!(turbo.api, "anthropic-messages");
    assert_eq!(turbo.base_url, "https://api.fireworks.ai/inference");
    assert_eq!(turbo.input, ["text", "image"]);
}

#[test]
fn together_default_kimi_model() {
    let m = model("together", "moonshotai/Kimi-K2.6");
    assert_eq!(m.api, "openai-completions");
    assert_eq!(m.base_url, "https://api.together.ai/v1");
    assert!(m.reasoning);
    assert_eq!(
        serde_json::to_value(&m.thinking_level_map).unwrap(),
        json!({"minimal": null, "low": null, "medium": null})
    );
    assert_eq!(m.input, ["text", "image"]);
    assert_eq!(m.context_window, 262_144);
    assert_eq!(m.max_tokens, 131_000);
    assert_eq!(m.cost, cost(1.2, 4.5, 0.2, 0.0));
    assert_eq!(
        m.compat,
        Some(json!({
            "supportsStore": false,
            "supportsDeveloperRole": false,
            "supportsReasoningEffort": false,
            "maxTokensField": "max_tokens",
            "thinkingFormat": "together",
            "supportsStrictMode": false,
            "supportsLongCacheRetention": false,
        }))
    );
    let compat: OpenAICompletionsCompat = m.compat_as();
    assert_eq!(compat.max_tokens_field.as_deref(), Some("max_tokens"));
    assert_eq!(compat.supports_strict_mode, Some(false));
    assert_eq!(compat.supports_usage_in_streaming, None);
}

#[test]
fn together_reasoning_controls() {
    let map = |m: &Model| serde_json::to_value(&m.thinking_level_map).unwrap();

    let gpt_oss = model("together", "openai/gpt-oss-120b");
    assert_eq!(map(gpt_oss), json!({"off": null, "minimal": null}));
    let compat: OpenAICompletionsCompat = gpt_oss.compat_as();
    assert_eq!(compat.supports_reasoning_effort, Some(true));
    assert_eq!(compat.thinking_format.as_deref(), Some("openai"));

    let deepseek = model("together", "deepseek-ai/DeepSeek-V4-Pro");
    assert_eq!(
        map(deepseek),
        json!({"minimal": null, "low": null, "medium": null, "high": "high", "xhigh": null})
    );
    let compat: OpenAICompletionsCompat = deepseek.compat_as();
    assert_eq!(compat.supports_reasoning_effort, Some(true));
    assert_eq!(compat.thinking_format.as_deref(), Some("together"));

    let minimax = model("together", "MiniMaxAI/MiniMax-M2.7");
    assert_eq!(
        map(minimax),
        json!({"off": null, "minimal": null, "low": null, "medium": null})
    );
    let compat: OpenAICompletionsCompat = minimax.compat_as();
    assert_eq!(compat.thinking_format, None);
    assert_eq!(compat.supports_reasoning_effort, Some(false));
}

#[test]
fn models_round_trip_through_the_hoocode_json_shape() {
    let m = model("together", "moonshotai/Kimi-K2.6");
    let value = serde_json::to_value(m).unwrap();
    assert_eq!(value["baseUrl"], "https://api.together.ai/v1");
    assert_eq!(value["cost"]["cacheRead"], 0.2);
    let back: Model = serde_json::from_value(value).unwrap();
    assert_eq!(&back, m);
    // An Anthropic model reads an empty compat view.
    let anthropic: AnthropicMessagesCompat = model("anthropic", "claude-opus-5").compat_as();
    assert_eq!(anthropic, AnthropicMessagesCompat::default());
}

// --- supports-xhigh.test.ts ---

#[test]
fn supported_thinking_levels_include_xhigh_where_hoocode_does() {
    for (provider, id) in [
        ("anthropic", "claude-opus-4-6"),
        ("anthropic", "claude-opus-4-7"),
        ("openai-codex", "gpt-5.4"),
        ("openai-codex", "gpt-5.5"),
        ("openrouter", "anthropic/claude-opus-4.6"),
    ] {
        assert!(supports_xhigh(model(provider, id)), "{provider}/{id}");
    }
    assert!(!supports_xhigh(model("anthropic", "claude-sonnet-4-5")));
}

#[test]
fn deepseek_v4_flash_offers_only_off_high_and_xhigh() {
    for (provider, id) in [
        ("deepseek", "deepseek-v4-flash"),
        ("opencode-go", "deepseek-v4-flash"),
        ("openrouter", "deepseek/deepseek-v4-flash"),
    ] {
        assert_eq!(
            get_supported_thinking_levels(model(provider, id)),
            [
                ThinkingLevel::Off,
                ThinkingLevel::High,
                ThinkingLevel::XHigh
            ],
            "{provider}/{id}"
        );
    }
}
