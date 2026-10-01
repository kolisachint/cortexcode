//! Port of hoocode `packages/coding-agent/test/model-resolver.test.ts` (v0.5.89).

use cortexcode_ai_types::{Model, ModelCost, ThinkingLevel};
use cortexcode_code_models::{
    default_model_for_provider, find_initial_model, parse_model_pattern, resolve_cli_model,
    resolve_model_scope, InitialModelOptions, ModelSource,
};

#[allow(clippy::too_many_arguments)]
fn model(
    id: &str,
    name: &str,
    provider: &str,
    base_url: &str,
    reasoning: bool,
    input: &[&str],
    cost: [f64; 4],
    context_window: u64,
    max_tokens: u64,
) -> Model {
    Model {
        id: id.into(),
        name: name.into(),
        api: "anthropic-messages".into(),
        provider: provider.into(),
        base_url: base_url.into(),
        reasoning,
        thinking_level_map: None,
        input: input.iter().map(|s| s.to_string()).collect(),
        cost: ModelCost {
            input: cost[0],
            output: cost[1],
            cache_read: cost[2],
            cache_write: cost[3],
        },
        context_window,
        max_tokens,
        headers: None,
        compat: None,
    }
}

fn mock_models() -> Vec<Model> {
    vec![
        model(
            "claude-sonnet-4-5",
            "Claude Sonnet 4.5",
            "anthropic",
            "https://api.anthropic.com",
            true,
            &["text", "image"],
            [3.0, 15.0, 0.3, 3.75],
            200_000,
            8192,
        ),
        model(
            "gpt-4o",
            "GPT-4o",
            "openai",
            "https://api.openai.com",
            false,
            &["text", "image"],
            [5.0, 15.0, 0.5, 5.0],
            128_000,
            4096,
        ),
    ]
}

fn mock_openrouter_models() -> Vec<Model> {
    vec![
        model(
            "qwen/qwen3-coder:exacto",
            "Qwen3 Coder Exacto",
            "openrouter",
            "https://openrouter.ai/api/v1",
            true,
            &["text"],
            [1.0, 2.0, 0.1, 1.0],
            128_000,
            8192,
        ),
        model(
            "openai/gpt-4o:extended",
            "GPT-4o Extended",
            "openrouter",
            "https://openrouter.ai/api/v1",
            false,
            &["text", "image"],
            [5.0, 15.0, 0.5, 5.0],
            128_000,
            4096,
        ),
    ]
}

fn all_models() -> Vec<Model> {
    let mut all = mock_models();
    all.extend(mock_openrouter_models());
    all
}

fn parse(pattern: &str) -> cortexcode_code_models::ParsedModelResult {
    parse_model_pattern(pattern, &all_models(), true)
}

fn id(result: &cortexcode_code_models::ParsedModelResult) -> Option<&str> {
    result.model.as_ref().map(|m| m.id.as_str())
}

// parseModelPattern: simple patterns without colons

#[test]
fn exact_match_returns_model_with_undefined_thinking_level() {
    let r = parse("claude-sonnet-4-5");
    assert_eq!(id(&r), Some("claude-sonnet-4-5"));
    assert_eq!(r.thinking_level, None);
    assert_eq!(r.warning, None);
}

#[test]
fn partial_match_returns_best_model_with_undefined_thinking_level() {
    let r = parse("sonnet");
    assert_eq!(id(&r), Some("claude-sonnet-4-5"));
    assert_eq!(r.thinking_level, None);
    assert_eq!(r.warning, None);
}

#[test]
fn no_match_returns_undefined_model_and_thinking_level() {
    let r = parse("nonexistent");
    assert_eq!(r.model, None);
    assert_eq!(r.thinking_level, None);
    assert_eq!(r.warning, None);
}

// patterns with valid thinking levels

#[test]
fn sonnet_high_returns_sonnet_with_high_thinking_level() {
    let r = parse("sonnet:high");
    assert_eq!(id(&r), Some("claude-sonnet-4-5"));
    assert_eq!(r.thinking_level, Some(ThinkingLevel::High));
    assert_eq!(r.warning, None);
}

#[test]
fn gpt_4o_medium_returns_gpt_4o_with_medium_thinking_level() {
    let r = parse("gpt-4o:medium");
    assert_eq!(id(&r), Some("gpt-4o"));
    assert_eq!(r.thinking_level, Some(ThinkingLevel::Medium));
    assert_eq!(r.warning, None);
}

