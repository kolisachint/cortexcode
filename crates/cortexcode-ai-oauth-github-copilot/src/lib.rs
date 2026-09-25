//! GitHub Copilot OAuth: port of hoocode `utils/oauth/github-copilot.ts`
//! (v0.5.89). Device-code login against github.com (or a GitHub Enterprise
//! domain), then a Copilot token from `copilot_internal/v2/token`.

use cortexcode_ai_oauth::{
    form_body, BoxFuture, Fetch, HttpRequest, OAuthAuthInfo, OAuthCredentials, OAuthLoginCallbacks,
    OAuthPrompt, OAuthProvider, ReqwestFetch,
};
use cortexcode_ai_types::{AbortSignal, Model};
use serde_json::Value;
use tokio::time::{Duration, Instant};

const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const USER_AGENT: &str = "GitHubCopilotChat/0.35.0";

/// `COPILOT_HEADERS`.
pub const COPILOT_HEADERS: [(&str, &str); 4] = [
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];

const INITIAL_POLL_INTERVAL_MULTIPLIER: f64 = 1.2;
const SLOW_DOWN_POLL_INTERVAL_MULTIPLIER: f64 = 1.4;

/// The `enterpriseUrl` credentials field.
const ENTERPRISE_URL: &str = "enterpriseUrl";

/// `normalizeDomain`: the hostname of a URL or bare domain.
pub fn normalize_domain(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    url::Url::parse(&candidate)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .filter(|h| !h.is_empty())
}

struct Urls {
    device_code: String,
    access_token: String,
    copilot_token: String,
}

fn urls(domain: &str) -> Urls {
    Urls {
        device_code: format!("https://{domain}/login/device/code"),
        access_token: format!("https://{domain}/login/oauth/access_token"),
        copilot_token: format!("https://api.{domain}/copilot_internal/v2/token"),
    }
}

/// `getBaseUrlFromToken`: `proxy-ep=proxy.X` -> `https://api.X`.
fn base_url_from_token(token: &str) -> Option<String> {
    let start = token.find("proxy-ep=")? + "proxy-ep=".len();
    let host = token[start..].split(';').next().filter(|h| !h.is_empty())?;
    let host = host
        .strip_prefix("proxy.")
        .map_or_else(|| host.to_string(), |rest| format!("api.{rest}"));
    Some(format!("https://{host}"))
}

/// `getGitHubCopilotBaseUrl`.
pub fn get_github_copilot_base_url(token: Option<&str>, enterprise_domain: Option<&str>) -> String {
    if let Some(url) = token.and_then(base_url_from_token) {
        return url;
    }
    match enterprise_domain {
        Some(domain) => format!("https://copilot-api.{domain}"),
        None => "https://api.individual.githubcopilot.com".to_string(),
    }
}

/// `fetchJson`: a non-2xx status is `"<status> <statusText>: <body>"`.
async fn fetch_json(fetch: &dyn Fetch, request: HttpRequest) -> Result<Value, String> {
    let response = fetch.fetch(request).await?;
    if !response.ok() {
        return Err(format!(
            "{} {}: {}",
            response.status, response.status_text, response.body
        ));
    }
    serde_json::from_str(&response.body).map_err(|e| e.to_string())
}

fn form_post(url: &str, pairs: &[(&str, &str)]) -> HttpRequest {
    HttpRequest::new("POST", url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", USER_AGENT)
        .body(form_body(pairs))
}

/// `DeviceCodeResponse`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: f64,
    pub expires_in: f64,
}

/// `startDeviceFlow`.
pub async fn start_device_flow(fetch: &dyn Fetch, domain: &str) -> Result<DeviceCode, String> {
    let data = fetch_json(
        fetch,
        form_post(
            &urls(domain).device_code,
            &[("client_id", CLIENT_ID), ("scope", "read:user")],
        ),
    )
    .await?;
    if !data.is_object() {
        return Err("Invalid device code response".to_string());
    }
    let text = |key: &str| data[key].as_str().map(str::to_string);
    let number = |key: &str| data[key].as_f64();
    match (
        text("device_code"),
        text("user_code"),
        text("verification_uri"),
        number("interval"),
        number("expires_in"),
    ) {
        (
            Some(device_code),
            Some(user_code),
            Some(verification_uri),
            Some(interval),
            Some(expires_in),
        ) => Ok(DeviceCode {
            device_code,
            user_code,
            verification_uri,
            interval,
            expires_in,
        }),
        _ => Err("Invalid device code response fields".to_string()),
    }
}

