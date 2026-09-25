//! Ported from `google-vertex-api-key-resolution.test.ts` (the `GoogleGenAI`
//! constructor options become [`ClientConfig`]) plus the payload side of
//! `google-thinking-disable.test.ts` and the SDK's REST mapping.

use super::*;
use cortexcode_ai_types::{Message, UserMessage};
use std::sync::{Mutex, MutexGuard};

/// Serializes tests that read or set the Google env vars.
pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const VERTEX_VARS: &[&str] = &[
    "GOOGLE_CLOUD_API_KEY",
    "GOOGLE_CLOUD_PROJECT",
    "GCLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    "GOOGLE_VERTEX_BASE_URL",
];

pub(crate) fn clear_vertex_env() -> Vec<(&'static str, Option<String>)> {
    VERTEX_VARS
        .iter()
        .map(|v| {
            let old = std::env::var(v).ok();
            std::env::remove_var(v);
            (*v, old)
        })
        .collect()
}

pub(crate) fn restore_env(saved: Vec<(&'static str, Option<String>)>) {
    for (var, value) in saved {
        match value {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
    }
}

fn catalog(provider: &str, id: &str) -> Model {
    cortexcode_ai_models::get_model(provider, id)
        .unwrap_or_else(|| panic!("{provider}/{id} in catalog"))
        .clone()
}

fn vertex_options(api_key: Option<&str>, project: bool) -> GoogleOptions {
    GoogleOptions {
        api_key: api_key.map(str::to_string),
        project: project.then(|| "test-project".to_string()),
        location: project.then(|| "us-central1".to_string()),
        ..Default::default()
    }
}

fn adc_config(model: &Model, options: &GoogleOptions) -> ClientConfig {
    let _env = env_lock();
    let saved = clear_vertex_env();
    let config = vertex_client_config(model, options);
    restore_env(saved);
    config.unwrap()
}

fn expect_adc(config: &ClientConfig) {
    assert!(config.vertexai);
    assert_eq!(config.project.as_deref(), Some("test-project"));
    assert_eq!(config.location.as_deref(), Some("us-central1"));
    assert_eq!(config.api_version.as_deref(), Some("v1"));
    assert_eq!(config.api_key, None);
}

// --- google-vertex-api-key-resolution.test.ts ---

#[test]
fn falls_back_to_adc_for_placeholder_and_marker_api_keys() {
    let model = catalog("google-vertex", "gemini-3-flash-preview");
    for key in ["<authenticated>", "gcp-vertex-credentials"] {
        expect_adc(&adc_config(&model, &vertex_options(Some(key), true)));
    }
}

#[test]
fn falls_back_to_adc_when_google_cloud_api_key_is_a_placeholder() {
    let model = catalog("google-vertex", "gemini-3-flash-preview");
    let _env = env_lock();
    let saved = clear_vertex_env();
    std::env::set_var("GOOGLE_CLOUD_API_KEY", "<authenticated>");
    let config = vertex_client_config(&model, &vertex_options(None, true));
    restore_env(saved);
    expect_adc(&config.unwrap());
}

#[test]
fn still_uses_the_api_key_client_for_real_api_keys() {
    let model = catalog("google-vertex", "gemini-3-flash-preview");
    let config = adc_config(
        &model,
        &vertex_options(Some("AIzaSyExampleRealisticLookingApiKey123456"), false),
    );
    assert!(config.vertexai);
    assert_eq!(
        config.api_key.as_deref(),
        Some("AIzaSyExampleRealisticLookingApiKey123456")
    );
    assert_eq!(config.api_version.as_deref(), Some("v1"));
    assert_eq!((config.project, config.location), (None, None));
}

#[test]
fn does_not_forward_generated_vertex_base_url_placeholders() {
    let model = catalog("google-vertex", "gemini-3-flash-preview");
    let config = adc_config(&model, &vertex_options(None, true));
    assert_eq!(config.http_options, None);
}

#[test]
fn forwards_a_custom_base_url_to_the_adc_and_api_key_clients() {
    let mut model = catalog("google-vertex", "gemini-3-flash-preview");
    model.base_url = "https://proxy.example.com".into();
    let expected = Some(HttpOptions {
        base_url: Some("https://proxy.example.com".into()),
        base_url_resource_scope: Some("COLLECTION".into()),
        api_version: None,
        headers: None,
    });
    let adc = adc_config(&model, &vertex_options(None, true));
    expect_adc(&adc);
    assert_eq!(adc.http_options, expected);
    let keyed = adc_config(
        &model,
        &vertex_options(Some("AIzaSyExampleRealisticLookingApiKey123456"), false),
    );
    assert!(keyed.api_key.is_some());
    assert_eq!(keyed.http_options, expected);
}

#[test]
fn does_not_append_the_api_version_when_the_base_url_has_one() {
    let mut model = catalog("google-vertex", "gemini-3-flash-preview");
    model.base_url = "https://proxy.example.com/v1/projects/test-project/locations/global".into();
    let config = adc_config(&model, &vertex_options(None, true));
    assert_eq!(
        config.http_options,
        Some(HttpOptions {
            base_url: Some(model.base_url.clone()),
            base_url_resource_scope: Some("COLLECTION".into()),
            api_version: Some(String::new()),
            headers: None,
        })
    );
}

#[test]
fn vertex_requires_project_and_location_without_an_api_key() {
    let model = catalog("google-vertex", "gemini-3-flash-preview");
    let _env = env_lock();
    let saved = clear_vertex_env();
    let no_project = vertex_client_config(&model, &GoogleOptions::default());
    std::env::set_var("GCLOUD_PROJECT", "p");
    let no_location = vertex_client_config(&model, &GoogleOptions::default());
    restore_env(saved);
    assert_eq!(
        no_project.err().as_deref(),
        Some("Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass project in options.")
    );
    assert_eq!(
        no_location.err().as_deref(),
        Some(
            "Vertex AI requires a location. Set GOOGLE_CLOUD_LOCATION or pass location in options."
        )
    );
}

// --- ApiClient URLs ---

#[test]
fn endpoints_follow_the_sdk_api_client() {
    let _env = env_lock();
    let saved = clear_vertex_env();
    let adc = |location: &str| ClientConfig {
        vertexai: true,
        project: Some("p".into()),
        location: Some(location.into()),
        api_version: Some("v1".into()),
        ..Default::default()
    };
    let url = |config: ClientConfig| config.endpoint("gemini-3-flash-preview").url;
    let regional = url(adc("us-central1"));
    let global = url(adc("global"));
    let multi = url(adc("eu"));
    let express = ClientConfig {
        vertexai: true,
        api_key: Some("k".into()),
        api_version: Some("v1".into()),
        ..Default::default()
    };
    let express_endpoint = express.endpoint("gemini-3-flash-preview");
    let proxied = url(ClientConfig {
        http_options: Some(HttpOptions {
            base_url: Some("https://proxy.example.com/v1/projects/p/locations/global".into()),
            base_url_resource_scope: Some("COLLECTION".into()),
            api_version: Some(String::new()),
            headers: None,
        }),
        ..adc("us-central1")
    });
    let gemini = gemini_client_config(
        &catalog("google", "gemini-2.5-flash"),
        "gk",
        &GoogleOptions::default(),
    )
    .endpoint("gemini-2.5-flash");
    restore_env(saved);

    assert_eq!(regional, "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse");
    assert_eq!(global, "https://aiplatform.googleapis.com/v1/projects/p/locations/global/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse");
    assert_eq!(multi, "https://aiplatform.eu.rep.googleapis.com/v1/projects/p/locations/eu/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse");
    assert_eq!(express_endpoint.url, "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse");
    assert_eq!(express_endpoint.auth, Auth::ApiKey("k".into()));
    assert_eq!(proxied, "https://proxy.example.com/v1/projects/p/locations/global/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse");
    assert_eq!(gemini.url, "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse");
    assert_eq!(gemini.auth, Auth::ApiKey("gk".into()));
}

// --- google-thinking-disable.test.ts (payloads) and the simple mappings ---

fn hello() -> Context {
    Context::new(
        String::new(),
        vec![Message::User(UserMessage {
            content: "Hello".into(),
            timestamp: 1,
        })],
        vec![],
    )
}

fn thinking_config(provider: &str, id: &str, level: Option<ThinkingLevel>) -> Value {
    let model = catalog(provider, id);
    let simple = SimpleStreamOptions {
        reasoning: level,
        ..Default::default()
    };
    let vertex = provider == "google-vertex";
    let options = if vertex {
        simple_vertex_options(&model, &simple)
    } else {
        simple_google_options(&model, &simple, "k".into())
    };
    build_params(&model, &hello(), &options, vertex).unwrap()["config"]["thinkingConfig"].clone()
}

#[test]
fn thinking_off_disables_thinking_as_each_family_allows() {
    assert_eq!(
        thinking_config("google", "gemini-2.5-flash", None),
        json!({"thinkingBudget": 0})
    );
    assert_eq!(
        thinking_config("google", "gemini-3-flash-preview", None),
        json!({"thinkingLevel": "MINIMAL"})
    );
    assert_eq!(
        thinking_config("google", "gemini-3.1-pro-preview", None),
        json!({"thinkingLevel": "LOW"})
    );
    assert_eq!(
        thinking_config("google", "gemma-4-31b-it", None),
        json!({"thinkingLevel": "MINIMAL"})
    );
    assert_eq!(
        thinking_config("google-vertex", "gemini-2.5-flash", None),
        json!({"thinkingBudget": 0})
    );
    assert_eq!(
        thinking_config("google-vertex", "gemini-3-flash-preview", None),
        json!({"thinkingLevel": "MINIMAL"})
    );
}

#[test]
fn thinking_levels_and_budgets_per_family() {
    let high = Some(ThinkingLevel::High);
    assert_eq!(
        thinking_config("google", "gemini-3-flash-preview", high.clone()),
        json!({"includeThoughts": true, "thinkingLevel": "HIGH"})
    );
    assert_eq!(
        thinking_config(
            "google",
            "gemini-3.1-pro-preview",
            Some(ThinkingLevel::Medium)
        ),
        json!({"includeThoughts": true, "thinkingLevel": "HIGH"})
    );
    assert_eq!(
        thinking_config("google", "gemini-2.5-pro", high.clone()),
        json!({"includeThoughts": true, "thinkingBudget": 32768})
    );
    assert_eq!(
        thinking_config(
            "google",
            "gemini-2.5-flash-lite",
            Some(ThinkingLevel::Minimal)
        ),
        json!({"includeThoughts": true, "thinkingBudget": 512})
    );
    assert_eq!(
        thinking_config("google-vertex", "gemini-2.5-flash", high),
        json!({"includeThoughts": true, "thinkingBudget": 24576})
    );
}

#[test]
fn params_carry_generation_settings_tools_and_tool_choice() {
    let model = catalog("google", "gemini-2.5-flash");
    let mut context = hello();
    context.system_prompt = "sys".into();
    context.tools = vec![cortexcode_ai_types::Tool {
        name: "read".into(),
        description: "Read".into(),
        parameters: json!({"type": "object"}),
        defer_loading: None,
    }];
    let options = GoogleOptions {
        temperature: Some(0.0),
        max_tokens: Some(100),
        tool_choice: Some("any".into()),
        ..Default::default()
    };
    let params = build_params(&model, &context, &options, false).unwrap();
    assert_eq!(
        params,
        json!({
            "model": "gemini-2.5-flash",
            "contents": [{"role": "user", "parts": [{"text": "Hello"}]}],
            "config": {
                "temperature": 0.0,
                "maxOutputTokens": 100,
                "systemInstruction": "sys",
                "tools": [{"functionDeclarations": [{"name": "read", "description": "Read", "parametersJsonSchema": {"type": "object"}}]}],
                "toolConfig": {"functionCallingConfig": {"mode": "ANY"}},
            },
        })
    );
    // The REST body the SDK sends.
    let rest = sdk_body(&params, false);
    assert_eq!(
        serde_json::to_string(&rest).unwrap(),
        r#"{"contents":[{"parts":[{"text":"Hello"}],"role":"user"}],"systemInstruction":{"parts":[{"text":"sys"}],"role":"user"},"tools":[{"functionDeclarations":[{"name":"read","description":"Read","parametersJsonSchema":{"type":"object"}}]}],"toolConfig":{"functionCallingConfig":{"mode":"ANY"}},"generationConfig":{"temperature":0.0,"maxOutputTokens":100}}"#
    );
    let vertex = sdk_body(&params, true);
    assert_eq!(
        vertex["tools"][0]["functionDeclarations"][0]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["description", "name", "parametersJsonSchema"]
    );
}

#[test]
fn sdk_parts_are_reordered_like_part_to_mldev() {
    let params = json!({"contents": [{"role": "model", "parts": [
        {"thought": true, "text": "t", "thoughtSignature": "s"},
        {"functionCall": {"name": "n", "args": {}, "id": "i"}, "thoughtSignature": "s"},
        {"inlineData": {"mimeType": "image/png", "data": "d"}},
    ]}], "config": {}});
    assert_eq!(
        serde_json::to_string(&sdk_body(&params, false)["contents"]).unwrap(),
        r#"[{"parts":[{"text":"t","thought":true,"thoughtSignature":"s"},{"functionCall":{"id":"i","args":{},"name":"n"},"thoughtSignature":"s"},{"inlineData":{"data":"d","mimeType":"image/png"}}],"role":"model"}]"#
    );
}

#[test]
fn an_aborted_signal_fails_build_params() {
    let signal = AbortSignal::new();
    signal.abort();
    let options = GoogleOptions {
        signal: Some(signal),
        ..Default::default()
    };
    assert_eq!(
        build_params(
            &catalog("google", "gemini-2.5-flash"),
            &hello(),
            &options,
            false
        ),
        Err("Request aborted".to_string())
    );
}