#[test]
fn all_valid_thinking_levels_work() {
    for (name, level) in [
        ("off", ThinkingLevel::Off),
        ("minimal", ThinkingLevel::Minimal),
        ("low", ThinkingLevel::Low),
        ("medium", ThinkingLevel::Medium),
        ("high", ThinkingLevel::High),
        ("xhigh", ThinkingLevel::XHigh),
    ] {
        let r = parse(&format!("sonnet:{name}"));
        assert_eq!(id(&r), Some("claude-sonnet-4-5"));
        assert_eq!(r.thinking_level, Some(level));
        assert_eq!(r.warning, None);
    }
}

// patterns with invalid thinking levels

#[test]
fn sonnet_random_returns_sonnet_with_warning() {
    let r = parse("sonnet:random");
    assert_eq!(id(&r), Some("claude-sonnet-4-5"));
    assert_eq!(r.thinking_level, None);
    let w = r.warning.unwrap();
    assert!(w.contains("Invalid thinking level"));
    assert!(w.contains("random"));
}

#[test]
fn gpt_4o_invalid_returns_gpt_4o_with_warning() {
    let r = parse("gpt-4o:invalid");
    assert_eq!(id(&r), Some("gpt-4o"));
    assert_eq!(r.thinking_level, None);
    assert!(r.warning.unwrap().contains("Invalid thinking level"));
}

// OpenRouter models with colons in IDs

#[test]
fn qwen3_coder_exacto_matches_the_model() {
    let r = parse("qwen/qwen3-coder:exacto");
    assert_eq!(id(&r), Some("qwen/qwen3-coder:exacto"));
    assert_eq!(r.thinking_level, None);
    assert_eq!(r.warning, None);
}

#[test]
fn openrouter_prefixed_qwen3_coder_exacto_matches() {
    let r = parse("openrouter/qwen/qwen3-coder:exacto");
    assert_eq!(id(&r), Some("qwen/qwen3-coder:exacto"));
    assert_eq!(r.model.as_ref().unwrap().provider, "openrouter");
    assert_eq!(r.thinking_level, None);
    assert_eq!(r.warning, None);
}

#[test]
fn qwen3_coder_exacto_high_matches_with_high_thinking_level() {
    let r = parse("qwen/qwen3-coder:exacto:high");
    assert_eq!(id(&r), Some("qwen/qwen3-coder:exacto"));
    assert_eq!(r.thinking_level, Some(ThinkingLevel::High));
    assert_eq!(r.warning, None);
}

#[test]
fn openrouter_prefixed_exacto_high_matches_with_provider_and_level() {
    let r = parse("openrouter/qwen/qwen3-coder:exacto:high");
    assert_eq!(id(&r), Some("qwen/qwen3-coder:exacto"));
    assert_eq!(r.model.as_ref().unwrap().provider, "openrouter");
    assert_eq!(r.thinking_level, Some(ThinkingLevel::High));
    assert_eq!(r.warning, None);
}

#[test]
fn gpt_4o_extended_matches_the_extended_model() {
    let r = parse("openai/gpt-4o:extended");
    assert_eq!(id(&r), Some("openai/gpt-4o:extended"));
    assert_eq!(r.thinking_level, None);
    assert_eq!(r.warning, None);
}

// invalid thinking levels with OpenRouter models

#[test]
fn qwen3_coder_exacto_random_returns_model_with_warning() {
    let r = parse("qwen/qwen3-coder:exacto:random");
    assert_eq!(id(&r), Some("qwen/qwen3-coder:exacto"));
    assert_eq!(r.thinking_level, None);
    let w = r.warning.unwrap();
    assert!(w.contains("Invalid thinking level"));
    assert!(w.contains("random"));
}

#[test]
fn qwen3_coder_exacto_high_random_returns_model_with_warning() {
    let r = parse("qwen/qwen3-coder:exacto:high:random");
    assert_eq!(id(&r), Some("qwen/qwen3-coder:exacto"));
    assert_eq!(r.thinking_level, None);
    let w = r.warning.unwrap();
    assert!(w.contains("Invalid thinking level"));
    assert!(w.contains("random"));
}

// edge cases

#[test]
fn empty_pattern_matches_via_partial_matching() {
    let r = parse("");
    assert!(r.model.is_some());
    assert_eq!(r.thinking_level, None);
}

