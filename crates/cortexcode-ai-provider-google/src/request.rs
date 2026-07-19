//! Request construction for Google Generative AI and Vertex AI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/google.ts`
//! and `providers/google-vertex.ts` (request-building portion).

use cortexcode_ai_types::{Context, Model, SimpleStreamOptions, ThinkingBudgets, ThinkingLevel};

use crate::shared::{convert_messages, convert_tools};

/// Resolve the Google Generative AI API key from options, then the environment.
pub fn resolve_gemini_credentials(options: &SimpleStreamOptions) -> Result<String, String> {
    if let Some(key) = &options.api_key {
        if !key.is_empty() {
            return Ok(key.clone());
        }
    }
    if let Ok(key) = std::env::var("GEMINI_API_KEY") {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err("no Google credentials found (set GEMINI_API_KEY)".into())
}

/// Vertex AI credentials this pass supports: an already-minted OAuth2 access
/// token. Full Application Default Credentials (service-account JWT signing
/// and token exchange) is not yet ported — see the crate README.
pub struct VertexCredentials {
    pub access_token: String,
    pub project: String,
    pub location: String,
}

pub fn resolve_vertex_credentials(
    options: &SimpleStreamOptions,
) -> Result<VertexCredentials, String> {
    let access_token = options
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .or_else(|| std::env::var("GOOGLE_VERTEX_ACCESS_TOKEN").ok().filter(|k| !k.is_empty()))
        .ok_or_else(|| {
            "no Vertex AI access token found (set GOOGLE_VERTEX_ACCESS_TOKEN, or pass one as api_key — \
             full Application Default Credentials support is not yet implemented)"
                .to_string()
        })?;

    let project = std::env::var("GOOGLE_CLOUD_PROJECT")
        .or_else(|_| std::env::var("GCLOUD_PROJECT"))
        .map_err(|_| {
            "Vertex AI requires a project ID (set GOOGLE_CLOUD_PROJECT or GCLOUD_PROJECT)"
                .to_string()
        })?;
    let location = std::env::var("GOOGLE_CLOUD_LOCATION")
        .map_err(|_| "Vertex AI requires a location (set GOOGLE_CLOUD_LOCATION)".to_string())?;

    Ok(VertexCredentials {
        access_token,
        project,
        location,
    })
}

/// Build the `:streamGenerateContent?alt=sse` request body shared by both
/// Google Generative AI and Vertex AI (they use the same wire format).
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: &SimpleStreamOptions,
) -> serde_json::Value {
    let contents = convert_messages(model, context);

    let mut body = serde_json::json!({
        "contents": contents,
        "generationConfig": {"maxOutputTokens": model.max_tokens},
    });
    if !context.system_prompt.is_empty() {
        body["systemInstruction"] = serde_json::json!({"parts": [{"text": context.system_prompt}]});
    }
    if !context.tools.is_empty() {
        if let Some(tools) = convert_tools(&context.tools, false) {
            body["tools"] = tools;
        }
    }

    if model.reasoning {
        if let Some(level) = &options.reasoning {
            body["generationConfig"]["thinkingConfig"] =
                build_thinking_config(model, level, options.thinking_budgets.as_ref());
        }
    }

    body
}

fn is_gemini3_pro(model_id: &str) -> bool {
    let lower = model_id.to_lowercase();
    lower.contains("gemini-3") && lower.contains("-pro")
}

fn is_gemini3_flash(model_id: &str) -> bool {
    let lower = model_id.to_lowercase();
    lower.contains("gemini-3") && lower.contains("-flash")
}

fn is_gemma4(model_id: &str) -> bool {
    let lower = model_id.to_lowercase();
    lower.contains("gemma-4") || lower.contains("gemma4")
}

fn build_thinking_config(
    model: &Model,
    level: &ThinkingLevel,
    budgets: Option<&ThinkingBudgets>,
) -> serde_json::Value {
    if *level == ThinkingLevel::Off {
        return disabled_thinking_config(&model.id);
    }

    let effort = if *level == ThinkingLevel::XHigh {
        &ThinkingLevel::High
    } else {
        level
    };

    if is_gemini3_pro(&model.id) || is_gemini3_flash(&model.id) || is_gemma4(&model.id) {
        return serde_json::json!({
            "includeThoughts": true,
            "thinkingLevel": thinking_level_string(&model.id, effort),
        });
    }

    serde_json::json!({
        "includeThoughts": true,
        "thinkingBudget": resolve_budget(&model.id, effort, budgets),
    })
}

