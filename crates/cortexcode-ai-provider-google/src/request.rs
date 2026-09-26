//! Requests for Google Generative AI (`google.ts`) and Vertex AI
//! (`google-vertex.ts`), hoocode v0.5.89: option mapping of the
//! `streamSimple*` functions, `buildParams`, the `@google/genai` 1.52 client
//! configuration (base URL, API version, auth) and its REST body mapping.

use std::collections::HashMap;

use cortexcode_ai_types::{
    AbortSignal, Context, Model, SimpleStreamOptions, ThinkingBudgets, ThinkingLevel,
};
use serde_json::{json, Map, Value};

use crate::shared::{convert_messages, convert_tools, map_tool_choice};

/// `@google/genai` version whose request shape and headers are mirrored.
const SDK_VERSION: &str = "1.52.0";
const VERTEX_API_VERSION: &str = "v1";
const GCP_VERTEX_CREDENTIALS_MARKER: &str = "gcp-vertex-credentials";

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// `thinking` of `GoogleOptions` / `GoogleVertexOptions`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoogleThinking {
    pub enabled: bool,
    /// -1 for dynamic, 0 to disable.
    pub budget_tokens: Option<i64>,
    /// `GoogleThinkingLevel`: `MINIMAL` | `LOW` | `MEDIUM` | `HIGH`.
    pub level: Option<String>,
}

/// `GoogleOptions` / `GoogleVertexOptions` (`project`/`location` are Vertex's).
#[derive(Debug, Clone, Default)]
pub struct GoogleOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub signal: Option<AbortSignal>,
    pub api_key: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    /// `auto` | `none` | `any`.
    pub tool_choice: Option<String>,
    pub thinking: Option<GoogleThinking>,
    pub project: Option<String>,
    pub location: Option<String>,
}

fn base_options(
    model: &Model,
    options: &SimpleStreamOptions,
    api_key: Option<String>,
) -> GoogleOptions {
    GoogleOptions {
        temperature: options.temperature,
        max_tokens: options
            .max_tokens
            .or((model.max_tokens > 0).then(|| model.max_tokens.min(32_000))),
        signal: options.signal.clone(),
        api_key: api_key.or_else(|| options.api_key.clone().filter(|k| !k.is_empty())),
        headers: options.headers.clone(),
        ..Default::default()
    }
}

/// The clamped effort; `off` after clamping means `high`.
fn effort(model: &Model, level: &ThinkingLevel) -> ThinkingLevel {
    match cortexcode_ai_models::clamp_thinking_level(model, level) {
        ThinkingLevel::Off | ThinkingLevel::XHigh => ThinkingLevel::High,
        other => other,
    }
}

fn reasoning(options: &SimpleStreamOptions) -> Option<&ThinkingLevel> {
    options
        .reasoning
        .as_ref()
        .filter(|l| **l != ThinkingLevel::Off)
}

/// `streamSimpleGoogle`'s option mapping.
pub fn simple_google_options(
    model: &Model,
    options: &SimpleStreamOptions,
    api_key: String,
) -> GoogleOptions {
    let base = base_options(model, options, Some(api_key));
    let Some(level) = reasoning(options) else {
        return GoogleOptions {
            thinking: Some(GoogleThinking::default()),
            ..base
        };
    };
    let effort = effort(model, level);
    let thinking =
        if is_gemini3_pro(&model.id) || is_gemini3_flash(&model.id) || is_gemma4(&model.id) {
            GoogleThinking {
                enabled: true,
                level: Some(thinking_level(&effort, &model.id, true).to_string()),
                budget_tokens: None,
            }
        } else {
            GoogleThinking {
                enabled: true,
                budget_tokens: Some(google_budget(
                    &model.id,
                    &effort,
                    options.thinking_budgets.as_ref(),
                    true,
                )),
                level: None,
            }
        };
    GoogleOptions {
        thinking: Some(thinking),
        ..base
    }
}

/// `streamSimpleGoogleVertex`'s option mapping (no Gemma 4 or 2.5-flash-lite
/// special cases there).
pub fn simple_vertex_options(model: &Model, options: &SimpleStreamOptions) -> GoogleOptions {
    let base = base_options(model, options, None);
    let Some(level) = reasoning(options) else {
        return GoogleOptions {
            thinking: Some(GoogleThinking::default()),
            ..base
        };
    };
    let effort = effort(model, level);
    let thinking = if is_gemini3_pro(&model.id) || is_gemini3_flash(&model.id) {
        GoogleThinking {
            enabled: true,
            level: Some(thinking_level(&effort, &model.id, false).to_string()),
            budget_tokens: None,
        }
    } else {
        GoogleThinking {
            enabled: true,
            budget_tokens: Some(google_budget(
                &model.id,
                &effort,
                options.thinking_budgets.as_ref(),
                false,
            )),
            level: None,
        }
    };
    GoogleOptions {
        thinking: Some(thinking),
        ..base
    }
}