#[test]
fn pattern_ending_with_colon_treats_empty_suffix_as_invalid() {
    let r = parse("sonnet:");
    assert_eq!(id(&r), Some("claude-sonnet-4-5"));
    assert!(r.warning.unwrap().contains("Invalid thinking level"));
}

// dash/dot separator normalization

fn copilot_models() -> Vec<Model> {
    vec![model(
        "claude-haiku-4.5",
        "Claude Haiku 4.5",
        "github-copilot",
        "https://api.githubcopilot.com",
        true,
        &["text"],
        [1.0, 2.0, 0.1, 1.0],
        200_000,
        8192,
    )]
}

#[test]
fn matches_dash_pattern_against_dotted_model_id() {
    let r = parse_model_pattern("claude-haiku-4-5", &copilot_models(), true);
    assert_eq!(id(&r), Some("claude-haiku-4.5"));
}

#[test]
fn matches_dotted_pattern_against_dotted_model_id() {
    let r = parse_model_pattern("claude-haiku-4.5", &copilot_models(), true);
    assert_eq!(id(&r), Some("claude-haiku-4.5"));
}

#[test]
fn still_returns_undefined_for_genuinely_unrelated_patterns() {
    let r = parse_model_pattern("gpt-4o", &copilot_models(), true);
    assert_eq!(r.model, None);
}

// resolveCliModel

fn provider_and_id(r: &cortexcode_code_models::ResolveCliModelResult) -> (String, String) {
    let m = r.model.as_ref().expect("model");
    (m.provider.clone(), m.id.clone())
}

#[test]
fn resolves_model_provider_id_without_provider() {
    let r = resolve_cli_model(None, Some("openai/gpt-4o"), &all_models());
    assert_eq!(r.error, None);
    assert_eq!(provider_and_id(&r), ("openai".into(), "gpt-4o".into()));
}

#[test]
fn resolves_fuzzy_patterns_within_an_explicit_provider() {
    let r = resolve_cli_model(Some("openai"), Some("4o"), &all_models());
    assert_eq!(r.error, None);
    assert_eq!(provider_and_id(&r), ("openai".into(), "gpt-4o".into()));
}

#[test]
fn supports_model_pattern_thinking_without_explicit_thinking() {
    let r = resolve_cli_model(None, Some("sonnet:high"), &all_models());
    assert_eq!(r.error, None);
    assert_eq!(r.model.as_ref().unwrap().id, "claude-sonnet-4-5");
    assert_eq!(r.thinking_level, Some(ThinkingLevel::High));
}

#[test]
fn prefers_exact_model_id_match_over_provider_inference() {
    let r = resolve_cli_model(None, Some("openai/gpt-4o:extended"), &all_models());
    assert_eq!(r.error, None);
    assert_eq!(
        provider_and_id(&r),
        ("openrouter".into(), "openai/gpt-4o:extended".into())
    );
}

#[test]
fn does_not_strip_invalid_suffix_as_thinking_level_in_model() {
    let r = resolve_cli_model(Some("openai"), Some("gpt-4o:extended"), &all_models());
    assert_eq!(r.error, None);
    assert_eq!(
        provider_and_id(&r),
        ("openai".into(), "gpt-4o:extended".into())
    );
}

#[test]
fn allows_custom_model_ids_for_explicit_providers_without_double_prefixing() {
    let r = resolve_cli_model(
        Some("openrouter"),
        Some("openrouter/openai/ghost-model"),
        &all_models(),
    );
    assert_eq!(r.error, None);
    assert_eq!(
        provider_and_id(&r),
        ("openrouter".into(), "openai/ghost-model".into())
    );
}

#[test]
fn returns_a_clear_error_when_there_are_no_models() {
    let r = resolve_cli_model(Some("openai"), Some("gpt-4o"), &[]);
    assert_eq!(r.model, None);
    assert!(r.error.unwrap().contains("No models available"));
}

#[test]
fn prefers_provider_model_split_over_gateway_model_with_matching_id() {
    let mut models = all_models();
    models.push(model(
        "glm-5",
        "GLM-5",
        "zai",
        "https://open.bigmodel.cn/api/paas/v4",
        true,
        &["text"],
        [1.0, 2.0, 0.1, 1.0],
        128_000,
        8192,
    ));
    models.push(model(
        "zai/glm-5",
        "GLM-5",
        "vercel-ai-gateway",
        "https://ai-gateway.vercel.sh",
        true,
        &["text"],
        [1.0, 2.0, 0.1, 1.0],
        128_000,
        8192,
    ));
    let r = resolve_cli_model(None, Some("zai/glm-5"), &models);
    assert_eq!(r.error, None);
    assert_eq!(provider_and_id(&r), ("zai".into(), "glm-5".into()));
}

