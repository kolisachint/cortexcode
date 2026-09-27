//! `utils/oauth/google-gemini-cli.ts`: the Gemini CLI client. Project
//! discovery is strict: an account with a tier but no managed project needs
//! `GOOGLE_CLOUD_PROJECT`.

use super::*;

/// The Gemini CLI flow.
pub const GEMINI_CLI: GoogleFlow = GoogleFlow {
    client_env: GoogleOAuthClientEnv {
        id_var: "CORTEXCODE_GEMINI_CLI_CLIENT_ID",
        secret_var: "CORTEXCODE_GEMINI_CLI_CLIENT_SECRET",
        product_name: "Google Cloud Code Assist (Gemini CLI)",
    },
    callback_port: 8085,
    callback_path: "/oauth2callback",
    redirect_uri: "http://localhost:8085/oauth2callback",
    scopes: &[
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
    ],
    refresh_label: "Google Cloud",
};

const TIER_FREE: &str = "free-tier";
const TIER_LEGACY: &str = "legacy-tier";
const TIER_STANDARD: &str = "standard-tier";
const POLL_INTERVAL_MS: u64 = 5000;
const REQUIRES_PROJECT: &str = "This account requires setting the GOOGLE_CLOUD_PROJECT or \
     GOOGLE_CLOUD_PROJECT_ID environment variable. See https://goo.gle/gemini-cli-auth-docs#workspace-gca";

fn metadata() -> Map<String, Value> {
    let mut metadata = Map::new();
    metadata.insert("ideType".into(), json!("IDE_UNSPECIFIED"));
    metadata.insert("platform".into(), json!("PLATFORM_UNSPECIFIED"));
    metadata.insert("pluginType".into(), json!("GEMINI"));
    metadata
}

/// `isVpcScAffectedUser`.
fn is_vpc_sc_affected_user(payload: &Value) -> bool {
    payload["error"]["details"]
        .as_array()
        .is_some_and(|details| {
            details
                .iter()
                .any(|d| d["reason"] == "SECURITY_POLICY_VIOLATED")
        })
}

/// `getDefaultTier`: the default allowed tier, else legacy.
fn default_tier_id(allowed_tiers: &Value) -> Option<String> {
    let tiers = allowed_tiers.as_array().filter(|t| !t.is_empty());
    match tiers.and_then(|t| t.iter().find(|tier| tier["isDefault"] == true)) {
        Some(tier) => tier["id"].as_str().map(str::to_string),
        None => Some(TIER_LEGACY.to_string()),
    }
}

/// `pollOperation`: until `done` (the first poll without waiting).
async fn poll_operation(
    fetch: &dyn Fetch,
    name: &str,
    headers: &[(&str, String)],
    callbacks: &dyn OAuthLoginCallbacks,
    poll_ms: u64,
) -> Result<Value, String> {
    let mut attempt = 0;
    loop {
        if attempt > 0 {
            callbacks.on_progress(&format!(
                "Waiting for project provisioning (attempt {})...",
                attempt + 1
            ));
            tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
        }
        let request = code_assist_request("GET", &format!("/v1internal/{name}"), headers, None);
        let response = fetch.fetch(request).await?;
        if !response.ok() {
            return Err(format!(
                "Failed to poll operation: {} {}",
                response.status, response.status_text
            ));
        }
        let data = parse_json(&response.body)?;
        if data["done"] == true {
            return Ok(data);
        }
        attempt += 1;
    }
}