/// `/gemini-3(?:\.\d+)?-<family>/` on the lowercased id.
fn is_gemini3(model_id: &str, family: &str) -> bool {
    let lower = model_id.to_lowercase();
    lower.match_indices("gemini-3").any(|(i, m)| {
        let mut rest = &lower[i + m.len()..];
        if let Some(after_dot) = rest.strip_prefix('.') {
            let digits = after_dot.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 {
                rest = &after_dot[digits..];
            }
        }
        rest.strip_prefix('-')
            .is_some_and(|r| r.starts_with(family))
    })
}

fn is_gemini3_pro(model_id: &str) -> bool {
    is_gemini3(model_id, "pro")
}

fn is_gemini3_flash(model_id: &str) -> bool {
    is_gemini3(model_id, "flash")
}

/// `/gemma-?4/`.
fn is_gemma4(model_id: &str) -> bool {
    let lower = model_id.to_lowercase();
    lower.contains("gemma4") || lower.contains("gemma-4")
}

/// `getThinkingLevel` / `getGemini3ThinkingLevel`.
fn thinking_level(effort: &ThinkingLevel, model_id: &str, gemma: bool) -> &'static str {
    if is_gemini3_pro(model_id) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "LOW",
            _ => "HIGH",
        };
    }
    if gemma && is_gemma4(model_id) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "MINIMAL",
            _ => "HIGH",
        };
    }
    match effort {
        ThinkingLevel::Minimal => "MINIMAL",
        ThinkingLevel::Low => "LOW",
        ThinkingLevel::Medium => "MEDIUM",
        _ => "HIGH",
    }
}

/// `getGoogleBudget`: custom budgets win, then per-family tables, else -1
/// (dynamic). `flash_lite` is the google.ts table (Vertex has none).
fn google_budget(
    model_id: &str,
    effort: &ThinkingLevel,
    custom: Option<&ThinkingBudgets>,
    flash_lite: bool,
) -> i64 {
    let index = match effort {
        ThinkingLevel::Minimal => 0,
        ThinkingLevel::Low => 1,
        ThinkingLevel::Medium => 2,
        _ => 3,
    };
    if let Some(budget) = custom.and_then(|b| [b.minimal, b.low, b.medium, b.high][index]) {
        return budget as i64;
    }
    let table: [i64; 4] = if model_id.contains("2.5-pro") {
        [128, 2048, 8192, 32768]
    } else if flash_lite && model_id.contains("2.5-flash-lite") {
        [512, 2048, 8192, 24576]
    } else if model_id.contains("2.5-flash") {
        [128, 2048, 8192, 24576]
    } else {
        return -1;
    };
    table[index]
}

/// `getDisabledThinkingConfig`: Gemini 3 cannot turn thinking fully off.
fn disabled_thinking_config(model_id: &str, gemma: bool) -> Value {
    if is_gemini3_pro(model_id) {
        json!({"thinkingLevel": "LOW"})
    } else if is_gemini3_flash(model_id) || (gemma && is_gemma4(model_id)) {
        json!({"thinkingLevel": "MINIMAL"})
    } else {
        json!({"thinkingBudget": 0})
    }
}

// ---------------------------------------------------------------------------
// buildParams
// ---------------------------------------------------------------------------

