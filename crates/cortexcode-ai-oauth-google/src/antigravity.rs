//! `utils/oauth/google-antigravity.ts`: the Antigravity client, which
//! reaches the consumer free tier. Project discovery is best effort: every
//! step falls through, ending at Google's managed consumer project.

use super::*;

/// The Antigravity flow.
pub const ANTIGRAVITY: GoogleFlow = GoogleFlow {
    client_env: GoogleOAuthClientEnv {
        id_var: "CORTEXCODE_ANTIGRAVITY_CLIENT_ID",
        secret_var: "CORTEXCODE_ANTIGRAVITY_CLIENT_SECRET",
        product_name: "Google Antigravity",
    },
    callback_port: 51121,
    callback_path: "/oauth-callback",
    redirect_uri: "http://localhost:51121/oauth-callback",
    scopes: &[
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
        "https://www.googleapis.com/auth/cclog",
        "https://www.googleapis.com/auth/experimentsandconfigs",
    ],
    refresh_label: "Antigravity",
};

/// Google's managed project for consumer accounts.
const MANAGED_CONSUMER_PROJECT: &str = "aicode-consumers";
const POLL_INTERVAL_MS: u64 = 5000;
const MAX_POLLS: u32 = 12;

fn metadata() -> Value {
    json!({
        "ideType": "ANTIGRAVITY",
        "platform": "PLATFORM_UNSPECIFIED",
        "pluginType": "GEMINI",
    })
}

/// `readProjectId`: a non-empty string or `{id}`.
fn read_project_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .or_else(|| value["id"].as_str())
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

/// `loadCodeAssist`: an existing project, else the tier to onboard.
async fn load(
    fetch: &dyn Fetch,
    headers: &[(&str, String)],
) -> Result<(Option<String>, Option<String>), String> {
    let request = code_assist_request(
        "POST",
        "/v1internal:loadCodeAssist",
        headers,
        Some(json!({"metadata": metadata()})),
    );
    let response = fetch.fetch(request).await?;
    if !response.ok() {
        return Ok((None, None));
    }
    let data = parse_json(&response.body)?;
    if let Some(existing) = read_project_id(&data["cloudaicompanionProject"]) {
        return Ok((Some(existing), None));
    }
    let tiers = data["allowedTiers"].as_array();
    let tier_id = data["currentTier"]["id"]
        .as_str()
        .or_else(|| {
            tiers.and_then(|t| {
                t.iter()
                    .find(|tier| tier["isDefault"] == true)
                    .and_then(|tier| tier["id"].as_str())
            })
        })
        .or_else(|| {
            tiers
                .and_then(|t| t.first())
                .and_then(|tier| tier["id"].as_str())
        })
        .map(str::to_string);
    Ok((None, tier_id))
}

/// `onboardUser` and its operation polls: the provisioned project.
async fn onboard(
    fetch: &dyn Fetch,
    headers: &[(&str, String)],
    tier_id: &str,
    env_project: Option<&str>,
    callbacks: &dyn OAuthLoginCallbacks,
    poll_ms: u64,
) -> Result<Option<String>, String> {
    callbacks.on_progress("Provisioning the Antigravity project (this may take a moment)...");
    let mut body = json!({"tierId": tier_id, "metadata": metadata()});
    if let Some(project) = env_project {
        body["cloudaicompanionProject"] = json!(project);
    }
    let request = code_assist_request("POST", "/v1internal:onboardUser", headers, Some(body));
    let response = fetch.fetch(request).await?;
    if !response.ok() {
        return Ok(None);
    }
    let mut operation = parse_json(&response.body)?;
    let mut attempt = 0;
    while operation["done"] != true && attempt < MAX_POLLS {
        let Some(name) = operation["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .map(str::to_string)
        else {
            break;
        };
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
        callbacks.on_progress(&format!(
            "Waiting for project provisioning (attempt {})...",
            attempt + 2
        ));
        let request = code_assist_request("GET", &format!("/v1internal/{name}"), headers, None);
        let poll = fetch.fetch(request).await?;
        if !poll.ok() {
            break;
        }
        operation = parse_json(&poll.body)?;
        attempt += 1;
    }
    Ok(read_project_id(
        &operation["response"]["cloudaicompanionProject"],
    ))
}

/// `discoverProject`: never fails.
pub(crate) async fn discover_project(
    fetch: &dyn Fetch,
    access_token: &str,
    callbacks: &dyn OAuthLoginCallbacks,
    env_project: Option<String>,
    poll_ms: u64,
) -> String {
    let headers = [
        ("Authorization", format!("Bearer {access_token}")),
        ("Content-Type", "application/json".to_string()),
        ("User-Agent", "google-api-nodejs-client/9.15.1".to_string()),
        (
            "X-Goog-Api-Client",
            "google-cloud-sdk vscode_cloudshelleditor/0.1".to_string(),
        ),
        ("Client-Metadata", metadata().to_string()),
    ];

    callbacks.on_progress("Checking for an existing project...");
    let tier_id = match load(fetch, &headers).await {
        Ok((Some(existing), _)) => return existing,
        Ok((None, tier_id)) => tier_id,
        Err(_) => None,
    };
    if let Some(tier_id) = tier_id.filter(|t| !t.is_empty()) {
        let env = env_project.as_deref();
        if let Ok(Some(project)) = onboard(fetch, &headers, &tier_id, env, callbacks, poll_ms).await
        {
            return project;
        }
    }
    if let Some(project) = env_project {
        return project;
    }
    callbacks.on_progress("Using Google's managed Antigravity project...");
    MANAGED_CONSUMER_PROJECT.to_string()
}

/// `refreshAntigravityToken`.
pub async fn refresh_antigravity_token(
    fetch: &dyn Fetch,
    refresh: &str,
    project_id: &str,
) -> Result<OAuthCredentials, String> {
    refresh_token(&ANTIGRAVITY, fetch, refresh, project_id).await
}

/// `loginAntigravity`.
pub async fn login_antigravity(
    fetch: &dyn Fetch,
    callbacks: &dyn OAuthLoginCallbacks,
) -> Result<OAuthCredentials, String> {
    login(&ANTIGRAVITY, fetch, callbacks, |fetch, token, callbacks| {
        Box::pin(async move {
            Ok(
                discover_project(fetch, &token, callbacks, env_project_id(), POLL_INTERVAL_MS)
                    .await,
            )
        })
    })
    .await
}

google_provider!(
    AntigravityOAuthProvider,
    "google-antigravity",
    "Google Antigravity (Gemini, Claude, GPT-OSS)",
    login_antigravity,
    refresh_antigravity_token,
    "Antigravity credentials missing projectId"
);
