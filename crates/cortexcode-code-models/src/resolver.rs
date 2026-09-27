//! Model resolution, scoping and initial selection.
//!
//! Port of hoocode `packages/coding-agent/src/core/model-resolver.ts` (pinned v0.5.89).
//! The TS functions print warnings with `console.warn` / exit on CLI errors; here they
//! are returned for the caller to print.

use crate::{AuthLookup, ModelRegistry};
use cortexcode_ai_types::{Model, ThinkingLevel};
use std::cmp::Ordering;

/// `defaultModelPerProvider`, in the TS object's key order (findInitialModel walks it).
pub const DEFAULT_MODEL_PER_PROVIDER: &[(&str, &str)] = &[
    ("anthropic", "claude-opus-4-7"),
    ("openai", "gpt-5.4"),
    ("azure-openai-responses", "gpt-5.4"),
    ("openai-codex", "gpt-5.6-terra"),
    ("deepseek", "deepseek-v4-pro"),
    ("google", "gemini-3.1-pro-preview"),
    ("google-vertex", "gemini-3.1-pro-preview"),
    ("google-gemini-cli", "gemini-3.1-pro-preview"),
    ("google-antigravity", "gemini-3.8-flash-tiered"),
    ("github-copilot", "gpt-5.4"),
    ("openrouter", "moonshotai/kimi-k2.6"),
    ("vercel-ai-gateway", "zai/glm-5.1"),
    ("xai", "grok-4.20-0309-reasoning"),
    ("groq", "openai/gpt-oss-120b"),
    ("cerebras", "zai-glm-4.7"),
    ("zai", "glm-5.1"),
    ("minimax", "MiniMax-M2.7"),
    ("minimax-cn", "MiniMax-M2.7"),
    ("moonshotai", "kimi-k2.6"),
    ("moonshotai-cn", "kimi-k2.6"),
    ("huggingface", "moonshotai/Kimi-K2.6"),
    ("fireworks", "accounts/fireworks/models/kimi-k2p6"),
    ("together", "moonshotai/Kimi-K2.6"),
    ("opencode", "kimi-k2.6"),
    ("opencode-go", "kimi-k2.6"),
    ("kimi-coding", "kimi-for-coding"),
    ("xiaomi", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-cn", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-ams", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-sgp", "mimo-v2.5-pro"),
    ("nvidia", "meta/llama-3.3-70b-instruct"),
];

/// `DEFAULT_THINKING_LEVEL` (core/defaults.ts).
pub const DEFAULT_THINKING_LEVEL: ThinkingLevel = ThinkingLevel::Off;

/// `defaultModelPerProvider[provider]`.
pub fn default_model_for_provider(provider: &str) -> Option<&'static str> {
    DEFAULT_MODEL_PER_PROVIDER
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, id)| *id)
}

/// `isValidThinkingLevel` + parse (cli/args.ts `VALID_THINKING_LEVELS`).
pub fn parse_thinking_level(level: &str) -> Option<ThinkingLevel> {
    Some(match level {
        "off" => ThinkingLevel::Off,
        "minimal" => ThinkingLevel::Minimal,
        "low" => ThinkingLevel::Low,
        "medium" => ThinkingLevel::Medium,
        "high" => ThinkingLevel::High,
        "xhigh" => ThinkingLevel::XHigh,
        _ => return None,
    })
}

/// An approximation of `String.prototype.localeCompare` (ICU root collation):
/// punctuation before digits before letters, case-insensitive first, then
/// lowercase before uppercase.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    fn key(c: char) -> (u8, char) {
        let class = if c.is_alphabetic() {
            2
        } else if c.is_numeric() {
            1
        } else {
            0
        };
        (class, c.to_lowercase().next().unwrap_or(c))
    }
    let primary = a.chars().map(key).cmp(b.chars().map(key));
    primary.then_with(|| {
        // Tertiary: lowercase sorts before uppercase.
        let case = |s: &str| s.chars().map(|c| c.is_uppercase()).collect::<Vec<_>>();
        case(a).cmp(&case(b))
    })
}

