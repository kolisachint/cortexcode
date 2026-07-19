//! GitHub Copilot OAuth flow (device authorization grant).
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `utils/oauth/github-copilot.ts`. Unlike Anthropic's flow, GitHub's device
//! code grant needs no local callback server or browser automation — the
//! whole login (start device flow, poll for token, exchange for a Copilot
//! token) is a plain HTTP request/poll loop, so it's fully ported here.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::types::OAuthCredentials;

const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const USER_AGENT: &str = "GitHubCopilotChat/0.35.0";

const INITIAL_POLL_INTERVAL_MULTIPLIER: f64 = 1.2;
const SLOW_DOWN_POLL_INTERVAL_MULTIPLIER: f64 = 1.4;

/// Normalize a user-supplied GitHub Enterprise URL/domain to a bare hostname.
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
}

struct Urls {
    device_code_url: String,
    access_token_url: String,
    copilot_token_url: String,
}

fn get_urls(domain: &str) -> Urls {
    Urls {
        device_code_url: format!("https://{domain}/login/device/code"),
        access_token_url: format!("https://{domain}/login/oauth/access_token"),
        copilot_token_url: format!("https://api.{domain}/copilot_internal/v2/token"),
    }
}

/// Extract the Copilot proxy endpoint from a token (`...;proxy-ep=proxy.foo.com;...`)
/// and convert it into the corresponding API base URL.
fn base_url_from_token(token: &str) -> Option<String> {
    let marker = "proxy-ep=";
    let start = token.find(marker)? + marker.len();
    let rest = &token[start..];
    let end = rest.find(';').unwrap_or(rest.len());
    let proxy_host = &rest[..end];
    let api_host = proxy_host.strip_prefix("proxy.").unwrap_or(proxy_host);
    Some(format!("https://api.{api_host}"))
}

/// Resolve the GitHub Copilot API base URL, preferring the proxy endpoint
/// embedded in the token and falling back to enterprise/default hosts.
pub fn get_base_url(token: Option<&str>, enterprise_domain: Option<&str>) -> String {
    if let Some(token) = token {
        if let Some(url) = base_url_from_token(token) {
            return url;
        }
    }
    if let Some(domain) = enterprise_domain {
        return format!("https://copilot-api.{domain}");
    }
    "https://api.individual.githubcopilot.com".to_string()
}

#[derive(serde::Deserialize)]
pub struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: f64,
    expires_in: f64,
}

/// Info the caller should show the user (verification URL + one-time code).
pub struct DeviceAuth {
    pub verification_uri: String,
    pub user_code: String,
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))
}

fn start_device_flow(domain: &str) -> Result<DeviceCodeResponse, String> {
    let urls = get_urls(domain);
    let client = http_client()?;
    let response = client
        .post(&urls.device_code_url)
        .header("accept", "application/json")
        .header("user-agent", USER_AGENT)
        .form(&[("client_id", CLIENT_ID), ("scope", "read:user")])
        .send()
        .map_err(|e| format!("device code request failed: {e}"))?;

    let status = response.status();
    let text = response.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{status}: {text}"));
    }
    serde_json::from_str(&text)
        .map_err(|e| format!("invalid device code response: {e}; body={text}"))
}

#[derive(serde::Deserialize)]
struct DeviceTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    interval: Option<f64>,
}

/// Poll GitHub's device-flow token endpoint until the user completes login.
fn poll_for_access_token(
    domain: &str,
    device_code: &str,
    interval_seconds: f64,
    expires_in: f64,
) -> Result<String, String> {
    let urls = get_urls(domain);
    let client = http_client()?;
    let deadline = Instant::now() + Duration::from_secs_f64(expires_in);
    let mut interval_ms = (interval_seconds * 1000.0).max(1000.0);
    let mut interval_multiplier = INITIAL_POLL_INTERVAL_MULTIPLIER;
    let mut slow_down_responses = 0u32;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait_ms = (interval_ms * interval_multiplier).min(remaining.as_millis() as f64);
        std::thread::sleep(Duration::from_millis(wait_ms.max(0.0) as u64));

        let response = client
            .post(&urls.access_token_url)
            .header("accept", "application/json")
            .header("user-agent", USER_AGENT)
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .map_err(|e| format!("device token request failed: {e}"))?;

        let text = response.text().unwrap_or_default();
        let parsed: DeviceTokenResponse =
            serde_json::from_str(&text).unwrap_or(DeviceTokenResponse {
                access_token: None,
                error: None,
                error_description: None,
                interval: None,
            });

        if let Some(token) = parsed.access_token {
            return Ok(token);
        }

        if let Some(error) = parsed.error {
            match error.as_str() {
                "authorization_pending" => continue,
                "slow_down" => {
                    slow_down_responses += 1;
                    interval_ms = parsed
                        .interval
                        .map(|s| s * 1000.0)
                        .unwrap_or(interval_ms + 5000.0)
                        .max(1000.0);
                    interval_multiplier = SLOW_DOWN_POLL_INTERVAL_MULTIPLIER;
                    continue;
                }
                other => {
                    let suffix = parsed
                        .error_description
                        .map(|d| format!(": {d}"))
                        .unwrap_or_default();
                    return Err(format!("Device flow failed: {other}{suffix}"));
                }
            }
        }
    }

    if slow_down_responses > 0 {
        Err("Device flow timed out after one or more slow_down responses. This is often caused by clock drift \
             in WSL or VM environments. Please sync or restart the VM clock and try again."
            .to_string())
    } else {
        Err("Device flow timed out".to_string())
    }
}

