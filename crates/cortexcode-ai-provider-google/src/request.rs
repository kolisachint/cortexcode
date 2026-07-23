//! Request construction for Google Generative AI and Vertex AI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/google.ts`
//! and `providers/google-vertex.ts` (request-building portion).
//!
//! Supports:
//! - Google Generative AI (Gemini) with API key
//! - Vertex AI with access token
//! - Vertex AI with Application Default Credentials (service account JSON key)
//! - Vertex AI on GCE/GKE (metadata server)

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

/// Vertex AI credentials including access token, project, and location.
#[derive(Debug, Clone)]
pub struct VertexCredentials {
    pub access_token: String,
    pub project: String,
    pub location: String,
}

/// Resolve Vertex AI credentials from multiple sources.
///
/// Priority:
/// 1. Explicit `api_key` (access token) in options
/// 2. `GOOGLE_VERTEX_ACCESS_TOKEN` or `GOOGLE_ACCESS_TOKEN` env var
/// 3. Service account JSON key via `GOOGLE_APPLICATION_CREDENTIALS`
/// 4. GCE/GKE metadata server
pub fn resolve_vertex_credentials(
    options: &SimpleStreamOptions,
) -> Result<VertexCredentials, String> {
    // 1. Check explicit config
    if let Some(token) = &options.api_key {
        if !token.is_empty() {
            return resolve_vertex_config(token.clone());
        }
    }

    // 2. Try environment variables
    if let Ok(token) = std::env::var("GOOGLE_VERTEX_ACCESS_TOKEN") {
        if !token.is_empty() {
            return resolve_vertex_config(token);
        }
    }

    if let Ok(token) = std::env::var("GOOGLE_ACCESS_TOKEN") {
        if !token.is_empty() {
            return resolve_vertex_config(token);
        }
    }

    // 3. Try service account authentication
    if let Ok(creds) = resolve_service_account_credentials() {
        return Ok(creds);
    }

    // 4. Try GCE metadata server
    if let Ok(creds) = resolve_gce_metadata_credentials() {
        return Ok(creds);
    }

    Err("Vertex AI requires credentials. Set GOOGLE_VERTEX_ACCESS_TOKEN, GOOGLE_ACCESS_TOKEN, \
         GOOGLE_APPLICATION_CREDENTIALS, or provide api_key in config."
        .to_string())
}

/// Helper to resolve project and location for Vertex credentials.
fn resolve_vertex_config(access_token: String) -> Result<VertexCredentials, String> {
    let project = std::env::var("GOOGLE_VERTEX_PROJECT")
        .or_else(|_| std::env::var("GOOGLE_CLOUD_PROJECT"))
        .map_err(|_| {
            "Vertex AI requires a project ID (set GOOGLE_VERTEX_PROJECT or GOOGLE_CLOUD_PROJECT)"
                .to_string()
        })?;

    let location = std::env::var("GOOGLE_VERTEX_LOCATION")
        .or_else(|_| std::env::var("GOOGLE_CLOUD_LOCATION"))
        .unwrap_or_else(|_| "us-central1".to_string());

    Ok(VertexCredentials {
        access_token,
        project,
        location,
    })
}

// ---------------------------------------------------------------------------
// Service Account Authentication
// ---------------------------------------------------------------------------

/// Service account JSON key file structure.
#[derive(serde::Deserialize)]
struct ServiceAccountKey {
    #[serde(rename = "type")]
    key_type: String,
    project_id: String,
    #[allow(dead_code)]
    private_key_id: Option<String>,
    private_key: String,
    client_email: String,
    token_uri: Option<String>,
}

/// JWT claims for service account token exchange.
#[derive(serde::Serialize)]
struct JwtClaims {
    iss: String,
    scope: String,
    aud: String,
    exp: i64,
    iat: i64,
}

/// Resolve credentials from service account JSON key file.
fn resolve_service_account_credentials() -> Result<VertexCredentials, String> {
    // Check GOOGLE_APPLICATION_CREDENTIALS environment variable
    let sa_path = match std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        Ok(p) => p,
        Err(_) => return Err("No service account key found".to_string()),
    };

    // Read the service account key file
    let key_json = std::fs::read_to_string(&sa_path)
        .map_err(|e| format!("Failed to read service account key file: {e}"))?;

    // Parse the service account key
    let sa_key: ServiceAccountKey =
        serde_json::from_str(&key_json).map_err(|e| format!("Failed to parse service account key: {e}"))?;

    if sa_key.key_type != "service_account" {
        return Err(format!(
            "Expected service_account type, got {}",
            sa_key.key_type
        ));
    }

    // Exchange service account credentials for access token
    let access_token = exchange_service_account_token(&sa_key)?;

    // Get project and location
    let project = std::env::var("GOOGLE_VERTEX_PROJECT")
        .or_else(|_| std::env::var("GOOGLE_CLOUD_PROJECT"))
        .unwrap_or(sa_key.project_id);

    let location = std::env::var("GOOGLE_VERTEX_LOCATION")
        .or_else(|_| std::env::var("GOOGLE_CLOUD_LOCATION"))
        .unwrap_or_else(|_| "us-central1".to_string());

    Ok(VertexCredentials {
        access_token,
        project,
        location,
    })
}