/// `abortableSleep`.
async fn abortable_sleep(ms: u64, signal: Option<&AbortSignal>) -> Result<(), String> {
    match signal {
        Some(signal) if signal.aborted() => Err("Login cancelled".to_string()),
        Some(signal) => tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()),
            _ = signal.cancelled() => Err("Login cancelled".to_string()),
        },
        None => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        }
    }
}

/// `pollForGitHubAccessToken`: wait (interval x 1.2, x 1.4 after
/// `slow_down`) before each poll, never past the device code's lifetime.
pub async fn poll_for_github_access_token(
    fetch: &dyn Fetch,
    domain: &str,
    device_code: &str,
    interval_seconds: f64,
    expires_in: f64,
    signal: Option<&AbortSignal>,
) -> Result<String, String> {
    let access_token_url = urls(domain).access_token;
    let deadline = Instant::now() + Duration::from_millis((expires_in * 1000.0) as u64);
    let mut interval_ms = ((interval_seconds * 1000.0).floor()).max(1000.0);
    let mut multiplier = INITIAL_POLL_INTERVAL_MULTIPLIER;
    let mut slow_down_responses = 0;

    while Instant::now() < deadline {
        if signal.is_some_and(AbortSignal::aborted) {
            return Err("Login cancelled".to_string());
        }
        let remaining_ms = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as f64;
        let wait_ms = (interval_ms * multiplier).ceil().min(remaining_ms);
        abortable_sleep(wait_ms as u64, signal).await?;

        let raw = fetch_json(
            fetch,
            form_post(
                &access_token_url,
                &[
                    ("client_id", CLIENT_ID),
                    ("device_code", device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ],
            ),
        )
        .await?;

        if let Some(token) = raw["access_token"].as_str() {
            return Ok(token.to_string());
        }
        if let Some(error) = raw["error"].as_str() {
            match error {
                "authorization_pending" => continue,
                "slow_down" => {
                    slow_down_responses += 1;
                    interval_ms = match raw["interval"].as_f64() {
                        Some(interval) if interval > 0.0 => interval * 1000.0,
                        _ => (interval_ms + 5000.0).max(1000.0),
                    };
                    multiplier = SLOW_DOWN_POLL_INTERVAL_MULTIPLIER;
                    continue;
                }
                _ => {
                    let suffix = raw["error_description"]
                        .as_str()
                        .filter(|d| !d.is_empty())
                        .map(|d| format!(": {d}"))
                        .unwrap_or_default();
                    return Err(format!("Device flow failed: {error}{suffix}"));
                }
            }
        }
    }

    if slow_down_responses > 0 {
        return Err("Device flow timed out after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments. Please sync or restart the VM clock and try again.".to_string());
    }
    Err("Device flow timed out".to_string())
}

/// `refreshGitHubCopilotToken`: a Copilot token for the GitHub token.
pub async fn refresh_github_copilot_token(
    fetch: &dyn Fetch,
    refresh_token: &str,
    enterprise_domain: Option<&str>,
) -> Result<OAuthCredentials, String> {
    let domain = enterprise_domain
        .filter(|d| !d.is_empty())
        .unwrap_or("github.com");
    let mut request = HttpRequest::new("GET", urls(domain).copilot_token)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {refresh_token}"));
    for (k, v) in COPILOT_HEADERS {
        request = request.header(k, v);
    }
    let raw = fetch_json(fetch, request).await?;
    if !raw.is_object() {
        return Err("Invalid Copilot token response".to_string());
    }
    let (Some(token), Some(expires_at)) = (raw["token"].as_str(), raw["expires_at"].as_f64())
    else {
        return Err("Invalid Copilot token response fields".to_string());
    };
    let mut credentials = OAuthCredentials::new(
        refresh_token,
        token,
        (expires_at * 1000.0) as i64 - 5 * 60 * 1000,
    );
    if let Some(domain) = enterprise_domain {
        credentials
            .extra
            .insert(ENTERPRISE_URL.into(), domain.into());
    }
    Ok(credentials)
}

/// `enableGitHubCopilotModel`: accept a model's policy; `false` on failure.
async fn enable_model(
    fetch: &dyn Fetch,
    token: &str,
    model_id: &str,
    enterprise: Option<&str>,
) -> bool {
    let base = get_github_copilot_base_url(Some(token), enterprise);
    let mut request = HttpRequest::new("POST", format!("{base}/models/{model_id}/policy"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"));
    for (k, v) in COPILOT_HEADERS {
        request = request.header(k, v);
    }
    let request = request
        .header("openai-intent", "chat-policy")
        .header("x-interaction-type", "chat-policy")
        .body(r#"{"state":"enabled"}"#);
    fetch.fetch(request).await.is_ok_and(|r| r.ok())
}

/// `enableAllGitHubCopilotModels`, concurrently.
async fn enable_all_models(fetch: &dyn Fetch, token: &str, enterprise: Option<&str>) {
    let models = cortexcode_ai_models::get_models("github-copilot");
    futures_util::future::join_all(
        models
            .iter()
            .map(|model| enable_model(fetch, token, &model.id, enterprise)),
    )
    .await;
}

/// `loginGitHubCopilot`.
pub async fn login_github_copilot(
    fetch: &dyn Fetch,
    callbacks: &dyn OAuthLoginCallbacks,
) -> Result<OAuthCredentials, String> {
    let input = callbacks
        .on_prompt(OAuthPrompt {
            message: "GitHub Enterprise URL/domain (blank for github.com)".to_string(),
            placeholder: Some("company.ghe.com".to_string()),
            allow_empty: true,
        })
        .await?;
    let signal = callbacks.signal();
    if signal.as_ref().is_some_and(AbortSignal::aborted) {
        return Err("Login cancelled".to_string());
    }
    let enterprise = normalize_domain(&input);
    if !input.trim().is_empty() && enterprise.is_none() {
        return Err("Invalid GitHub Enterprise URL/domain".to_string());
    }
    let domain = enterprise
        .clone()
        .unwrap_or_else(|| "github.com".to_string());

    let device = start_device_flow(fetch, &domain).await?;
    callbacks.on_auth(OAuthAuthInfo {
        url: device.verification_uri.clone(),
        instructions: Some(format!("Enter code: {}", device.user_code)),
    });
    let github_token = poll_for_github_access_token(
        fetch,
        &domain,
        &device.device_code,
        device.interval,
        device.expires_in,
        signal.as_ref(),
    )
    .await?;
    let credentials =
        refresh_github_copilot_token(fetch, &github_token, enterprise.as_deref()).await?;

    callbacks.on_progress("Enabling models...");
    enable_all_models(fetch, &credentials.access, enterprise.as_deref()).await;
    Ok(credentials)
}

/// `githubCopilotOAuthProvider`.
pub struct GitHubCopilotOAuthProvider {
    fetch: Box<dyn Fetch>,
}

impl Default for GitHubCopilotOAuthProvider {
    fn default() -> Self {
        Self::with_fetch(Box::new(ReqwestFetch))
    }
}

impl GitHubCopilotOAuthProvider {
    pub fn with_fetch(fetch: Box<dyn Fetch>) -> Self {
        Self { fetch }
    }
}

impl OAuthProvider for GitHubCopilotOAuthProvider {
    fn id(&self) -> &str {
        "github-copilot"
    }

    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    fn login<'a>(
        &'a self,
        callbacks: &'a dyn OAuthLoginCallbacks,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(login_github_copilot(self.fetch.as_ref(), callbacks))
    }

    fn refresh_token<'a>(
        &'a self,
        credentials: &'a OAuthCredentials,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(refresh_github_copilot_token(
            self.fetch.as_ref(),
            &credentials.refresh,
            credentials.extra_str(ENTERPRISE_URL),
        ))
    }

    fn get_api_key(&self, credentials: &OAuthCredentials) -> String {
        credentials.access.clone()
    }

    /// Point Copilot models at the base URL the token names.
    fn modify_models(&self, models: Vec<Model>, credentials: &OAuthCredentials) -> Vec<Model> {
        let domain = credentials
            .extra_str(ENTERPRISE_URL)
            .and_then(normalize_domain);
        let base_url = get_github_copilot_base_url(Some(&credentials.access), domain.as_deref());
        models
            .into_iter()
            .map(|mut m| {
                if m.provider == "github-copilot" {
                    m.base_url = base_url.clone();
                }
                m
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