/// Start the device-flow login, returning the info to display to the user
/// plus a completion function to call once they've entered the code.
///
/// Mirrors the TS `loginGitHubCopilot` two-phase shape without forcing a
/// blocking prompt callback into this crate's API.
pub fn start_login(domain: &str) -> Result<(DeviceAuth, DeviceCodeResponse), String> {
    let device = start_device_flow(domain)?;
    let auth = DeviceAuth {
        verification_uri: device.verification_uri.clone(),
        user_code: device.user_code.clone(),
    };
    Ok((auth, device))
}

/// Complete a device-flow login started with [`start_login`]: blocks polling
/// until the user finishes authorizing in their browser, then exchanges the
/// resulting GitHub access token for a Copilot token.
pub fn complete_login(
    domain: &str,
    device: &DeviceCodeResponse,
    enterprise_domain: Option<&str>,
) -> Result<OAuthCredentials, String> {
    let github_token = poll_for_access_token(
        domain,
        &device.device_code,
        device.interval,
        device.expires_in,
    )?;
    refresh_token(&github_token, enterprise_domain)
}

/// Exchange a GitHub access token (or a previously-issued one) for a fresh
/// Copilot token. GitHub Copilot tokens are short-lived; this is both the
/// initial exchange and the subsequent refresh path.
pub fn refresh_token(
    github_token: &str,
    enterprise_domain: Option<&str>,
) -> Result<OAuthCredentials, String> {
    let domain = enterprise_domain.unwrap_or("github.com");
    let urls = get_urls(domain);
    let client = http_client()?;

    let response = client
        .get(&urls.copilot_token_url)
        .header("accept", "application/json")
        .header("authorization", format!("Bearer {github_token}"))
        .header("user-agent", USER_AGENT)
        .header("editor-version", "vscode/1.107.0")
        .header("editor-plugin-version", "copilot-chat/0.35.0")
        .header("copilot-integration-id", "vscode-chat")
        .send()
        .map_err(|e| format!("copilot token request failed: {e}"))?;

    let status = response.status();
    let text = response.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{status}: {text}"));
    }

    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid Copilot token response: {e}; body={text}"))?;
    let token = value["token"]
        .as_str()
        .ok_or("Invalid Copilot token response fields")?;
    let expires_at = value["expires_at"]
        .as_i64()
        .ok_or("Invalid Copilot token response fields")?;

    let mut extra = HashMap::new();
    if let Some(domain) = enterprise_domain {
        extra.insert("enterprise_url".to_string(), serde_json::json!(domain));
    }

    Ok(OAuthCredentials {
        refresh: github_token.to_string(),
        access: token.to_string(),
        expires: expires_at * 1000 - 5 * 60 * 1000,
        extra,
    })
}

/// Convert credentials into the bearer token used as the provider API key.
pub fn get_api_key(credentials: &OAuthCredentials) -> &str {
    &credentials.access
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_domain_bare_host() {
        assert_eq!(
            normalize_domain("company.ghe.com"),
            Some("company.ghe.com".to_string())
        );
    }

    #[test]
    fn test_normalize_domain_full_url() {
        assert_eq!(
            normalize_domain("https://company.ghe.com/"),
            Some("company.ghe.com".to_string())
        );
    }

    #[test]
    fn test_normalize_domain_empty() {
        assert_eq!(normalize_domain("   "), None);
    }

    #[test]
    fn test_base_url_from_token() {
        let token = "tid=abc;exp=123;proxy-ep=proxy.individual.githubcopilot.com;foo=bar";
        assert_eq!(
            base_url_from_token(token),
            Some("https://api.individual.githubcopilot.com".to_string())
        );
    }

    #[test]
    fn test_base_url_from_token_missing() {
        assert_eq!(base_url_from_token("tid=abc;exp=123"), None);
    }

    #[test]
    fn test_get_base_url_prefers_token() {
        let token = "proxy-ep=proxy.foo.githubcopilot.com;";
        assert_eq!(
            get_base_url(Some(token), None),
            "https://api.foo.githubcopilot.com"
        );
    }

    #[test]
    fn test_get_base_url_falls_back_to_enterprise() {
        assert_eq!(
            get_base_url(None, Some("company.ghe.com")),
            "https://copilot-api.company.ghe.com"
        );
    }

    #[test]
    fn test_get_base_url_default() {
        assert_eq!(
            get_base_url(None, None),
            "https://api.individual.githubcopilot.com"
        );
    }

    #[test]
    fn test_get_urls_shape() {
        let urls = get_urls("github.com");
        assert_eq!(urls.device_code_url, "https://github.com/login/device/code");
        assert_eq!(
            urls.access_token_url,
            "https://github.com/login/oauth/access_token"
        );
        assert_eq!(
            urls.copilot_token_url,
            "https://api.github.com/copilot_internal/v2/token"
        );
    }

    #[test]
    fn test_get_urls_enterprise_domain() {
        let urls = get_urls("company.ghe.com");
        assert_eq!(
            urls.device_code_url,
            "https://company.ghe.com/login/device/code"
        );
        assert_eq!(
            urls.copilot_token_url,
            "https://api.company.ghe.com/copilot_internal/v2/token"
        );
    }
}