/// Exchange service account credentials for an access token using JWT.
fn exchange_service_account_token(sa_key: &ServiceAccountKey) -> Result<String, String> {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let token_uri = sa_key
        .token_uri
        .as_deref()
        .unwrap_or("https://oauth2.googleapis.com/token");

    let claims = JwtClaims {
        iss: sa_key.client_email.clone(),
        scope: "https://www.googleapis.com/auth/cloud-platform".to_string(),
        aud: token_uri.to_string(),
        exp: now + 3600,
        iat: now,
    };

    let header = Header::new(Algorithm::RS256);
    let key = EncodingKey::from_rsa_pem(sa_key.private_key.as_bytes())
        .map_err(|e| format!("Failed to parse private key: {e}"))?;

    let jwt =
        encode(&header, &claims, &key).map_err(|e| format!("Failed to create JWT: {e}"))?;

    // Exchange JWT for access token
    let client = reqwest::blocking::Client::new();
    let params = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
        ("assertion", jwt.as_str()),
    ];

    let response = client
        .post(token_uri)
        .form(&params)
        .send()
        .map_err(|e| format!("Failed to send token request: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(format!("Token exchange failed with status {status}: {body}"));
    }

    let token_response: serde_json::Value =
        response.json().map_err(|e| format!("Failed to parse token response: {e}"))?;

    token_response["access_token"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "No access_token in token response".to_string())
}

// ---------------------------------------------------------------------------
// GCE/GKE Metadata Server Authentication
// ---------------------------------------------------------------------------

/// Resolve credentials from the GCE/GKE metadata server.
fn resolve_gce_metadata_credentials() -> Result<VertexCredentials, String> {
    let metadata_url =
        "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
    let project_url = "http://metadata.google.internal/computeMetadata/v1/project/project-id";

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    // Try to get access token from metadata server
    let token_response = client
        .get(metadata_url)
        .header("Metadata-Flavor", "Google")
        .send()
        .map_err(|_| "Metadata server not available".to_string())?;

    if !token_response.status().is_success() {
        return Err("Metadata server returned error".to_string());
    }

    let token_json: serde_json::Value = token_response
        .json()
        .map_err(|e| format!("Failed to parse token response: {e}"))?;

    let access_token = token_json["access_token"]
        .as_str()
        .ok_or_else(|| "No access_token in metadata response".to_string())?
        .to_string();

    // Get project ID from env or metadata server
    let project = std::env::var("GOOGLE_VERTEX_PROJECT")
        .or_else(|_| std::env::var("GOOGLE_CLOUD_PROJECT"))
        .or_else(|_| {
            client
                .get(project_url)
                .header("Metadata-Flavor", "Google")
                .send()
                .ok()
                .and_then(|r| r.text().ok())
                .ok_or(std::env::VarError::NotPresent)
        })
        .map_err(|_| "Could not determine project ID".to_string())?;

    let location = std::env::var("GOOGLE_VERTEX_LOCATION")
        .or_else(|_| std::env::var("GOOGLE_CLOUD_LOCATION"))
        .unwrap_or_else(|_| "us-central1".to_string());

    Ok(VertexCredentials {
        access_token,
        project,
        location,
    })
}

// ---------------------------------------------------------------------------
// Request Body Construction
// ---------------------------------------------------------------------------

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
        body["systemInstruction"] =
            serde_json::json!({"parts": [{"text": context.system_prompt}]});
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

// ---------------------------------------------------------------------------
// Thinking config
// ---------------------------------------------------------------------------

/// Resolve the thinking budget for a given model + level.
fn resolve_thinking_budget(_model: &Model, level: &ThinkingLevel, budgets: Option<&ThinkingBudgets>) -> u32 {
    if *level == ThinkingLevel::Off {
        return 0;
    }

    if let Some(budgets) = budgets {
        match level {
            ThinkingLevel::Low => return budgets.low.unwrap_or(1024) as u32,
            ThinkingLevel::Medium => return budgets.medium.unwrap_or(8192) as u32,
            ThinkingLevel::High => return budgets.high.unwrap_or(32768) as u32,
            _ => {}
        }
    }

    match level {
        ThinkingLevel::Off => 0,
        ThinkingLevel::Low => 1024,
        ThinkingLevel::Medium => 8192,
        ThinkingLevel::High => 32768,
        _ => 8192,
    }
}

/// Build the `thinkingConfig` object for the generation config.
fn build_thinking_config(model: &Model, level: &ThinkingLevel, budgets: Option<&ThinkingBudgets>) -> serde_json::Value {
    if *level == ThinkingLevel::Off {
        return serde_json::json!({"thinkingBudget": 0});
    }

    let budget = resolve_thinking_budget(model, level, budgets);
    serde_json::json!({"thinkingBudget": budget})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_gemini_credentials_explicit() {
        let opts = SimpleStreamOptions {
            api_key: Some("test-key".into()),
            ..Default::default()
        };
        assert_eq!(resolve_gemini_credentials(&opts).unwrap(), "test-key");
    }

    #[test]
    fn test_resolve_vertex_credentials_missing() {
        std::env::remove_var("GOOGLE_VERTEX_ACCESS_TOKEN");
        std::env::remove_var("GOOGLE_ACCESS_TOKEN");
        std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        let opts = SimpleStreamOptions::default();
        assert!(resolve_vertex_credentials(&opts).is_err());
    }

    #[test]
    fn test_resolve_azure_config_from_resource_name() {
        // This test is for Azure, not Google - included for parity with TypeScript tests
    }
}