/// `ScopedModel`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedModel {
    pub model: Model,
    /// Explicit level from the pattern (`model:high`), `None` otherwise.
    pub thinking_level: Option<ThinkingLevel>,
}

/// `isAlias`: no `-YYYYMMDD` suffix (or ends in `-latest`).
fn is_alias(id: &str) -> bool {
    if id.ends_with("-latest") {
        return true;
    }
    let bytes = id.as_bytes();
    let dated = bytes.len() >= 9
        && bytes[bytes.len() - 9] == b'-'
        && bytes[bytes.len() - 8..].iter().all(u8::is_ascii_digit);
    !dated
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// `findExactModelReferenceMatch`: a bare id or a canonical `provider/modelId`;
/// ambiguous matches are rejected.
pub fn find_exact_model_reference_match<'a>(
    model_reference: &str,
    available_models: &'a [Model],
) -> Option<&'a Model> {
    let trimmed = model_reference.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.to_lowercase();

    let canonical: Vec<&Model> = available_models
        .iter()
        .filter(|m| format!("{}/{}", m.provider, m.id).to_lowercase() == normalized)
        .collect();
    match canonical.len() {
        1 => return Some(canonical[0]),
        n if n > 1 => return None,
        _ => {}
    }

    if let Some(slash) = trimmed.find('/') {
        let provider = trimmed[..slash].trim();
        let model_id = trimmed[slash + 1..].trim();
        if !provider.is_empty() && !model_id.is_empty() {
            let matches: Vec<&Model> = available_models
                .iter()
                .filter(|m| {
                    eq_ignore_case(&m.provider, provider) && eq_ignore_case(&m.id, model_id)
                })
                .collect();
            match matches.len() {
                1 => return Some(matches[0]),
                n if n > 1 => return None,
                _ => {}
            }
        }
    }

    let id_matches: Vec<&Model> = available_models
        .iter()
        .filter(|m| m.id.to_lowercase() == normalized)
        .collect();
    (id_matches.len() == 1).then(|| id_matches[0])
}

/// `tryMatchModel`: exact reference, then substring of id/name (then with `.`/`-`
/// normalized); aliases beat dated versions, highest id wins.
fn try_match_model<'a>(pattern: &str, available_models: &'a [Model]) -> Option<&'a Model> {
    if let Some(exact) = find_exact_model_reference_match(pattern, available_models) {
        return Some(exact);
    }
    let lower = pattern.to_lowercase();
    let mut matches: Vec<&Model> = available_models
        .iter()
        .filter(|m| m.id.to_lowercase().contains(&lower) || m.name.to_lowercase().contains(&lower))
        .collect();
    if matches.is_empty() {
        let normalize = |s: &str| s.to_lowercase().replace('.', "-");
        let normalized = normalize(pattern);
        matches = available_models
            .iter()
            .filter(|m| {
                normalize(&m.id).contains(&normalized) || normalize(&m.name).contains(&normalized)
            })
            .collect();
    }
    if matches.is_empty() {
        return None;
    }
    let (mut aliases, mut dated): (Vec<&Model>, Vec<&Model>) =
        matches.into_iter().partition(|m| is_alias(&m.id));
    let pick = if aliases.is_empty() {
        &mut dated
    } else {
        &mut aliases
    };
    // `sort((a, b) => b.id.localeCompare(a.id))` is stable: first of the highest.
    pick.sort_by(|a, b| locale_compare(&b.id, &a.id));
    pick.first().copied()
}

/// `ParsedModelResult`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub warning: Option<String>,
}

