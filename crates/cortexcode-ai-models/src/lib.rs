//! LLM model registry for cortex AI.
//!
//! Port of hoocode `packages/ai/src/models.ts`. The catalog itself is data in
//! `cortexcode-ai-models-catalog` (`models.generated.ts` at the pin); this
//! crate keeps the lookup logic. Providers and each provider's models keep
//! hoocode's order (`getProviders()` / `getModels()` iterate insertion order).

use std::collections::HashMap;
use std::sync::OnceLock;

pub use cortexcode_ai_types::Model;

/// provider -> models, in catalog order, plus an index for lookups.
struct Registry {
    providers: Vec<(String, Vec<Model>)>,
    index: HashMap<(String, String), (usize, usize)>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let models: Vec<Model> = serde_json::from_str(cortexcode_ai_models_catalog::MODELS_JSON)
            .expect("the embedded model catalog matches the Model shape");
        build_registry(models)
    })
}

fn build_registry(models: Vec<Model>) -> Registry {
    let mut providers: Vec<(String, Vec<Model>)> = Vec::new();
    let mut index = HashMap::new();
    for model in models {
        let p = match providers
            .iter()
            .position(|(name, _)| *name == model.provider)
        {
            Some(p) => p,
            None => {
                providers.push((model.provider.clone(), Vec::new()));
                providers.len() - 1
            }
        };
        let key = (model.provider.clone(), model.id.clone());
        let list = &mut providers[p].1;
        match index.get(&key) {
            // A repeated id replaces the earlier entry in place (Map.set).
            Some(&(_, m)) => list[m] = model,
            None => {
                index.insert(key, (p, list.len()));
                list.push(model);
            }
        }
    }
    Registry { providers, index }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// `getModel(provider, modelId)`.
pub fn get_model(provider: &str, model_id: &str) -> Option<&'static Model> {
    let r = registry();
    let &(p, m) = r.index.get(&(provider.to_string(), model_id.to_string()))?;
    Some(&r.providers[p].1[m])
}

/// `getProviders()`, in catalog order.
pub fn get_providers() -> Vec<&'static str> {
    registry()
        .providers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect()
}

/// `getModels(provider)`, in catalog order.
pub fn get_models(provider: &str) -> Vec<&'static Model> {
    registry()
        .providers
        .iter()
        .find(|(name, _)| name == provider)
        .map(|(_, models)| models.iter().collect())
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
