//! Anthropic OAuth flow (Claude Pro/Max).
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `utils/oauth/anthropic.ts`.
//!
//! The interactive portions of the TS flow — spinning up a local HTTP
//! callback server and opening a browser to the authorize URL — are not
//! ported here; those belong to an interactive CLI/TUI layer, not this
//! provider-logic crate. What *is* ported (and is what actually talks to
//! Anthropic's OAuth endpoints) is: building the authorize URL, parsing
//! whatever the user pastes back (a bare code, a `code#state` pair, or the
//! full redirect URL), exchanging the code for tokens, and refreshing an
//! access token — everything the caller of this crate needs to drive its
//! own callback server / prompt loop.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::pkce::Pkce;
use crate::types::OAuthCredentials;

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const DEFAULT_CALLBACK_PORT: u16 = 53692;
const SCOPES: &str =
    "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// Build the browser-facing authorize URL for the Anthropic OAuth flow.
///
/// `redirect_uri` should point at the caller's own local callback server
/// (conventionally `http://localhost:53692/callback`).
pub fn build_authorize_url(pkce: &Pkce, redirect_uri: &str) -> String {
    let params = [
        ("code", "true"),
        ("client_id", CLIENT_ID),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri),
        ("scope", SCOPES),
        ("code_challenge", &pkce.challenge),
        ("code_challenge_method", "S256"),
        ("state", &pkce.verifier),
    ];
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params)
        .finish();
    format!("{AUTHORIZE_URL}?{query}")
}

/// The default local redirect URI callers should listen on.
pub fn default_redirect_uri() -> String {
    format!("http://localhost:{DEFAULT_CALLBACK_PORT}/callback")
}

/// Parse whatever the user pastes back: a full redirect URL, a `code#state`
/// pair, a `code=...&state=...` query fragment, or a bare authorization code.
pub fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }

    if let Ok(parsed) = url::Url::parse(value) {
        let mut code = None;
        let mut state = None;
        for (k, v) in parsed.query_pairs() {
            if k == "code" {
                code = Some(v.into_owned());
            } else if k == "state" {
                state = Some(v.into_owned());
            }
        }
        if code.is_some() || state.is_some() {
            return (code, state);
        }
    }

    if let Some((code, state)) = value.split_once('#') {
        return (Some(code.to_string()), Some(state.to_string()));
    }

    if value.contains("code=") {
        let mut code = None;
        let mut state = None;
        for pair in value.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                match k {
                    "code" => code = Some(v.to_string()),
                    "state" => state = Some(v.to_string()),
                    _ => {}
                }
            }
        }
        return (code, state);
    }

    (Some(value.to_string()), None)
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn credentials_from_token_response(data: TokenResponse) -> OAuthCredentials {
    OAuthCredentials {
        refresh: data.refresh_token,
        access: data.access_token,
        expires: now_millis() + data.expires_in * 1000 - 5 * 60 * 1000,
        extra: HashMap::new(),
    }
}

fn post_token_request(body: &HashMap<&str, &str>) -> Result<TokenResponse, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let response = client
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(body)
        .send()
        .map_err(|e| format!("token request failed. url={TOKEN_URL}; details={e}"))?;

    let status = response.status();
    let text = response.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "HTTP request failed. status={status}; url={TOKEN_URL}; body={text}"
        ));
    }

    serde_json::from_str(&text)
        .map_err(|e| format!("token response was invalid JSON. body={text}; details={e}"))
}

/// Exchange an authorization code (from the redirect / manual paste) for tokens.
pub fn exchange_authorization_code(
    code: &str,
    state: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthCredentials, String> {
    let mut body = HashMap::new();
    body.insert("grant_type", "authorization_code");
    body.insert("client_id", CLIENT_ID);
    body.insert("code", code);
    body.insert("state", state);
    body.insert("redirect_uri", redirect_uri);
    body.insert("code_verifier", verifier);

    post_token_request(&body).map(credentials_from_token_response)
}

/// Refresh an Anthropic OAuth access token.
pub fn refresh_token(refresh_token: &str) -> Result<OAuthCredentials, String> {
    let mut body = HashMap::new();
    body.insert("grant_type", "refresh_token");
    body.insert("client_id", CLIENT_ID);
    body.insert("refresh_token", refresh_token);

    post_token_request(&body).map(credentials_from_token_response)
}

/// Convert credentials into the bearer token used as the provider API key.
pub fn get_api_key(credentials: &OAuthCredentials) -> &str {
    &credentials.access
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkce::generate_pkce;

    #[test]
    fn test_build_authorize_url_contains_expected_params() {
        let pkce = generate_pkce();
        let url = build_authorize_url(&pkce, &default_redirect_uri());
        assert!(url.starts_with(AUTHORIZE_URL));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("state={}", pkce.verifier)));
    }

    #[test]
    fn test_parse_authorization_input_full_url() {
        let (code, state) =
            parse_authorization_input("http://localhost:53692/callback?code=abc123&state=xyz789");
        assert_eq!(code, Some("abc123".to_string()));
        assert_eq!(state, Some("xyz789".to_string()));
    }

    #[test]
    fn test_parse_authorization_input_hash_pair() {
        let (code, state) = parse_authorization_input("abc123#xyz789");
        assert_eq!(code, Some("abc123".to_string()));
        assert_eq!(state, Some("xyz789".to_string()));
    }

    #[test]
    fn test_parse_authorization_input_query_fragment() {
        let (code, state) = parse_authorization_input("code=abc123&state=xyz789");
        assert_eq!(code, Some("abc123".to_string()));
        assert_eq!(state, Some("xyz789".to_string()));
    }

    #[test]
    fn test_parse_authorization_input_bare_code() {
        let (code, state) = parse_authorization_input("just-a-code");
        assert_eq!(code, Some("just-a-code".to_string()));
        assert_eq!(state, None);
    }

    #[test]
    fn test_parse_authorization_input_empty() {
        let (code, state) = parse_authorization_input("   ");
        assert_eq!(code, None);
        assert_eq!(state, None);
    }

    #[test]
    fn test_get_api_key() {
        let creds = OAuthCredentials {
            refresh: "r".into(),
            access: "a".into(),
            expires: 0,
            extra: HashMap::new(),
        };
        assert_eq!(get_api_key(&creds), "a");
    }

    #[test]
    fn test_credentials_from_token_response() {
        let creds = credentials_from_token_response(TokenResponse {
            access_token: "acc".into(),
            refresh_token: "ref".into(),
            expires_in: 3600,
        });
        assert_eq!(creds.access, "acc");
        assert_eq!(creds.refresh, "ref");
        // expires should be ~55 minutes from now (3600s - 5min buffer), not exactly 3600s.
        let now = now_millis();
        assert!(creds.expires > now + 54 * 60 * 1000);
        assert!(creds.expires < now + 56 * 60 * 1000);
    }
}
