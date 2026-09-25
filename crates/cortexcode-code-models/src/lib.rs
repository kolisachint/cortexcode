//! Model registry: built-in models plus `models.json` custom providers and overrides.
//!
//! Port of hoocode `packages/coding-agent/src/core/model-registry.ts` (pinned v0.5.89).
//! Not yet ported: dynamic provider registration by extensions (`registerProvider`),
//! OAuth `modifyModels`, and auth-storage lookups (ledger 10.4b); callers pass an
//! [`AuthLookup`] instead.

pub mod config_value;

use cortexcode_ai_types::{Model, ModelCost};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use config_value::{resolve_config_value, resolve_config_value_or_err, resolve_headers_or_err};

// ---------------------------------------------------------------------------
// models.json schema (ModelsConfigSchema)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelsConfig {
    providers: HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderConfig {
    #[allow(dead_code)]
    name: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    api: Option<String>,
    headers: Option<HashMap<String, String>>,
    compat: Option<Map<String, Value>>,
    auth_header: Option<bool>,
    models: Option<Vec<ModelDefinition>>,
    model_overrides: Option<HashMap<String, ModelOverride>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelDefinition {
    id: String,
    name: Option<String>,
    api: Option<String>,
    base_url: Option<String>,
    reasoning: Option<bool>,
    thinking_level_map: Option<HashMap<String, Value>>,
    input: Option<Vec<String>>,
    cost: Option<FullCost>,
    context_window: Option<f64>,
    max_tokens: Option<f64>,
    headers: Option<HashMap<String, String>>,
    compat: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FullCost {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelOverride {
    name: Option<String>,
    reasoning: Option<bool>,
    thinking_level_map: Option<HashMap<String, Value>>,
    input: Option<Vec<String>>,
    cost: Option<PartialCost>,
    context_window: Option<f64>,
    max_tokens: Option<f64>,
    headers: Option<HashMap<String, String>>,
    compat: Option<Map<String, Value>>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `stripJsonComments()`: remove `//` line comments and trailing commas outside strings.
pub fn strip_json_comments(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    // Pass 1: line comments.
    let mut pass1 = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            let end = string_end(&chars, i);
            pass1.extend(&chars[i..end]);
            i = end;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else {
            pass1.push(c);
            i += 1;
        }
    }
    // Pass 2: `,` followed only by whitespace and `}`/`]`.
    let chars: Vec<char> = pass1.chars().collect();
    let mut out = String::with_capacity(pass1.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            let end = string_end(&chars, i);
            out.extend(&chars[i..end]);
            i = end;
        } else if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if !matches!(chars.get(j), Some('}') | Some(']')) {
                out.push(c);
            }
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Index just past the JSON string literal starting at `start` (a `"`).
fn string_end(chars: &[char], start: usize) -> usize {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

/// `mergeCompat()`: shallow merge, with `openRouterRouting` / `vercelGatewayRouting`
/// merged one level deeper.
fn merge_compat(base: Option<&Value>, over: Option<&Map<String, Value>>) -> Option<Value> {
    let Some(over) = over else {
        return base.cloned();
    };
    let base_obj = base.and_then(Value::as_object).cloned().unwrap_or_default();
    let mut merged = base_obj.clone();
    for (k, v) in over {
        merged.insert(k.clone(), v.clone());
    }
    for key in ["openRouterRouting", "vercelGatewayRouting"] {
        let b = base_obj.get(key).and_then(Value::as_object);
        let o = over.get(key).and_then(Value::as_object);
        if b.is_some() || o.is_some() {
            let mut m = b.cloned().unwrap_or_default();
            for (k, v) in o.cloned().unwrap_or_default() {
                m.insert(k, v);
            }
            merged.insert(key.to_string(), Value::Object(m));
        }
    }
    Some(Value::Object(merged))
}

fn to_thinking_map(map: &HashMap<String, Value>) -> HashMap<String, Value> {
    map.clone()
}

/// `applyModelOverride()`.
fn apply_model_override(model: &Model, over: &ModelOverride) -> Model {
    let mut result = model.clone();
    if let Some(name) = &over.name {
        result.name = name.clone();
    }
    if let Some(r) = over.reasoning {
        result.reasoning = r;
    }
    if let Some(map) = &over.thinking_level_map {
        let mut merged = model.thinking_level_map.clone().unwrap_or_default();
        merged.extend(to_thinking_map(map));
        result.thinking_level_map = Some(merged);
    }
    if let Some(input) = &over.input {
        result.input = input.clone();
    }
    if let Some(cw) = over.context_window {
        result.context_window = cw as u64;
    }
    if let Some(mt) = over.max_tokens {
        result.max_tokens = mt as u64;
    }
    if let Some(cost) = &over.cost {
        result.cost = ModelCost {
            input: cost.input.unwrap_or(model.cost.input),
            output: cost.output.unwrap_or(model.cost.output),
            cache_read: cost.cache_read.unwrap_or(model.cost.cache_read),
            cache_write: cost.cache_write.unwrap_or(model.cost.cache_write),
        };
    }
    result.compat = merge_compat(model.compat.as_ref(), over.compat.as_ref());
    result
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Credentials stored outside models.json (auth.json / OAuth), by provider.
/// Real implementation: ledger 10.4b (`code-auth`).
pub trait AuthLookup {
    /// API key from auth storage, without env-var fallback (`includeFallback: false`).
    fn api_key(&self, provider: &str) -> Option<String>;
    /// Whether auth storage has any credential for the provider.
    fn has_auth(&self, provider: &str) -> bool {
        self.api_key(provider).is_some()
    }
}

/// No stored credentials.
pub struct NoAuth;

impl AuthLookup for NoAuth {
    fn api_key(&self, _provider: &str) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Default)]
struct ProviderRequestConfig {
    api_key: Option<String>,
    headers: Option<HashMap<String, String>>,
    auth_header: bool,
}

/// Resolved request auth for a model (TS `ResolvedRequestAuth` with `ok: true`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RequestAuth {
    pub api_key: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

/// TS `ProviderOverride`: baseUrl/compat applied to a provider's built-in models.
#[derive(Debug, Clone, Default)]
struct ProviderOverride {
    base_url: Option<String>,
    compat: Option<Map<String, Value>>,
}

#[derive(Default)]
struct CustomModels {
    models: Vec<Model>,
    overrides: HashMap<String, ProviderOverride>,
    model_overrides: HashMap<String, HashMap<String, ModelOverride>>,
}

/// Built-in + custom models and per-provider request configuration.
pub struct ModelRegistry {
    models: Vec<Model>,
    provider_request_configs: HashMap<String, ProviderRequestConfig>,
    model_request_headers: HashMap<String, HashMap<String, String>>,
    load_error: Option<String>,
    models_json_path: Option<PathBuf>,
}

impl ModelRegistry {
    /// Load built-ins plus the given `models.json` (TS `ModelRegistry.create`).
    pub fn create(models_json_path: impl Into<PathBuf>) -> Self {
        let mut r = Self::empty(Some(models_json_path.into()));
        r.load_models();
        r
    }

    /// Built-in models only (TS `ModelRegistry.inMemory`).
    pub fn in_memory() -> Self {
        let mut r = Self::empty(None);
        r.load_models();
        r
    }

    fn empty(path: Option<PathBuf>) -> Self {
        Self {
            models: Vec::new(),
            provider_request_configs: HashMap::new(),
            model_request_headers: HashMap::new(),
            load_error: None,
            models_json_path: path,
        }
    }

    /// Reload from disk.
    pub fn refresh(&mut self) {
        self.provider_request_configs.clear();
        self.model_request_headers.clear();
        self.load_error = None;
        self.load_models();
    }

    /// Error from loading models.json, if any (built-ins are still available).
    pub fn error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// All models (built-in + custom).
    pub fn get_all(&self) -> &[Model] {
        &self.models
    }

    /// Models whose provider has auth configured.
    pub fn get_available(&self, auth: &dyn AuthLookup) -> Vec<&Model> {
        self.models
            .iter()
            .filter(|m| self.has_configured_auth(m, auth))
            .collect()
    }

    /// Find a model by provider and id.
    pub fn find(&self, provider: &str, model_id: &str) -> Option<&Model> {
        self.models
            .iter()
            .find(|m| m.provider == provider && m.id == model_id)
    }

    pub fn has_configured_auth(&self, model: &Model, auth: &dyn AuthLookup) -> bool {
        auth.has_auth(&model.provider)
            || self
                .provider_request_configs
                .get(&model.provider)
                .is_some_and(|c| c.api_key.is_some())
    }

    fn load_models(&mut self) {
        let custom = match self.models_json_path.clone() {
            Some(path) => match self.load_custom_models(&path) {
                Ok(c) => c,
                Err(e) => {
                    self.load_error = Some(e);
                    CustomModels::default()
                }
            },
            None => CustomModels::default(),
        };
        let built_in = load_built_in_models(&custom.overrides, &custom.model_overrides);
        self.models = merge_custom_models(built_in, custom.models);
    }

    fn load_custom_models(&mut self, path: &Path) -> Result<CustomModels, String> {
        if !path.exists() {
            return Ok(CustomModels::default());
        }
        let file = path.display();
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to load models.json: {e}\n\nFile: {file}"))?;
        let parsed: Value = serde_json::from_str(&strip_json_comments(&content))
            .map_err(|e| format!("Failed to parse models.json: {e}\n\nFile: {file}"))?;
        let config: ModelsConfig = serde_json::from_value(parsed)
            .map_err(|e| format!("Invalid models.json schema:\n  - {e}\n\nFile: {file}"))?;
        validate_config(&config)
            .map_err(|e| format!("Failed to load models.json: {e}\n\nFile: {file}"))?;

        let mut custom = CustomModels::default();
        for (name, pc) in &config.providers {
            if pc.base_url.is_some() || pc.compat.is_some() {
                custom.overrides.insert(
                    name.clone(),
                    ProviderOverride {
                        base_url: pc.base_url.clone(),
                        compat: pc.compat.clone(),
                    },
                );
            }
            if pc.api_key.is_some() || pc.headers.is_some() || pc.auth_header == Some(true) {
                self.provider_request_configs.insert(
                    name.clone(),
                    ProviderRequestConfig {
                        api_key: pc.api_key.clone(),
                        headers: pc.headers.clone(),
                        auth_header: pc.auth_header == Some(true),
                    },
                );
            }
            if let Some(mo) = &pc.model_overrides {
                custom.model_overrides.insert(name.clone(), mo.clone());
                for (model_id, o) in mo {
                    self.store_model_headers(name, model_id, o.headers.as_ref());
                }
            }
        }
        custom.models = self.parse_models(&config);
        Ok(custom)
    }

    fn store_model_headers(
        &mut self,
        provider: &str,
        model_id: &str,
        headers: Option<&HashMap<String, String>>,
    ) {
        let key = format!("{provider}:{model_id}");
        match headers {
            Some(h) if !h.is_empty() => {
                self.model_request_headers.insert(key, h.clone());
            }
            _ => {
                self.model_request_headers.remove(&key);
            }
        }
    }

    /// `parseModels()`: custom model definitions with their defaults.
    fn parse_models(&mut self, config: &ModelsConfig) -> Vec<Model> {
        let mut models = Vec::new();
        let mut providers: Vec<_> = config.providers.iter().collect();
        providers.sort_by(|a, b| a.0.cmp(b.0));
        for (name, pc) in providers {
            let defs = pc.models.clone().unwrap_or_default();
            if defs.is_empty() {
                continue;
            }
            let built_in = built_in_defaults(name);
            for def in defs {
                let Some(api) = def
                    .api
                    .clone()
                    .or_else(|| pc.api.clone())
                    .or_else(|| built_in.as_ref().map(|d| d.0.clone()))
                else {
                    continue;
                };
                let Some(base_url) = def
                    .base_url
                    .clone()
                    .or_else(|| pc.base_url.clone())
                    .or_else(|| built_in.as_ref().map(|d| d.1.clone()))
                else {
                    continue;
                };
                let provider_compat = pc.compat.clone().map(Value::Object);
                let compat = merge_compat(provider_compat.as_ref(), def.compat.as_ref());
                self.store_model_headers(name, &def.id, def.headers.as_ref());
                let cost = def
                    .cost
                    .clone()
                    .map_or(ModelCost::default(), |c| ModelCost {
                        input: c.input,
                        output: c.output,
                        cache_read: c.cache_read,
                        cache_write: c.cache_write,
                    });
                models.push(Model {
                    name: def.name.clone().unwrap_or_else(|| def.id.clone()),
                    id: def.id,
                    api,
                    provider: name.clone(),
                    base_url,
                    reasoning: def.reasoning.unwrap_or(false),
                    thinking_level_map: def.thinking_level_map.as_ref().map(to_thinking_map),
                    input: def.input.unwrap_or_else(|| vec!["text".to_string()]),
                    cost,
                    context_window: def.context_window.map_or(128_000, |v| v as u64),
                    max_tokens: def.max_tokens.map_or(16_384, |v| v as u64),
                    headers: None,
                    compat,
                });
            }
        }
        models
    }

    /// `getApiKeyAndHeaders()`.
    pub fn get_api_key_and_headers(
        &self,
        model: &Model,
        auth: &dyn AuthLookup,
    ) -> Result<RequestAuth, String> {
        let pc = self.provider_request_configs.get(&model.provider);
        let api_key = match auth.api_key(&model.provider) {
            Some(k) => Some(k),
            None => match pc.and_then(|c| c.api_key.as_deref()) {
                Some(cfg) => Some(resolve_config_value_or_err(
                    cfg,
                    &format!("API key for provider \"{}\"", model.provider),
                )?),
                None => None,
            },
        };
        let provider_headers = resolve_headers_or_err(
            pc.and_then(|c| c.headers.as_ref()),
            &format!("provider \"{}\"", model.provider),
        )?;
        let model_headers = resolve_headers_or_err(
            self.model_request_headers
                .get(&format!("{}:{}", model.provider, model.id)),
            &format!("model \"{}/{}\"", model.provider, model.id),
        )?;
        let mut headers: Option<HashMap<String, String>> =
            if model.headers.is_some() || provider_headers.is_some() || model_headers.is_some() {
                let mut h = model.headers.clone().unwrap_or_default();
                h.extend(provider_headers.unwrap_or_default());
                h.extend(model_headers.unwrap_or_default());
                Some(h)
            } else {
                None
            };
        if pc.is_some_and(|c| c.auth_header) {
            let Some(key) = &api_key else {
                return Err(format!("No API key found for \"{}\"", model.provider));
            };
            headers
                .get_or_insert_with(HashMap::new)
                .insert("Authorization".into(), format!("Bearer {key}"));
        }
        Ok(RequestAuth {
            api_key,
            headers: headers.filter(|h| !h.is_empty()),
        })
    }
}

/// `validateConfig()`.
fn validate_config(config: &ModelsConfig) -> Result<(), String> {
    let built_in: std::collections::HashSet<&str> =
        cortexcode_ai_models::get_providers().into_iter().collect();
    let mut names: Vec<_> = config.providers.keys().collect();
    names.sort();
    for name in names {
        let pc = &config.providers[name];
        let is_built_in = built_in.contains(name.as_str());
        let models = pc.models.clone().unwrap_or_default();
        let has_overrides = pc.model_overrides.as_ref().is_some_and(|m| !m.is_empty());
        if models.is_empty() {
            if pc.base_url.is_none()
                && pc.headers.is_none()
                && pc.compat.is_none()
                && !has_overrides
            {
                return Err(format!(
                    "Provider {name}: must specify \"baseUrl\", \"headers\", \"compat\", \"modelOverrides\", or \"models\"."
                ));
            }
        } else if !is_built_in {
            if pc.base_url.is_none() {
                return Err(format!(
                    "Provider {name}: \"baseUrl\" is required when defining custom models."
                ));
            }
            if pc.api_key.is_none() {
                return Err(format!(
                    "Provider {name}: \"apiKey\" is required when defining custom models."
                ));
            }
        }
        for def in &models {
            if pc.api.is_none() && def.api.is_none() && !is_built_in {
                return Err(format!(
                    "Provider {name}, model {}: no \"api\" specified. Set at provider or model level.",
                    def.id
                ));
            }
            if def.id.is_empty() {
                return Err(format!("Provider {name}: model missing \"id\""));
            }
            if def.context_window.is_some_and(|v| v <= 0.0) {
                return Err(format!(
                    "Provider {name}, model {}: invalid contextWindow",
                    def.id
                ));
            }
            if def.max_tokens.is_some_and(|v| v <= 0.0) {
                return Err(format!(
                    "Provider {name}, model {}: invalid maxTokens",
                    def.id
                ));
            }
        }
    }
    Ok(())
}

/// Built-in (api, baseUrl) defaults for a provider, from its first model.
fn built_in_defaults(provider: &str) -> Option<(String, String)> {
    cortexcode_ai_models::get_models(provider)
        .first()
        .map(|m| (m.api.clone(), m.base_url.clone()))
}

/// `loadBuiltInModels()`: built-ins with provider and per-model overrides applied.
fn load_built_in_models(
    overrides: &HashMap<String, ProviderOverride>,
    model_overrides: &HashMap<String, HashMap<String, ModelOverride>>,
) -> Vec<Model> {
    // Catalog order, as `getProviders()` / `getModels()` iterate it.
    let mut out = Vec::new();
    for provider in cortexcode_ai_models::get_providers() {
        for m in cortexcode_ai_models::get_models(provider) {
            let mut model = m.clone();
            if let Some(o) = overrides.get(provider) {
                if let Some(url) = &o.base_url {
                    model.base_url = url.clone();
                }
                model.compat = merge_compat(model.compat.as_ref(), o.compat.as_ref());
            }
            if let Some(o) = model_overrides.get(provider).and_then(|mo| mo.get(&m.id)) {
                model = apply_model_override(&model, o);
            }
            out.push(model);
        }
    }
    out
}

/// `mergeCustomModels()`: custom wins on provider+id conflicts.
fn merge_custom_models(mut built_in: Vec<Model>, custom: Vec<Model>) -> Vec<Model> {
    for c in custom {
        match built_in
            .iter()
            .position(|m| m.provider == c.provider && m.id == c.id)
        {
            Some(i) => built_in[i] = c,
            None => built_in.push(c),
        }
    }
    built_in
}

/// Default `models.json` location: `$CORTEXCODE_CODING_AGENT_DIR` / `$HOOCODE_CODING_AGENT_DIR`,
/// else `~/.cortexcode/models.json`, falling back to `~/.hoocode/models.json` when only
/// that exists (plan §10.6). Moves to `code-paths` in ledger 10.1.
pub fn default_models_json_path() -> Option<PathBuf> {
    for var in ["CORTEXCODE_CODING_AGENT_DIR", "HOOCODE_CODING_AGENT_DIR"] {
        if let Ok(dir) = std::env::var(var) {
            if !dir.is_empty() {
                return Some(PathBuf::from(dir).join("models.json"));
            }
        }
    }
    let home = dirs::home_dir()?;
    let primary = home.join(".cortexcode").join("models.json");
    let legacy = home.join(".hoocode").join("models.json");
    Some(if !primary.exists() && legacy.exists() {
        legacy
    } else {
        primary
    })
}

#[cfg(test)]
mod tests;