#[test]
fn resolves_provider_prefixed_fuzzy_patterns() {
    let r = resolve_cli_model(None, Some("openrouter/qwen"), &all_models());
    assert_eq!(r.error, None);
    assert_eq!(
        provider_and_id(&r),
        ("openrouter".into(), "qwen/qwen3-coder:exacto".into())
    );
}

#[test]
fn infers_provider_from_model_prefix_even_when_provider_is_also_set() {
    let without = resolve_cli_model(None, Some("anthropic/claude-sonnet-4-5"), &all_models());
    assert_eq!(without.error, None);
    assert_eq!(
        provider_and_id(&without),
        ("anthropic".into(), "claude-sonnet-4-5".into())
    );

    let conflicting = resolve_cli_model(
        Some("opencode"),
        Some("anthropic/claude-sonnet-4-5"),
        &all_models(),
    );
    assert_eq!(conflicting.model, None);
    assert!(conflicting.error.unwrap().contains("Unknown provider"));
}

// default model selection

#[test]
fn openai_defaults_track_current_models() {
    assert_eq!(default_model_for_provider("openai"), Some("gpt-5.4"));
    assert_eq!(
        default_model_for_provider("openai-codex"),
        Some("gpt-5.6-terra")
    );
}

#[test]
fn zai_minimax_and_cerebras_defaults_track_current_models() {
    assert_eq!(default_model_for_provider("zai"), Some("glm-5.1"));
    assert_eq!(default_model_for_provider("minimax"), Some("MiniMax-M2.7"));
    assert_eq!(
        default_model_for_provider("minimax-cn"),
        Some("MiniMax-M2.7")
    );
    assert_eq!(default_model_for_provider("cerebras"), Some("zai-glm-4.7"));
}

#[test]
fn ai_gateway_default_tracks_current_model() {
    assert_eq!(
        default_model_for_provider("vercel-ai-gateway"),
        Some("zai/glm-5.1")
    );
}

// The release regenerates the catalog first; a default that upstream dropped
// must fail here, not in the publish step.
#[test]
fn fireworks_together_and_opencode_go_defaults_are_in_the_catalog() {
    for provider in ["fireworks", "together", "opencode-go"] {
        let id = default_model_for_provider(provider).unwrap();
        assert!(
            cortexcode_ai_models::get_model(provider, id).is_some(),
            "{provider}/{id}"
        );
    }
}

/// Every default must survive a catalog regeneration (pin bump), not only the
/// three hoocode tests above. `UPSTREAM_STALE` are defaults the pinned hoocode
/// itself names but its catalog no longer has (findInitialModel skips them,
/// buildFallbackModel falls back to the provider's first model); they stay
/// for parity. Drop an entry when hoocode fixes it; any new miss fails here.
#[test]
fn every_provider_default_is_in_the_catalog() {
    const UPSTREAM_STALE: &[&str] = &["cerebras/zai-glm-4.7", "zai/glm-5.1"];
    let missing: Vec<String> = cortexcode_code_models::DEFAULT_MODEL_PER_PROVIDER
        .iter()
        .filter(|(provider, id)| cortexcode_ai_models::get_model(provider, id).is_none())
        .map(|(provider, id)| format!("{provider}/{id}"))
        .collect();
    assert_eq!(missing, UPSTREAM_STALE, "defaults missing from catalog");
}

/// Registry stub: `auth` lists providers with configured auth.
struct Stub {
    all: Vec<Model>,
    available: Vec<Model>,
    auth: Vec<&'static str>,
}

impl ModelSource for Stub {
    fn all_models(&self) -> Vec<Model> {
        self.all.clone()
    }
    fn available_models(&self) -> Vec<Model> {
        self.available.clone()
    }
    fn find_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.all
            .iter()
            .find(|m| m.provider == provider && m.id == model_id)
            .cloned()
    }
    fn model_has_configured_auth(&self, model: &Model) -> bool {
        self.auth.contains(&model.provider.as_str())
    }
}

