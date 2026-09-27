//! Port of hoocode `packages/ai/test/openai-codex-oauth.test.ts` (`fetch`
//! stubbed through the [`Fetch`] seam) plus the login flow and helpers.

use super::*;
use cortexcode_ai_oauth::HttpResponse;
use serde_json::json;
use std::sync::Mutex;

/// Records every request and answers with `respond`.
struct FakeFetch {
    requests: std::sync::Arc<Mutex<Vec<HttpRequest>>>,
    respond: Box<dyn Fn(&HttpRequest) -> HttpResponse + Send + Sync>,
}

impl FakeFetch {
    fn new(respond: impl Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static) -> Self {
        Self {
            requests: Default::default(),
            respond: Box::new(respond),
        }
    }
}

impl Fetch for FakeFetch {
    fn fetch(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>> {
        let response = (self.respond)(&request);
        self.requests.lock().unwrap().push(request);
        Box::pin(async move { Ok(response) })
    }
}

fn token_with_account(account: &str) -> String {
    let payload = base64::engine::general_purpose::STANDARD
        .encode(json!({JWT_CLAIM_PATH: {"chatgpt_account_id": account}}).to_string());
    format!("aaa.{payload}.bbb")
}

fn form(request: &HttpRequest) -> Vec<(String, String)> {
    url::form_urlencoded::parse(request.body.as_deref().unwrap().as_bytes())
        .into_owned()
        .collect()
}

#[tokio::test]
async fn does_not_write_token_refresh_failures_to_stderr() {
    // Rust never logs here; the error carries the status and body.
    let fetch = FakeFetch::new(|_| HttpResponse {
        status: 401,
        status_text: "Unauthorized".into(),
        body: json!({"error": {
            "message": "Could not validate your token. Please try signing in again.",
            "type": "invalid_request_error",
        }})
        .to_string(),
    });
    let error = refresh_openai_codex_token(&fetch, "invalid-refresh-token")
        .await
        .unwrap_err();
    assert!(
        error.starts_with("OpenAI Codex token refresh failed (401): "),
        "{error}"
    );
    assert!(error.contains("Could not validate your token"), "{error}");
}

#[tokio::test]
async fn refresh_sends_the_form_and_returns_the_account_id() {
    let access = token_with_account("acc_1");
    let access_for_reply = access.clone();
    let fetch = FakeFetch::new(move |_| HttpResponse {
        status: 200,
        status_text: "OK".into(),
        body: json!({"access_token": access_for_reply, "refresh_token": "r2", "expires_in": 3600})
            .to_string(),
    });
    let before = cortexcode_ai_oauth::now_ms();
    let creds = refresh_openai_codex_token(&fetch, "r1").await.unwrap();
    assert_eq!(creds.access, access);
    assert_eq!(creds.refresh, "r2");
    assert!(creds.expires >= before + 3_600_000);
    assert_eq!(creds.extra_str("accountId"), Some("acc_1"));
    let request = fetch.requests.lock().unwrap()[0].clone();
    assert_eq!(request.url, TOKEN_URL);
    assert_eq!(
        request.header_value("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        form(&request),
        [
            ("grant_type".to_string(), "refresh_token".to_string()),
            ("refresh_token".into(), "r1".into()),
            ("client_id".into(), CLIENT_ID.into()),
        ]
    );
}

#[tokio::test]
async fn refresh_reports_missing_fields_and_missing_account() {
    let fetch = FakeFetch::new(|_| HttpResponse {
        status: 200,
        status_text: "OK".into(),
        body: r#"{"access_token":"a"}"#.into(),
    });
    assert_eq!(
        refresh_openai_codex_token(&fetch, "r").await.unwrap_err(),
        r#"OpenAI Codex token refresh response missing fields: {"access_token":"a"}"#
    );
    let fetch = FakeFetch::new(|_| HttpResponse {
        status: 200,
        status_text: "OK".into(),
        body: r#"{"access_token":"a.b.c","refresh_token":"r","expires_in":1}"#.into(),
    });
    assert_eq!(
        refresh_openai_codex_token(&fetch, "r").await.unwrap_err(),
        "Failed to extract accountId from token"
    );
}

/// Callbacks that paste the redirect URL back as manual input.
struct ManualPaste {
    auth_url: Mutex<String>,
    state_override: Option<&'static str>,
}

impl OAuthLoginCallbacks for ManualPaste {
    fn on_auth(&self, info: OAuthAuthInfo) {
        *self.auth_url.lock().unwrap() = info.url;
    }

    fn on_prompt(&self, _prompt: OAuthPrompt) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Ok(String::new()) })
    }

    fn on_manual_code_input(&self) -> Option<BoxFuture<'_, Result<String, String>>> {
        let url = url::Url::parse(&self.auth_url.lock().unwrap()).unwrap();
        let state = self.state_override.map(str::to_string).unwrap_or_else(|| {
            url.query_pairs()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.into_owned())
                .unwrap()
        });
        let pasted = format!("{REDIRECT_URI}?code=manual-code&state={state}");
        Some(Box::pin(async move { Ok(pasted) }))
    }
}