/// `parseModelPattern`: the full pattern first, then peel `:suffix` thinking levels
/// off the end. With `allow_invalid_thinking_level_fallback` false (CLI `--model`),
/// an invalid suffix is part of the id and fails the match.
pub fn parse_model_pattern(
    pattern: &str,
    available_models: &[Model],
    allow_invalid_thinking_level_fallback: bool,
) -> ParsedModelResult {
    if let Some(model) = try_match_model(pattern, available_models) {
        return ParsedModelResult {
            model: Some(model.clone()),
            ..Default::default()
        };
    }
    let Some(colon) = pattern.rfind(':') else {
        return ParsedModelResult::default();
    };
    let prefix = &pattern[..colon];
    let suffix = &pattern[colon + 1..];

    if let Some(level) = parse_thinking_level(suffix) {
        let result = parse_model_pattern(
            prefix,
            available_models,
            allow_invalid_thinking_level_fallback,
        );
        if result.model.is_some() {
            return ParsedModelResult {
                thinking_level: if result.warning.is_some() {
                    None
                } else {
                    Some(level)
                },
                ..result
            };
        }
        return result;
    }

    if !allow_invalid_thinking_level_fallback {
        return ParsedModelResult::default();
    }
    let result = parse_model_pattern(
        prefix,
        available_models,
        allow_invalid_thinking_level_fallback,
    );
    if result.model.is_some() {
        return ParsedModelResult {
            model: result.model,
            thinking_level: None,
            warning: Some(format!(
                "Invalid thinking level \"{suffix}\" in pattern \"{pattern}\". Using default instead."
            )),
        };
    }
    result
}

/// `minimatch(text, glob, { nocase: true })` for model references: `*` and `?`
/// stay within a `/` segment, `[...]` classes, `**` crosses segments.
fn glob_matches(glob: &str, text: &str) -> bool {
    let Ok(pattern) = glob::Pattern::new(glob) else {
        return false;
    };
    pattern.matches_with(
        text,
        glob::MatchOptions {
            case_sensitive: false,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        },
    )
}

/// Result of [`resolve_model_scope`]: the models plus the `Warning: ...` lines
/// hoocode prints (in yellow) while resolving.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelScope {
    pub models: Vec<ScopedModel>,
    pub warnings: Vec<String>,
}

/// `resolveModelScope`: `--models` / `enabledModels` patterns against the models
/// with configured auth. Globs match `provider/id` or the bare id.
pub fn resolve_model_scope(patterns: &[String], available_models: &[Model]) -> ModelScope {
    let mut scope = ModelScope::default();
    let push = |scope: &mut ModelScope, model: &Model, level: Option<ThinkingLevel>| {
        if !scope
            .models
            .iter()
            .any(|sm| sm.model.provider == model.provider && sm.model.id == model.id)
        {
            scope.models.push(ScopedModel {
                model: model.clone(),
                thinking_level: level,
            });
        }
    };

    for pattern in patterns {
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            let mut glob = pattern.as_str();
            let mut level = None;
            if let Some(colon) = pattern.rfind(':') {
                if let Some(l) = parse_thinking_level(&pattern[colon + 1..]) {
                    level = Some(l);
                    glob = &pattern[..colon];
                }
            }
            let matching: Vec<&Model> = available_models
                .iter()
                .filter(|m| {
                    glob_matches(glob, &format!("{}/{}", m.provider, m.id))
                        || glob_matches(glob, &m.id)
                })
                .collect();
            if matching.is_empty() {
                scope
                    .warnings
                    .push(format!("Warning: No models match pattern \"{pattern}\""));
                continue;
            }
            for model in matching {
                push(&mut scope, model, level.clone());
            }
            continue;
        }

        let parsed = parse_model_pattern(pattern, available_models, true);
        if let Some(warning) = &parsed.warning {
            scope.warnings.push(format!("Warning: {warning}"));
        }
        let Some(model) = parsed.model else {
            scope
                .warnings
                .push(format!("Warning: No models match pattern \"{pattern}\""));
            continue;
        };
        push(&mut scope, &model, parsed.thinking_level);
    }
    scope
}

/// `ResolveCliModelResult`: `error` is set (and `model` unset) on failure.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolveCliModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub warning: Option<String>,
    pub error: Option<String>,
}

/// `buildFallbackModel`: a custom id on a known provider, shaped like the
/// provider's default (or first) model.
fn build_fallback_model(
    provider: &str,
    model_id: &str,
    available_models: &[Model],
) -> Option<Model> {
    let provider_models: Vec<&Model> = available_models
        .iter()
        .filter(|m| m.provider == provider)
        .collect();
    let first = *provider_models.first()?;
    let base = default_model_for_provider(provider)
        .and_then(|id| provider_models.iter().find(|m| m.id == id).copied())
        .unwrap_or(first);
    Some(Model {
        id: model_id.to_string(),
        name: model_id.to_string(),
        ..base.clone()
    })
}