/// `discoverProject`.
pub(crate) async fn discover_project(
    fetch: &dyn Fetch,
    access_token: &str,
    callbacks: &dyn OAuthLoginCallbacks,
    env_project: Option<String>,
    poll_ms: u64,
) -> Result<String, String> {
    let headers = [
        ("Authorization", format!("Bearer {access_token}")),
        ("Content-Type", "application/json".to_string()),
        ("User-Agent", "google-api-nodejs-client/9.15.1".to_string()),
        ("X-Goog-Api-Client", "gl-node/22.17.0".to_string()),
    ];

    callbacks.on_progress("Checking for existing Cloud Code Assist project...");
    let mut load_body = Map::new();
    let mut load_metadata = metadata();
    if let Some(project) = &env_project {
        load_body.insert("cloudaicompanionProject".into(), json!(project));
        load_metadata.insert("duetProject".into(), json!(project));
    }
    load_body.insert("metadata".into(), Value::Object(load_metadata));
    let request = code_assist_request(
        "POST",
        "/v1internal:loadCodeAssist",
        &headers,
        Some(Value::Object(load_body)),
    );
    let response = fetch.fetch(request).await?;
    let data = if response.ok() {
        parse_json(&response.body)?
    } else {
        let payload = serde_json::from_str::<Value>(&response.body).unwrap_or(Value::Null);
        if !is_vpc_sc_affected_user(&payload) {
            return Err(format!(
                "loadCodeAssist failed: {} {}: {}",
                response.status, response.status_text, response.body
            ));
        }
        json!({"currentTier": {"id": TIER_STANDARD}})
    };

    if data["currentTier"].is_object() {
        if let Some(project) = data["cloudaicompanionProject"]
            .as_str()
            .filter(|p| !p.is_empty())
        {
            return Ok(project.to_string());
        }
        return env_project.ok_or_else(|| REQUIRES_PROJECT.to_string());
    }

    let tier_id = default_tier_id(&data["allowedTiers"]).unwrap_or_else(|| TIER_FREE.to_string());
    if tier_id != TIER_FREE && env_project.is_none() {
        return Err(REQUIRES_PROJECT.to_string());
    }

    callbacks.on_progress("Provisioning Cloud Code Assist project (this may take a moment)...");
    // Free tier: Google provisions the project, so none is sent.
    let mut onboard_body = Map::new();
    onboard_body.insert("tierId".into(), json!(tier_id));
    let mut onboard_metadata = metadata();
    let paid_project = env_project.as_ref().filter(|_| tier_id != TIER_FREE);
    if let Some(project) = paid_project {
        onboard_metadata.insert("duetProject".into(), json!(project));
    }
    onboard_body.insert("metadata".into(), Value::Object(onboard_metadata));
    if let Some(project) = paid_project {
        onboard_body.insert("cloudaicompanionProject".into(), json!(project));
    }
    let request = code_assist_request(
        "POST",
        "/v1internal:onboardUser",
        &headers,
        Some(Value::Object(onboard_body)),
    );
    let response = fetch.fetch(request).await?;
    if !response.ok() {
        return Err(format!(
            "onboardUser failed: {} {}: {}",
            response.status, response.status_text, response.body
        ));
    }
    let mut operation = parse_json(&response.body)?;
    if operation["done"] != true {
        if let Some(name) = operation["name"].as_str().filter(|n| !n.is_empty()) {
            let name = name.to_string();
            operation = poll_operation(fetch, &name, &headers, callbacks, poll_ms).await?;
        }
    }

    if let Some(project) = operation["response"]["cloudaicompanionProject"]["id"]
        .as_str()
        .filter(|p| !p.is_empty())
    {
        return Ok(project.to_string());
    }
    env_project.ok_or_else(|| {
        "Could not discover or provision a Google Cloud project. Try setting the \
         GOOGLE_CLOUD_PROJECT or GOOGLE_CLOUD_PROJECT_ID environment variable. See \
         https://goo.gle/gemini-cli-auth-docs#workspace-gca"
            .to_string()
    })
}

/// `refreshGoogleCloudToken`.
pub async fn refresh_google_cloud_token(
    fetch: &dyn Fetch,
    refresh: &str,
    project_id: &str,
) -> Result<OAuthCredentials, String> {
    refresh_token(&GEMINI_CLI, fetch, refresh, project_id).await
}

/// `loginGeminiCli`.
pub async fn login_gemini_cli(
    fetch: &dyn Fetch,
    callbacks: &dyn OAuthLoginCallbacks,
) -> Result<OAuthCredentials, String> {
    login(&GEMINI_CLI, fetch, callbacks, |fetch, token, callbacks| {
        Box::pin(async move {
            discover_project(fetch, &token, callbacks, env_project_id(), POLL_INTERVAL_MS).await
        })
    })
    .await
}

google_provider!(
    GeminiCliOAuthProvider,
    "google-gemini-cli",
    "Google Cloud Code Assist (Gemini CLI)",
    login_gemini_cli,
    refresh_google_cloud_token,
    "Google Cloud credentials missing projectId"
);