/// `buildParams`: the SDK's `GenerateContentParameters` (`{model, contents,
/// config}`). `vertex` selects google-vertex.ts (no Gemma 4 rule). Fails with
/// `Request aborted` when the signal already fired.
pub fn build_params(
    model: &Model,
    context: &Context,
    options: &GoogleOptions,
    vertex: bool,
) -> Result<Value, String> {
    let contents = convert_messages(model, context);
    let mut config = Map::new();
    if let Some(temperature) = options.temperature {
        config.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = options.max_tokens {
        config.insert("maxOutputTokens".into(), json!(max_tokens));
    }
    if !context.system_prompt.is_empty() {
        config.insert("systemInstruction".into(), json!(context.system_prompt));
    }
    if let Some(tools) = convert_tools(&context.tools, false) {
        config.insert("tools".into(), tools);
        if let Some(choice) = &options.tool_choice {
            config.insert(
                "toolConfig".into(),
                json!({"functionCallingConfig": {"mode": map_tool_choice(choice)}}),
            );
        }
    }

    match &options.thinking {
        Some(thinking) if thinking.enabled && model.reasoning => {
            let mut thinking_config = json!({"includeThoughts": true});
            if let Some(level) = &thinking.level {
                thinking_config["thinkingLevel"] = json!(level);
            } else if let Some(budget) = thinking.budget_tokens {
                thinking_config["thinkingBudget"] = json!(budget);
            }
            config.insert("thinkingConfig".into(), thinking_config);
        }
        Some(thinking) if model.reasoning && !thinking.enabled => {
            config.insert(
                "thinkingConfig".into(),
                disabled_thinking_config(&model.id, !vertex),
            );
        }
        _ => {}
    }

    if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
        return Err("Request aborted".to_string());
    }

    Ok(json!({"model": model.id, "contents": contents, "config": config}))
}

// ---------------------------------------------------------------------------
// @google/genai: REST body
// ---------------------------------------------------------------------------

/// `partToMldev` / `partToVertex` for the fields hoocode sends, in the SDK's
/// key order (mldev also reorders `functionCall` and `inlineData`).
fn sdk_part(part: &Value, vertex: bool) -> Value {
    let mut out = Map::new();
    if let Some(call) = part.get("functionCall") {
        let call = if vertex {
            call.clone()
        } else {
            let mut ordered = Map::new();
            for key in ["id", "args", "name"] {
                if let Some(v) = call.get(key).filter(|v| !v.is_null()) {
                    ordered.insert(key.into(), v.clone());
                }
            }
            Value::Object(ordered)
        };
        out.insert("functionCall".into(), call);
    }
    if let Some(response) = part.get("functionResponse") {
        out.insert("functionResponse".into(), response.clone());
    }
    if let Some(data) = part.get("inlineData") {
        let data = if vertex {
            data.clone()
        } else {
            json!({"data": data["data"], "mimeType": data["mimeType"]})
        };
        out.insert("inlineData".into(), data);
    }
    for key in ["text", "thought", "thoughtSignature"] {
        if let Some(v) = part.get(key).filter(|v| !v.is_null()) {
            out.insert(key.into(), v.clone());
        }
    }
    Value::Object(out)
}

/// `contentToMldev` / `contentToVertex`: `{parts, role}`.
fn sdk_content(content: &Value, vertex: bool) -> Value {
    let parts: Vec<Value> = content["parts"]
        .as_array()
        .map(|parts| parts.iter().map(|p| sdk_part(p, vertex)).collect())
        .unwrap_or_default();
    json!({"parts": parts, "role": content["role"]})
}