fn disabled_thinking_config(model_id: &str) -> serde_json::Value {
    if is_gemini3_pro(model_id) {
        return serde_json::json!({"thinkingLevel": "LOW"});
    }
    if is_gemini3_flash(model_id) || is_gemma4(model_id) {
        return serde_json::json!({"thinkingLevel": "MINIMAL"});
    }
    serde_json::json!({"thinkingBudget": 0})
}

fn thinking_level_string(model_id: &str, level: &ThinkingLevel) -> &'static str {
    if is_gemini3_pro(model_id) {
        return match level {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "LOW",
            _ => "HIGH",
        };
    }
    if is_gemma4(model_id) {
        return match level {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "MINIMAL",
            _ => "HIGH",
        };
    }
    match level {
        ThinkingLevel::Minimal => "MINIMAL",
        ThinkingLevel::Low => "LOW",
        ThinkingLevel::Medium => "MEDIUM",
        _ => "HIGH",
    }
}

fn resolve_budget(model_id: &str, level: &ThinkingLevel, budgets: Option<&ThinkingBudgets>) -> i64 {
    if let Some(b) = budgets {
        let explicit = match level {
            ThinkingLevel::Minimal => b.minimal,
            ThinkingLevel::Low => b.low,
            ThinkingLevel::Medium => b.medium,
            ThinkingLevel::High | ThinkingLevel::XHigh => b.high,
            ThinkingLevel::Off => None,
        };
        if let Some(v) = explicit {
            return v as i64;
        }
    }

    let table: &[(&str, [i64; 4])] = &[
        ("2.5-pro", [128, 2048, 8192, 32768]),
        ("2.5-flash-lite", [512, 2048, 8192, 24576]),
        ("2.5-flash", [128, 2048, 8192, 24576]),
    ];

    for (needle, budgets) in table {
        if model_id.contains(needle) {
            return match level {
                ThinkingLevel::Minimal => budgets[0],
                ThinkingLevel::Low => budgets[1],
                ThinkingLevel::Medium => budgets[2],
                _ => budgets[3],
            };
        }
    }

    -1
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::Context;

    fn model(id: &str, reasoning: bool) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: "google-generative-ai".into(),
            provider: "google".into(),
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            reasoning,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: cortexcode_ai_types::ModelCost::default(),
            context_window: 1_000_000,
            max_tokens: 8192,
            headers: None,
        }
    }

    #[test]
    fn test_resolve_gemini_credentials_explicit() {
        let options = SimpleStreamOptions {
            api_key: Some("key123".into()),
            ..Default::default()
        };
        assert_eq!(resolve_gemini_credentials(&options).unwrap(), "key123");
    }

    #[test]
    fn test_resolve_vertex_credentials_missing_token() {
        std::env::remove_var("GOOGLE_VERTEX_ACCESS_TOKEN");
        let options = SimpleStreamOptions::default();
        assert!(resolve_vertex_credentials(&options).is_err());
    }

    #[test]
    fn test_build_request_body_basic() {
        let m = model("gemini-2.0-flash", false);
        let ctx = Context::new("be nice".into(), vec![], vec![]);
        let body = build_request_body(&m, &ctx, &SimpleStreamOptions::default());
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be nice");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 8192);
    }

    #[test]
    fn test_build_thinking_config_budget_model() {
        let m = model("gemini-2.5-pro", true);
        let ctx = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..Default::default()
        };
        let body = build_request_body(&m, &ctx, &options);
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            32768
        );
    }

    #[test]
    fn test_build_thinking_config_gemini3_pro_level() {
        let m = model("gemini-3-pro", true);
        let ctx = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Medium),
            ..Default::default()
        };
        let body = build_request_body(&m, &ctx, &options);
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "HIGH"
        );
    }

    #[test]
    fn test_build_thinking_config_off_gemini3_pro() {
        let m = model("gemini-3-pro", true);
        let ctx = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Off),
            ..Default::default()
        };
        let body = build_request_body(&m, &ctx, &options);
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
    }

    #[test]
    fn test_build_thinking_config_custom_budget_override() {
        let m = model("gemini-2.5-pro", true);
        let ctx = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Low),
            thinking_budgets: Some(ThinkingBudgets {
                minimal: None,
                low: Some(999),
                medium: None,
                high: None,
                xhigh: None,
            }),
            ..Default::default()
        };
        let body = build_request_body(&m, &ctx, &options);
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            999
        );
    }
}
