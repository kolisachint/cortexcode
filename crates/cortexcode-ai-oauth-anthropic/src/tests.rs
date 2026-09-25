//! Port of hoocode `packages/ai/test/anthropic-oauth.test.ts` (`fetch`
//! stubbed through the [`Fetch`] seam) plus `parseAuthorizationInput`.

use super::*;
use cortexcode_ai_oauth::HttpResponse;
use std::sync::Mutex;

/// Records every request and answers with `respond`.
struct FakeFetch {
    requests: Mutex<Vec<HttpRequest>>,
    respond: Box<dyn Fn(&HttpRequest) -> HttpResponse + Send + Sync>,
}

impl FakeFetch {
    fn new(respond: impl Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
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

fn json_response(body: Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        status_text: "OK".into(),
        body: body.to_string(),
    }
}

fn body_of(request: &HttpRequest) -> Value {
    serde_json::from_str(request.body.as_deref().unwrap()).unwrap()
}

/// Callbacks that paste the redirect URL back as manual input.
struct ManualPaste {
    auth_url: Mutex<String>,
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
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
                .expect("state and redirect_uri in the auth URL")
        };
        let pasted = format!(
            "{}?code=manual-code&state={}",
            param("redirect_uri"),
            param("state")
        );
        Some(Box::pin(async move { Ok(pasted) }))
    }
}

#[tokio::test]
async fn keeps_the_localhost_redirect_uri_for_manual_callback_login() {
    let fetch = FakeFetch::new(|request| {
        assert_eq!(request.url, "https://platform.claude.com/v1/oauth/token");
        assert_eq!(request.method, "POST");
        let body = body_of(request);
        assert_eq!(body["grant_type"], "authorization_code");
        assert_eq!(body["code"], "manual-code");
        assert_eq!(body["redirect_uri"], "http://localhost:53692/callback");
        json_response(
            json!({"access_token": "access-token", "refresh_token": "refresh-token", "expires_in": 3600}),
        )
    });
    let callbacks = ManualPaste {
        auth_url: Mutex::new(String::new()),
    };
    let credentials = login_anthropic(&fetch, &callbacks).await.unwrap();
    assert_eq!(credentials.access, "access-token");
    assert_eq!(credentials.refresh, "refresh-token");
    assert_eq!(fetch.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn omits_scope_from_refresh_token_requests() {
    let fetch = FakeFetch::new(|request| {
        assert_eq!(request.url, "https://platform.claude.com/v1/oauth/token");
        assert_eq!(request.method, "POST");
        let body = body_of(request);
        assert_eq!(body["grant_type"], "refresh_token");
        assert!(body["client_id"].as_str().is_some_and(|c| !c.is_empty()));
        assert_eq!(body["refresh_token"], "refresh-token");
        assert!(body.get("scope").is_none());
        json_response(
            json!({"access_token": "new-access-token", "refresh_token": "new-refresh-token", "expires_in": 3600}),
        )
    });
    let credentials = refresh_anthropic_token(&fetch, "refresh-token")
        .await
        .unwrap();
    assert_eq!(credentials.access, "new-access-token");
    assert_eq!(credentials.refresh, "new-refresh-token");
    assert_eq!(fetch.requests.lock().unwrap().len(), 1);
    // Five minutes of margin on the stated lifetime.
    let now = cortexcode_ai_oauth::now_ms();
    assert!(credentials.expires > now + 54 * 60_000 && credentials.expires <= now + 55 * 60_000);
}

#[tokio::test]
async fn http_and_json_failures_carry_the_ts_messages() {
    let fetch = FakeFetch::new(|_| HttpResponse {
        status: 400,
        status_text: "Bad Request".into(),
        body: "nope".into(),
    });
    assert_eq!(
        refresh_anthropic_token(&fetch, "r").await.unwrap_err(),
        "Anthropic token refresh request failed. url=https://platform.claude.com/v1/oauth/token; details=Error: HTTP request failed. status=400; url=https://platform.claude.com/v1/oauth/token; body=nope"
    );
    let fetch = FakeFetch::new(|_| HttpResponse {
        status: 200,
        status_text: "OK".into(),
        body: "not json".into(),
    });
    assert!(refresh_anthropic_token(&fetch, "r")
        .await
        .unwrap_err()
        .starts_with("Anthropic token refresh returned invalid JSON. url=https://platform.claude.com/v1/oauth/token; body=not json; details=Error: "));
}

#[test]
fn authorization_input_forms() {
    let parse = parse_authorization_input;
    assert_eq!(
        parse("http://localhost:53692/callback?code=abc&state=xyz"),
        (Some("abc".into()), Some("xyz".into()))
    );
    // A URL without the params yields neither (no fall-through).
    assert_eq!(parse("http://localhost:53692/callback"), (None, None));
    assert_eq!(parse("abc#xyz"), (Some("abc".into()), Some("xyz".into())));
    assert_eq!(
        parse("code=abc&state=xyz"),
        (Some("abc".into()), Some("xyz".into()))
    );
    assert_eq!(parse("just-a-code"), (Some("just-a-code".into()), None));
    assert_eq!(parse("   "), (None, None));
    assert_eq!(
        parse_pasted("abc#other", "v"),
        Err("OAuth state mismatch".into())
    );
    assert_eq!(
        parse_pasted("abc", "v"),
        Ok((Some("abc".into()), Some("v".into())))
    );
}

#[test]
fn authorize_url_carries_pkce_and_the_localhost_redirect() {
    let url = url::Url::parse(&authorize_url("chal", "verif")).unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(
        url.as_str().split('?').next(),
        Some("https://claude.ai/oauth/authorize")
    );
    assert_eq!(params["code"], "true");
    assert_eq!(params["redirect_uri"], "http://localhost:53692/callback");
    assert_eq!(params["code_challenge"], "chal");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["state"], "verif");
    assert_eq!(params["scope"], SCOPES);
}