/// `generateContentParametersToMldev` / `…ToVertex`: the REST request body.
pub fn sdk_body(params: &Value, vertex: bool) -> Value {
    let mut body = Map::new();
    let contents: Vec<Value> = params["contents"]
        .as_array()
        .map(|c| c.iter().map(|c| sdk_content(c, vertex)).collect())
        .unwrap_or_default();
    body.insert("contents".into(), json!(contents));

    let config = &params["config"];
    let mut generation = Map::new();
    if let Some(Value::String(system)) = config.get("systemInstruction") {
        // tContent(string): a user content with one text part.
        body.insert(
            "systemInstruction".into(),
            sdk_content(
                &json!({"role": "user", "parts": [{"text": system}]}),
                vertex,
            ),
        );
    }
    for key in ["temperature", "maxOutputTokens"] {
        if let Some(v) = config.get(key) {
            generation.insert(key.into(), v.clone());
        }
    }
    if let Some(Value::Array(tools)) = config.get("tools") {
        let tools: Vec<Value> = tools
            .iter()
            .map(|tool| {
                let declarations: Vec<Value> = tool["functionDeclarations"]
                    .as_array()
                    .map(|decls| {
                        decls
                            .iter()
                            .map(|d| {
                                if !vertex {
                                    return d.clone();
                                }
                                // functionDeclarationToVertex order.
                                let mut ordered = Map::new();
                                for key in
                                    ["description", "name", "parameters", "parametersJsonSchema"]
                                {
                                    if let Some(v) = d.get(key) {
                                        ordered.insert(key.into(), v.clone());
                                    }
                                }
                                Value::Object(ordered)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                json!({"functionDeclarations": declarations})
            })
            .collect();
        body.insert("tools".into(), json!(tools));
    }
    if let Some(tool_config) = config.get("toolConfig") {
        body.insert("toolConfig".into(), tool_config.clone());
    }
    if let Some(thinking) = config.get("thinkingConfig") {
        generation.insert("thinkingConfig".into(), thinking.clone());
    }
    body.insert("generationConfig".into(), Value::Object(generation));
    Value::Object(body)
}

// ---------------------------------------------------------------------------
// @google/genai: client configuration
// ---------------------------------------------------------------------------

/// `httpOptions` passed to `new GoogleGenAI(...)`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HttpOptions {
    pub base_url: Option<String>,
    /// `COLLECTION` when a custom Vertex base URL is used.
    pub base_url_resource_scope: Option<String>,
    pub api_version: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

/// The constructor options of `GoogleGenAI` as hoocode builds them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientConfig {
    pub vertexai: bool,
    pub api_key: Option<String>,
    pub project: Option<String>,
    pub location: Option<String>,
    pub api_version: Option<String>,
    pub http_options: Option<HttpOptions>,
}

fn merged_headers(model: &Model, options: &GoogleOptions) -> Option<HashMap<String, String>> {
    if model.headers.is_none() && options.headers.is_none() {
        return None;
    }
    let mut headers = model.headers.clone().unwrap_or_default();
    headers.extend(options.headers.clone().unwrap_or_default());
    Some(headers)
}

/// google.ts `createClient`: the model's base URL already carries the API
/// version.
pub fn gemini_client_config(model: &Model, api_key: &str, options: &GoogleOptions) -> ClientConfig {
    let mut http = HttpOptions::default();
    if !model.base_url.is_empty() {
        http.base_url = Some(model.base_url.clone());
        http.api_version = Some(String::new());
    }
    http.headers = merged_headers(model, options);
    ClientConfig {
        vertexai: false,
        api_key: Some(api_key.to_string()),
        http_options: (http != HttpOptions::default()).then_some(http),
        ..Default::default()
    }
}

/// google-vertex.ts `resolveApiKey`: options, then `GOOGLE_CLOUD_API_KEY`,
/// ignoring the ADC marker and `<placeholder>` values.
fn resolve_vertex_api_key(options: &GoogleOptions) -> Option<String> {
    let key = options
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("GOOGLE_CLOUD_API_KEY")
                .ok()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
        })?;
    let placeholder = key.len() > 2
        && key.starts_with('<')
        && key.ends_with('>')
        && !key[1..key.len() - 1].contains('>');
    (key != GCP_VERTEX_CREDENTIALS_MARKER && !placeholder).then_some(key)
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// `baseUrlIncludesApiVersion`: a `v1`/`v1beta…` path segment.
fn base_url_includes_api_version(base_url: &str) -> bool {
    let is_version = |part: &str| {
        let Some(rest) = part.strip_prefix('v') else {
            return false;
        };
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return false;
        }
        let rest = &rest[digits..];
        rest.is_empty()
            || rest
                .strip_prefix("beta")
                .is_some_and(|d| d.chars().all(|c| c.is_ascii_digit()))
    };
    match reqwest::Url::parse(base_url) {
        Ok(url) => url.path().split('/').any(is_version),
        Err(_) => base_url.split('/').any(is_version),
    }
}

/// google-vertex.ts `buildHttpOptions`.
fn vertex_http_options(model: &Model, options: &GoogleOptions) -> Option<HttpOptions> {
    let mut http = HttpOptions::default();
    let base_url = model.base_url.trim();
    if !base_url.is_empty() && !base_url.contains("{location}") {
        http.base_url = Some(base_url.to_string());
        http.base_url_resource_scope = Some("COLLECTION".into());
        if base_url_includes_api_version(base_url) {
            http.api_version = Some(String::new());
        }
    }
    http.headers = merged_headers(model, options);
    (http != HttpOptions::default()).then_some(http)
}

/// google-vertex.ts client choice: a real API key (express mode), else ADC
/// with project and location (both required).
pub fn vertex_client_config(
    model: &Model,
    options: &GoogleOptions,
) -> Result<ClientConfig, String> {
    let http_options = vertex_http_options(model, options);
    if let Some(api_key) = resolve_vertex_api_key(options) {
        return Ok(ClientConfig {
            vertexai: true,
            api_key: Some(api_key),
            api_version: Some(VERTEX_API_VERSION.into()),
            http_options,
            ..Default::default()
        });
    }
    let project = options
        .project
        .clone()
        .filter(|p| !p.is_empty())
        .or_else(|| env("GOOGLE_CLOUD_PROJECT"))
        .or_else(|| env("GCLOUD_PROJECT"))
        .ok_or_else(|| {
            "Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass project in options."
                .to_string()
        })?;
    let location = options
        .location
        .clone()
        .filter(|l| !l.is_empty())
        .or_else(|| env("GOOGLE_CLOUD_LOCATION"))
        .ok_or_else(|| {
            "Vertex AI requires a location. Set GOOGLE_CLOUD_LOCATION or pass location in options."
                .to_string()
        })?;
    Ok(ClientConfig {
        vertexai: true,
        project: Some(project),
        location: Some(location),
        api_version: Some(VERTEX_API_VERSION.into()),
        http_options,
        ..Default::default()
    })
}

/// How a request authenticates.
#[derive(Debug, Clone, PartialEq)]
pub enum Auth {
    /// `x-goog-api-key`.
    ApiKey(String),
    /// Application Default Credentials (`Authorization: Bearer …`).
    Adc,
}

/// What `ApiClient` sends for `models.generateContentStream`.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub auth: Auth,
}

