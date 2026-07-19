//! LLM model registry and discovery for cortex AI.
//!
//! Provides a registry of known LLM models with their capabilities, pricing,
//! and provider metadata. Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `models.ts` + `models.generated.ts`.
//!
//! Models are loaded lazily from an embedded data file (`data/models.json`)
//! on first access.

use std::collections::HashMap;
use std::sync::OnceLock;

pub use cortexcode_ai_types::Model;

// ---------------------------------------------------------------------------
// Registry type: provider -> (model_id -> Model)
// ---------------------------------------------------------------------------

type Registry = HashMap<String, HashMap<String, Model>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        // Start with built-in models from embedded JSON.
        let data = include_str!("../data/models.json");
        let model_list: Vec<serde_json::Value> =
            serde_json::from_str(data).expect("embedded data/models.json is valid JSON");
        build_registry_from_json(&model_list)
    })
}

fn build_registry_from_json(model_list: &[serde_json::Value]) -> Registry {
    let mut registry: Registry = HashMap::new();

    for entry in model_list {
        let provider = entry["provider"]
            .as_str()
            .expect("model entry must have 'provider'")
            .to_string();

        let model = Model {
            id: entry["id"].as_str().unwrap_or_default().to_string(),
            name: entry["name"].as_str().unwrap_or_default().to_string(),
            api: entry["api"].as_str().unwrap_or("unknown").to_string(),
            provider: provider.clone(),
            base_url: entry["baseUrl"].as_str().unwrap_or_default().to_string(),
            reasoning: entry["reasoning"].as_bool().unwrap_or(false),
            thinking_level_map: parse_thinking_level_map(entry),
            input: parse_input_modalities(entry),
            cost: cortexcode_ai_types::ModelCost {
                input: entry["cost"]["input"].as_f64().unwrap_or(0.0),
                output: entry["cost"]["output"].as_f64().unwrap_or(0.0),
                cache_read: entry["cost"]["cacheRead"].as_f64().unwrap_or(0.0),
                cache_write: entry["cost"]["cacheWrite"].as_f64().unwrap_or(0.0),
            },
            context_window: entry["contextWindow"].as_u64().unwrap_or(4096),
            max_tokens: entry["maxTokens"].as_u64().unwrap_or(4096),
            headers: parse_headers(entry),
        };

        registry
            .entry(provider)
            .or_default()
            .insert(model.id.clone(), model);
    }

    registry
}

fn parse_thinking_level_map(
    entry: &serde_json::Value,
) -> Option<HashMap<String, serde_json::Value>> {
    let map = entry.get("thinkingLevelMap")?;
    if map.is_null() {
        return None;
    }
    let map_obj = map.as_object()?;
    let mut result = HashMap::new();
    for (k, v) in map_obj {
        result.insert(k.clone(), v.clone());
    }
    Some(result)
}

fn parse_input_modalities(entry: &serde_json::Value) -> Vec<String> {
    entry["input"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| vec!["text".to_string()])
}

fn parse_headers(entry: &serde_json::Value) -> Option<HashMap<String, String>> {
    let headers = entry.get("headers")?;
    if headers.is_null() {
        return None;
    }
    let obj = headers.as_object()?;
    let mut result = HashMap::new();
    for (k, v) in obj {
        if let Some(val) = v.as_str() {
            result.insert(k.clone(), val.to_string());
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get a specific model by provider and model ID.
pub fn get_model(provider: &str, model_id: &str) -> Option<&'static Model> {
    registry().get(provider)?.get(model_id)
}

/// List all known provider names.
pub fn get_providers() -> Vec<&'static str> {
    registry().keys().map(|s| s.as_str()).collect()
}

/// List all models for a given provider.
pub fn get_models(provider: &str) -> Vec<&'static Model> {
    registry()
        .get(provider)
        .map(|models| models.values().collect())
        .unwrap_or_default()
}