#[test]
fn find_initial_model_accepts_explicit_provider_custom_model_ids() {
    let stub = Stub {
        all: all_models(),
        available: vec![],
        auth: vec![],
    };
    let r = find_initial_model(
        InitialModelOptions {
            cli_provider: Some("openrouter"),
            cli_model: Some("openrouter/openai/ghost-model"),
            ..Default::default()
        },
        &stub,
    )
    .unwrap();
    let m = r.model.unwrap();
    assert_eq!(m.provider, "openrouter");
    assert_eq!(m.id, "openai/ghost-model");
}

#[test]
fn find_initial_model_selects_ai_gateway_default_when_available() {
    let gateway = model(
        "anthropic/claude-opus-4-6",
        "Claude Opus 4.6",
        "vercel-ai-gateway",
        "https://ai-gateway.vercel.sh",
        true,
        &["text", "image"],
        [5.0, 15.0, 0.5, 5.0],
        200_000,
        8192,
    );
    let stub = Stub {
        all: vec![],
        available: vec![gateway],
        auth: vec![],
    };
    let r = find_initial_model(InitialModelOptions::default(), &stub).unwrap();
    let m = r.model.unwrap();
    assert_eq!(m.provider, "vercel-ai-gateway");
    assert_eq!(m.id, "anthropic/claude-opus-4-6");
}

// findInitialModel saved/default selection

fn authed(providers: Vec<&'static str>) -> Stub {
    Stub {
        all: all_models(),
        available: mock_models()
            .into_iter()
            .filter(|m| providers.contains(&m.provider.as_str()))
            .collect(),
        auth: providers,
    }
}

#[test]
fn honours_a_saved_default_when_that_provider_has_auth() {
    let r = find_initial_model(
        InitialModelOptions {
            default_provider: Some("openai"),
            default_model_id: Some("gpt-4o"),
            ..Default::default()
        },
        &authed(vec!["anthropic", "openai"]),
    )
    .unwrap();
    let m = r.model.unwrap();
    assert_eq!(m.provider, "openai");
    assert_eq!(m.id, "gpt-4o");
}

#[test]
fn skips_a_saved_default_without_auth_and_auto_detects_an_available_provider() {
    let r = find_initial_model(
        InitialModelOptions {
            default_provider: Some("anthropic"),
            default_model_id: Some("claude-sonnet-4-5"),
            ..Default::default()
        },
        &authed(vec!["openai"]),
    )
    .unwrap();
    assert_eq!(r.model.unwrap().provider, "openai");
}

#[test]
fn falls_through_to_no_model_when_nothing_is_authed() {
    let r = find_initial_model(
        InitialModelOptions {
            default_provider: Some("anthropic"),
            default_model_id: Some("claude-sonnet-4-5"),
            ..Default::default()
        },
        &authed(vec![]),
    )
    .unwrap();
    assert_eq!(r.model, None);
}

// Beyond the TS file: resolveModelScope (globs, thinking suffixes, warnings).

#[test]
fn resolve_model_scope_globs_fuzzy_and_warnings() {
    let patterns: Vec<String> = [
        "anthropic/*:high",
        "*gpt*",
        "sonnet",
        "nothing-here",
        "4o:bogus",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let scope = resolve_model_scope(&patterns, &all_models());
    let ids: Vec<(&str, Option<ThinkingLevel>)> = scope
        .models
        .iter()
        .map(|sm| (sm.model.id.as_str(), sm.thinking_level.clone()))
        .collect();
    assert_eq!(
        ids,
        vec![
            ("claude-sonnet-4-5", Some(ThinkingLevel::High)),
            ("gpt-4o", None),
            // "4o" matches both aliases; the highest id sorts first.
            ("openai/gpt-4o:extended", None),
        ]
    );
    assert_eq!(
        scope.warnings,
        vec![
            "Warning: No models match pattern \"nothing-here\"".to_string(),
            "Warning: Invalid thinking level \"bogus\" in pattern \"4o:bogus\". Using default instead."
                .to_string(),
        ]
    );
}

#[test]
fn resolve_model_scope_glob_star_stays_within_a_segment() {
    // `*` does not cross `/`: "openrouter/*" matches the provider's ids only
    // through the provider/id form, and "qwen*" never matches "qwen/qwen3...".
    let scope = resolve_model_scope(&["qwen*".to_string()], &all_models());
    assert!(scope.models.is_empty());
    let scope = resolve_model_scope(&["OPENROUTER/*/*".to_string()], &all_models());
    assert_eq!(scope.models.len(), 2);
}