impl ClientConfig {
    /// `ApiClient`'s base URL, API version and project path, then the
    /// `:streamGenerateContent?alt=sse` URL and default headers.
    pub fn endpoint(&self, model_id: &str) -> Endpoint {
        let http = self.http_options.clone().unwrap_or_default();
        let (default_base, default_version) = if self.vertexai {
            let location = self.location.as_deref();
            let base = if self.api_key.is_some() || location == Some("global") {
                "https://aiplatform.googleapis.com/".to_string()
            } else if let Some(loc @ ("us" | "eu")) = location {
                format!("https://aiplatform.{loc}.rep.googleapis.com/")
            } else {
                format!(
                    "https://{}-aiplatform.googleapis.com/",
                    location.unwrap_or("global")
                )
            };
            (base, self.api_version.clone().unwrap_or("v1beta1".into()))
        } else {
            (
                "https://generativelanguage.googleapis.com/".to_string(),
                self.api_version.clone().unwrap_or("v1beta".into()),
            )
        };
        // The node client falls back to GOOGLE_{VERTEX,GEMINI}_BASE_URL.
        let env_base = if self.vertexai {
            env("GOOGLE_VERTEX_BASE_URL")
        } else {
            env("GOOGLE_GEMINI_BASE_URL")
        };
        let base = http.base_url.clone().or(env_base).unwrap_or(default_base);
        let version = http.api_version.clone().unwrap_or(default_version);

        let mut elements = vec![base.strip_suffix('/').unwrap_or(&base).to_string()];
        if !version.is_empty() {
            elements.push(version);
        }
        let collection_scope = http.base_url.is_some()
            && http.base_url_resource_scope.as_deref() == Some("COLLECTION");
        if self.vertexai && self.api_key.is_none() && !collection_scope {
            elements.push(format!(
                "projects/{}/locations/{}",
                self.project.as_deref().unwrap_or_default(),
                self.location.as_deref().unwrap_or_default()
            ));
        }
        let model_path = if self.vertexai {
            format!("publishers/google/models/{model_id}")
        } else {
            format!("models/{model_id}")
        };
        elements.push(format!("{model_path}:streamGenerateContent"));
        let url = format!("{}?alt=sse", elements.join("/"));

        // getDefaultHeaders, then httpOptions.headers.
        let label = format!("google-genai-sdk/{SDK_VERSION} gl-rust/cortexcode");
        let mut headers = vec![
            ("User-Agent".to_string(), label.clone()),
            ("x-goog-api-client".to_string(), label),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        for (k, v) in http.headers.unwrap_or_default() {
            match headers
                .iter_mut()
                .find(|(name, _)| name.eq_ignore_ascii_case(&k))
            {
                Some(slot) => *slot = (k, v),
                None => headers.push((k, v)),
            }
        }
        let auth = match &self.api_key {
            Some(key) => Auth::ApiKey(key.clone()),
            None => Auth::Adc,
        };
        Endpoint { url, headers, auth }
    }
}

#[cfg(test)]
#[path = "request_tests.rs"]
pub(crate) mod tests;