#[tokio::test]
async fn login_with_a_pasted_redirect_url_exchanges_the_code() {
    let access = token_with_account("acc_login");
    let access_for_reply = access.clone();
    let fetch = FakeFetch::new(move |_| HttpResponse {
        status: 200,
        status_text: "OK".into(),
        body: json!({"access_token": access_for_reply, "refresh_token": "r", "expires_in": 60})
            .to_string(),
    });
    let callbacks = ManualPaste {
        auth_url: Mutex::new(String::new()),
        state_override: None,
    };
    let creds = login_openai_codex(&fetch, &callbacks, None).await.unwrap();
    assert_eq!(creds.extra_str("accountId"), Some("acc_login"));

    let auth_url = url::Url::parse(&callbacks.auth_url.lock().unwrap()).unwrap();
    let params: Vec<(String, String)> = auth_url.query_pairs().into_owned().collect();
    let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(auth_url.as_str().split('?').next(), Some(AUTHORIZE_URL));
    assert_eq!(get("redirect_uri"), Some(REDIRECT_URI));
    assert_eq!(get("originator"), Some("pi"));
    assert_eq!(get("codex_cli_simplified_flow"), Some("true"));
    assert_eq!(get("scope"), Some(SCOPE));
    assert_eq!(get("state").unwrap().len(), 32);

    let request = fetch.requests.lock().unwrap()[0].clone();
    let form = form(&request);
    let field = |k: &str| form.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(field("grant_type"), Some("authorization_code"));
    assert_eq!(field("code"), Some("manual-code"));
    assert_eq!(field("redirect_uri"), Some(REDIRECT_URI));
    assert!(field("code_verifier").is_some());
}

#[tokio::test]
async fn login_rejects_a_pasted_state_mismatch() {
    let fetch = FakeFetch::new(|_| unreachable!("no token request on a state mismatch"));
    let callbacks = ManualPaste {
        auth_url: Mutex::new(String::new()),
        state_override: Some("other"),
    };
    assert_eq!(
        login_openai_codex(&fetch, &callbacks, None)
            .await
            .unwrap_err(),
        "State mismatch"
    );
}

#[test]
fn parses_authorization_input_like_the_ts_helper() {
    assert_eq!(parse_authorization_input("  "), (None, None));
    assert_eq!(
        parse_authorization_input("http://localhost:1455/auth/callback?code=c&state=s"),
        (Some("c".into()), Some("s".into()))
    );
    assert_eq!(
        parse_authorization_input("c#s"),
        (Some("c".into()), Some("s".into()))
    );
    assert_eq!(
        parse_authorization_input("code=c&state=s"),
        (Some("c".into()), Some("s".into()))
    );
    assert_eq!(
        parse_authorization_input("bare"),
        (Some("bare".into()), None)
    );
}

#[test]
fn account_id_comes_from_the_jwt_claim() {
    assert_eq!(get_account_id(&token_with_account("x")), Some("x".into()));
    assert_eq!(get_account_id("not-a-jwt"), None);
    assert_eq!(get_account_id(&token_with_account("")), None);
}

#[test]
fn provider_metadata_matches_hoocode() {
    let provider = OpenAICodexOAuthProvider::default();
    assert_eq!(provider.id(), "openai-codex");
    assert_eq!(provider.name(), "ChatGPT Plus/Pro (Codex Subscription)");
    assert!(provider.uses_callback_server());
    let creds = OAuthCredentials::new("r", "a", 0);
    assert_eq!(provider.get_api_key(&creds), "a");
}