fn exact_id_or_reference<'a>(input: &str, models: &'a [Model]) -> Option<&'a Model> {
    let lower = input.to_lowercase();
    models.iter().find(|m| {
        m.id.to_lowercase() == lower || format!("{}/{}", m.provider, m.id).to_lowercase() == lower
    })
}

/// `resolveCliModel`: `--provider` / `--model` (with `provider/pattern` and
/// `pattern:thinking` shorthands) against *all* models, so `--api-key` works for
/// first-time setup.
pub fn resolve_cli_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    all_models: &[Model],
) -> ResolveCliModelResult {
    let Some(cli_model) = cli_model else {
        return ResolveCliModelResult::default();
    };
    if all_models.is_empty() {
        return ResolveCliModelResult {
            error: Some(
                "No models available. Check your installation or add models to models.json.".into(),
            ),
            ..Default::default()
        };
    }

    // Case-insensitive canonical provider lookup (last model wins, like the TS Map).
    let canonical_provider = |name: &str| -> Option<String> {
        let lower = name.to_lowercase();
        all_models
            .iter()
            .rev()
            .find(|m| m.provider.to_lowercase() == lower)
            .map(|m| m.provider.clone())
    };

    let mut provider = cli_provider.and_then(canonical_provider);
    if let (Some(cli_provider), None) = (cli_provider, &provider) {
        return ResolveCliModelResult {
            error: Some(format!(
                "Unknown provider \"{cli_provider}\". Use --list-models to see available providers/models."
            )),
            ..Default::default()
        };
    }

    let mut pattern = cli_model.to_string();
    let mut inferred_provider = false;
    if provider.is_none() {
        if let Some(slash) = cli_model.find('/') {
            if let Some(canonical) = canonical_provider(&cli_model[..slash]) {
                provider = Some(canonical);
                pattern = cli_model[slash + 1..].to_string();
                inferred_provider = true;
            }
        }
    }

    if provider.is_none() {
        if let Some(exact) = exact_id_or_reference(cli_model, all_models) {
            return ResolveCliModelResult {
                model: Some(exact.clone()),
                ..Default::default()
            };
        }
    }

    if let (Some(_), Some(provider)) = (cli_provider, &provider) {
        let prefix = format!("{provider}/");
        if cli_model.to_lowercase().starts_with(&prefix.to_lowercase()) {
            pattern = cli_model[prefix.len()..].to_string();
        }
    }

    let candidates: Vec<Model> = match &provider {
        Some(p) => all_models
            .iter()
            .filter(|m| &m.provider == p)
            .cloned()
            .collect(),
        None => all_models.to_vec(),
    };
    let parsed = parse_model_pattern(&pattern, &candidates, false);
    if parsed.model.is_some() {
        return ResolveCliModelResult {
            model: parsed.model,
            thinking_level: parsed.thinking_level,
            warning: parsed.warning,
            error: None,
        };
    }

    if inferred_provider {
        if let Some(exact) = exact_id_or_reference(cli_model, all_models) {
            return ResolveCliModelResult {
                model: Some(exact.clone()),
                ..Default::default()
            };
        }
        let fallback = parse_model_pattern(cli_model, all_models, false);
        if fallback.model.is_some() {
            return ResolveCliModelResult {
                model: fallback.model,
                thinking_level: fallback.thinking_level,
                warning: fallback.warning,
                error: None,
            };
        }
    }

    if let Some(provider) = &provider {
        if let Some(model) = build_fallback_model(provider, &pattern, all_models) {
            let note = format!(
                "Model \"{pattern}\" not found for provider \"{provider}\". Using custom model id."
            );
            return ResolveCliModelResult {
                model: Some(model),
                thinking_level: None,
                warning: Some(match &parsed.warning {
                    Some(w) => format!("{w} {note}"),
                    None => note,
                }),
                error: None,
            };
        }
    }

    let display = match &provider {
        Some(p) => format!("{p}/{pattern}"),
        None => cli_model.to_string(),
    };
    ResolveCliModelResult {
        model: None,
        thinking_level: None,
        warning: parsed.warning,
        error: Some(format!(
            "Model \"{display}\" not found. Use --list-models to see available models."
        )),
    }
}