/// Calculate the monetary cost for a given model and usage.
///
/// Cost is computed as: `model_cost_per_million * usage_tokens / 1_000_000`.
pub fn calculate_cost(
    model: &Model,
    usage: &cortexcode_ai_types::Usage,
) -> cortexcode_ai_types::Cost {
    let input = (model.cost.input / 1_000_000.0) * usage.input as f64;
    let output = (model.cost.output / 1_000_000.0) * usage.output as f64;
    let cache_read = (model.cost.cache_read / 1_000_000.0) * usage.cache_read as f64;
    let cache_write = (model.cost.cache_write / 1_000_000.0) * usage.cache_write as f64;
    let total = input + output + cache_read + cache_write;

    cortexcode_ai_types::Cost {
        input,
        output,
        cache_read,
        cache_write,
        total,
    }
}

/// Ordered list of all thinking levels.
const EXTENDED_THINKING_LEVELS: &[cortexcode_ai_types::ThinkingLevel] = &[
    cortexcode_ai_types::ThinkingLevel::Off,
    cortexcode_ai_types::ThinkingLevel::Minimal,
    cortexcode_ai_types::ThinkingLevel::Low,
    cortexcode_ai_types::ThinkingLevel::Medium,
    cortexcode_ai_types::ThinkingLevel::High,
    cortexcode_ai_types::ThinkingLevel::XHigh,
];

/// Get the thinking levels supported by a given model.
///
/// If the model does not support reasoning (`model.reasoning == false`),
/// only `Off` is returned.
pub fn get_supported_thinking_levels(model: &Model) -> Vec<cortexcode_ai_types::ThinkingLevel> {
    if !model.reasoning {
        return vec![cortexcode_ai_types::ThinkingLevel::Off];
    }

    EXTENDED_THINKING_LEVELS
        .iter()
        .filter(|level| {
            let level_str = level_to_str(level);
            let mapped = model
                .thinking_level_map
                .as_ref()
                .and_then(|map| map.get(level_str));

            // null marks the level as unsupported.
            if let Some(val) = mapped {
                return !val.is_null();
            }

            // xhigh requires an explicit mapping to be considered supported.
            // All other levels are supported by default.
            level_str != "xhigh"
        })
        .cloned()
        .collect()
}

/// Clamp a requested thinking level to the nearest supported level.
///
/// If the requested level is supported, it is returned as-is.
/// Otherwise, the function first tries higher levels, then lower levels.
/// Falls back to `Off` if no level is supported.
pub fn clamp_thinking_level(
    model: &Model,
    level: &cortexcode_ai_types::ThinkingLevel,
) -> cortexcode_ai_types::ThinkingLevel {
    let available = get_supported_thinking_levels(model);
    if available.contains(level) {
        return level.clone();
    }

    let Some(idx) = EXTENDED_THINKING_LEVELS.iter().position(|l| l == level) else {
        return available
            .first()
            .cloned()
            .unwrap_or(cortexcode_ai_types::ThinkingLevel::Off);
    };

    // Try higher levels first.
    for candidate in &EXTENDED_THINKING_LEVELS[idx..] {
        if available.contains(candidate) {
            return candidate.clone();
        }
    }

    // Then try lower levels.
    for candidate in EXTENDED_THINKING_LEVELS[..idx].iter().rev() {
        if available.contains(candidate) {
            return candidate.clone();
        }
    }

    available
        .first()
        .cloned()
        .unwrap_or(cortexcode_ai_types::ThinkingLevel::Off)
}