/// What [`find_initial_model`] needs from the registry (the TS tests stub these).
pub trait ModelSource {
    fn all_models(&self) -> Vec<Model>;
    fn available_models(&self) -> Vec<Model>;
    fn find_model(&self, provider: &str, model_id: &str) -> Option<Model>;
    fn model_has_configured_auth(&self, model: &Model) -> bool;
}

/// A registry together with the credentials that decide availability.
pub struct RegistryWithAuth<'a> {
    pub registry: &'a ModelRegistry,
    pub auth: &'a dyn AuthLookup,
}

impl ModelSource for RegistryWithAuth<'_> {
    fn all_models(&self) -> Vec<Model> {
        self.registry.get_all().to_vec()
    }
    fn available_models(&self) -> Vec<Model> {
        self.registry
            .get_available(self.auth)
            .into_iter()
            .cloned()
            .collect()
    }
    fn find_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.registry.find(provider, model_id).cloned()
    }
    fn model_has_configured_auth(&self, model: &Model) -> bool {
        self.registry.has_configured_auth(model, self.auth)
    }
}

/// Inputs of `findInitialModel`.
#[derive(Debug, Clone, Default)]
pub struct InitialModelOptions<'a> {
    pub cli_provider: Option<&'a str>,
    pub cli_model: Option<&'a str>,
    pub scoped_models: &'a [ScopedModel],
    pub is_continuing: bool,
    pub default_provider: Option<&'a str>,
    pub default_model_id: Option<&'a str>,
    pub default_thinking_level: Option<ThinkingLevel>,
}

/// `InitialModelResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct InitialModelResult {
    pub model: Option<Model>,
    pub thinking_level: ThinkingLevel,
    pub fallback_message: Option<String>,
}

/// `findInitialModel`: CLI flags, then the first scoped model (unless continuing),
/// then the saved default when its provider has auth, then a known provider's
/// default among the available models, then the first available model.
/// `Err` carries the CLI error hoocode prints before exiting.
pub fn find_initial_model(
    options: InitialModelOptions<'_>,
    source: &dyn ModelSource,
) -> Result<InitialModelResult, String> {
    let found = |model: Model, thinking_level: ThinkingLevel| InitialModelResult {
        model: Some(model),
        thinking_level,
        fallback_message: None,
    };

    if let (Some(provider), Some(model)) = (options.cli_provider, options.cli_model) {
        let resolved = resolve_cli_model(Some(provider), Some(model), &source.all_models());
        if let Some(error) = resolved.error {
            return Err(error);
        }
        if let Some(model) = resolved.model {
            return Ok(found(model, DEFAULT_THINKING_LEVEL));
        }
    }

    if let Some(first) = options.scoped_models.first() {
        if !options.is_continuing {
            let level = first
                .thinking_level
                .clone()
                .or(options.default_thinking_level.clone())
                .unwrap_or(DEFAULT_THINKING_LEVEL);
            return Ok(found(first.model.clone(), level));
        }
    }

    if let (Some(provider), Some(model_id)) = (options.default_provider, options.default_model_id) {
        if let Some(model) = source.find_model(provider, model_id) {
            if source.model_has_configured_auth(&model) {
                let level = options
                    .default_thinking_level
                    .clone()
                    .unwrap_or(DEFAULT_THINKING_LEVEL);
                return Ok(found(model, level));
            }
        }
    }

    let available = source.available_models();
    if let Some(first) = available.first() {
        for (provider, default_id) in DEFAULT_MODEL_PER_PROVIDER {
            if let Some(m) = available
                .iter()
                .find(|m| m.provider == *provider && m.id == *default_id)
            {
                return Ok(found(m.clone(), DEFAULT_THINKING_LEVEL));
            }
        }
        return Ok(found(first.clone(), DEFAULT_THINKING_LEVEL));
    }

    Ok(InitialModelResult {
        model: None,
        thinking_level: DEFAULT_THINKING_LEVEL,
        fallback_message: None,
    })
}