/// Check if two models are equal by comparing both their ID and provider.
///
/// Returns `false` if either model reference is `None`.
pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.id == b.id && a.provider == b.provider,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn level_to_str(level: &cortexcode_ai_types::ThinkingLevel) -> &'static str {
    use cortexcode_ai_types::ThinkingLevel::*;
    match level {
        Off => "off",
        Minimal => "minimal",
        Low => "low",
        Medium => "medium",
        High => "high",
        XHigh => "xhigh",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::{Cost, ThinkingLevel, Usage};

    #[test]
    fn test_get_providers() {
        let providers = get_providers();
        assert!(
            providers.contains(&"anthropic"),
            "providers should include anthropic"
        );
    }

    #[test]
    fn test_get_known_model() {
        let model = get_model("anthropic", "claude-haiku-4-5");
        assert!(model.is_some(), "claude-haiku-4-5 should exist");
        let model = model.unwrap();
        assert_eq!(model.provider, "anthropic");
        assert_eq!(model.api, "anthropic-messages");
    }

    #[test]
    fn test_get_unknown_model() {
        assert!(get_model("nonexistent", "model").is_none());
    }

    #[test]
    fn test_get_models_for_provider() {
        let models = get_models("openai");
        assert!(!models.is_empty(), "openai should have models");
    }

    #[test]
    fn test_calculate_cost() {
        let model = get_model("anthropic", "claude-haiku-4-5").expect("claude-haiku-4-5 exists");
        let usage = Usage {
            input: 1_000_000,
            output: 500_000,
            cache_read: 200_000,
            cache_write: 100_000,
            total_tokens: 1_800_000,
            cost: Cost::default(),
        };
        let cost = calculate_cost(model, &usage);

        // claude-haiku-4-5: input=$1, output=$5, cache_read=$0.10, cache_write=$1.25 per million
        assert!((cost.input - 1.0).abs() < 0.001);
        assert!((cost.output - 2.5).abs() < 0.001);
        assert!((cost.cache_read - 0.02).abs() < 0.001);
        assert!((cost.cache_write - 0.125).abs() < 0.001);
    }

    #[test]
    fn test_reasoning_model_supports_levels() {
        // gpt-5 via azure-openai-responses has reasoning=true and thinkingLevelMap={off: null}
        // so off is excluded, xhigh is excluded (undefined), but minimal through high are supported
        let model = get_model("azure-openai-responses", "gpt-5").expect("gpt-5 exists");
        let levels = get_supported_thinking_levels(model);
        assert!(!levels.contains(&ThinkingLevel::Off));
        assert!(levels.contains(&ThinkingLevel::Minimal));
        assert!(!levels.contains(&ThinkingLevel::XHigh));
    }

    #[test]
    fn test_non_reasoning_model_only_off() {
        let model = get_model("openai", "gpt-4").expect("gpt-4 exists");
        let levels = get_supported_thinking_levels(model);
        assert_eq!(levels, vec![ThinkingLevel::Off]);
    }

    #[test]
    fn test_reasoning_model_with_no_map() {
        // claude-haiku-4-5 has reasoning=true and no thinkingLevelMap.
        // Per TypeScript logic, xhigh requires explicit mapping so it's excluded.
        // Levels off through high are supported by default.
        let model = get_model("anthropic", "claude-haiku-4-5").expect("claude-haiku-4-5 exists");
        let levels = get_supported_thinking_levels(model);
        assert!(levels.contains(&ThinkingLevel::Off));
        assert!(levels.contains(&ThinkingLevel::Minimal));
        assert!(levels.contains(&ThinkingLevel::High));
        assert!(!levels.contains(&ThinkingLevel::XHigh));
        assert_eq!(levels.len(), 5); // off, minimal, low, medium, high
    }

    #[test]
    fn test_clamp_level_exact() {
        let model = get_model("anthropic", "claude-haiku-4-5").expect("claude-haiku-4-5 exists");
        assert_eq!(
            clamp_thinking_level(model, &ThinkingLevel::Minimal),
            ThinkingLevel::Minimal
        );
    }

    #[test]
    fn test_clamp_level_to_nearest() {
        // gpt-5 via azure-openai-responses has reasoning=true and xhigh not in thinkingLevelMap
        // so xhigh should be unsupported, clamp should pick nearest supported (high)
        let model = get_model("azure-openai-responses", "gpt-5").expect("gpt-5 exists");
        assert_eq!(
            clamp_thinking_level(model, &ThinkingLevel::XHigh),
            ThinkingLevel::High
        );
    }

    #[test]
    fn test_models_are_equal() {
        let a = get_model("anthropic", "claude-haiku-4-5");
        let b = get_model("anthropic", "claude-haiku-4-5");
        assert!(models_are_equal(a, b));
        assert!(!models_are_equal(a, get_model("openai", "gpt-4")));
    }

    #[test]
    fn test_models_are_equal_none() {
        assert!(!models_are_equal(None, None));
        let m = get_model("anthropic", "claude-haiku-4-5");
        assert!(!models_are_equal(m, None));
    }

    #[test]
    fn test_get_providers_coverage() {
        let providers = get_providers();
        for p in &["anthropic", "openai", "google", "deepseek", "together"] {
            assert!(providers.contains(p), "expected provider '{p}' in registry");
        }
    }
}
