//! The OAuth half of hoocode `google-gemini-cli.test.ts` (both providers,
//! `getApiKey`), plus the flows with `fetch` stubbed through [`Fetch`].

use super::*;
use cortexcode_ai_oauth::{HttpResponse, OAuthPrompt};
use std::sync::{Arc, Mutex};

/// Tests that read the client variables hold this.
async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let guard = LOCK.lock().await;
    for flow in [GEMINI_CLI, ANTIGRAVITY] {
        std::env::set_var(flow.client_env.id_var, "client-id");
        std::env::set_var(flow.client_env.secret_var, "client-secret");
    }
    guard
}

fn ok(body: Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        status_text: "OK".into(),
        body: body.to_string(),
    }
}

fn status(code: u16, text: &str, body: &str) -> HttpResponse {
    HttpResponse {
        status: code,
        status_text: text.into(),
        body: body.into(),
    }
}

type Respond = Box<dyn Fn(&HttpRequest) -> Result<HttpResponse, String> + Send + Sync>;

/// Records every request and answers with `respond`.
#[derive(Clone)]
struct FakeFetch {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    respond: Arc<Respond>,
}

impl FakeFetch {
    fn new(
        respond: impl Fn(&HttpRequest) -> Result<HttpResponse, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            requests: Default::default(),
            respond: Arc::new(Box::new(respond)),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn paths(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| r.url.replace(CODE_ASSIST_ENDPOINT, ""))
            .collect()
    }
}

impl Fetch for FakeFetch {
    fn fetch(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>> {
        let response = (self.respond)(&request);
        self.requests.lock().unwrap().push(request);
        Box::pin(async move { response })
    }
}

#[derive(Default)]
struct Callbacks {
    progress: Mutex<Vec<String>>,
    url: Mutex<Option<String>>,
    /// Build the pasted input from the auth URL.
    paste: Option<fn(&str) -> String>,
}

impl OAuthLoginCallbacks for Callbacks {
    fn on_auth(&self, info: OAuthAuthInfo) {
        *self.url.lock().unwrap() = Some(info.url);
    }
    fn on_prompt(&self, _prompt: OAuthPrompt) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Err("no prompt".to_string()) })
    }
    fn on_progress(&self, message: &str) {
        self.progress.lock().unwrap().push(message.to_string());
    }
    fn on_manual_code_input(&self) -> Option<BoxFuture<'_, Result<String, String>>> {
        let paste = self.paste?;
        let url = self.url.lock().unwrap().clone().unwrap();
        Some(Box::pin(async move { Ok(paste(&url)) }))
    }
}

fn param(url: &str, name: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .unwrap()
}

fn form(request: &HttpRequest) -> Vec<(String, String)> {
    url::form_urlencoded::parse(request.body.as_deref().unwrap().as_bytes())
        .into_owned()
        .collect()
}

fn creds() -> OAuthCredentials {
    let mut creds = OAuthCredentials::new("r", "ya29.token", 0);
    creds.extra.insert("projectId".into(), json!("p"));
    creds
}

// --- google-gemini-cli.test.ts: "Google OAuth providers" ---

#[test]
fn gemini_cli_provider_encodes_token_plus_project_into_the_api_key() {
    let provider = GeminiCliOAuthProvider::default();
    assert_eq!(provider.id(), "google-gemini-cli");
    assert!(provider.uses_callback_server());
    assert_eq!(
        provider.get_api_key(&creds()),
        json!({"token": "ya29.token", "projectId": "p"}).to_string()
    );
}

#[test]
fn antigravity_provider_is_registered_with_its_name() {
    let provider = AntigravityOAuthProvider::default();
    assert_eq!(provider.id(), "google-antigravity");
    assert!(provider.name().contains("Antigravity"));
    assert!(provider.uses_callback_server());
    assert_eq!(
        provider.get_api_key(&creds()),
        r#"{"token":"ya29.token","projectId":"p"}"#
    );
    // `JSON.stringify` drops an undefined projectId.
    assert_eq!(
        provider.get_api_key(&OAuthCredentials::new("r", "a", 0)),
        r#"{"token":"a"}"#
    );
}

// --- google-oauth-client.ts ---

#[test]
fn a_missing_client_names_the_variables_to_set() {
    let env = GEMINI_CLI.client_env;
    let none = |_: &str| None;
    let err = read_google_oauth_client_with(&env, none).unwrap_err();
    assert!(err.starts_with(
        "Google Cloud Code Assist (Gemini CLI) needs an OAuth client, and cortexcode does not ship one. \
         Set CORTEXCODE_GEMINI_CLI_CLIENT_ID and CORTEXCODE_GEMINI_CLI_CLIENT_SECRET to the"
    ));
    let only_id = |v: &str| v.ends_with("_ID").then(|| "id".to_string());
    let err = read_google_oauth_client_with(&env, only_id).unwrap_err();
    assert!(err.contains("Set CORTEXCODE_GEMINI_CLI_CLIENT_SECRET to the"));
    let both = |v: &str| Some(v.to_lowercase());
    assert_eq!(
        read_google_oauth_client_with(&env, both).unwrap(),
        GoogleOAuthClient {
            client_id: "cortexcode_gemini_cli_client_id".into(),
            client_secret: "cortexcode_gemini_cli_client_secret".into(),
        }
    );
}

#[test]
fn redirect_urls_and_the_authorize_url() {
    assert_eq!(
        parse_redirect_url(" http://localhost:8085/oauth2callback?code=c&state=s "),
        (Some("c".into()), Some("s".into()))
    );
    // A bare code is not a URL.
    assert_eq!(parse_redirect_url("4/abc"), (None, None));

    let url = ANTIGRAVITY.authorize_url("id", "chal", "ver");
    assert!(url.starts_with(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id=id&response_type=code&"
    ));
    assert_eq!(
        param(&url, "redirect_uri"),
        "http://localhost:51121/oauth-callback"
    );
    assert!(param(&url, "scope")
        .ends_with("/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs"));
    assert_eq!(param(&url, "state"), "ver");
    assert_eq!(param(&url, "code_challenge_method"), "S256");
    assert_eq!(param(&url, "access_type"), "offline");
    assert_eq!(param(&url, "prompt"), "consent");
    assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcloud-platform+https"));
}

// --- refresh ---

#[tokio::test]
async fn refresh_posts_the_client_and_keeps_the_refresh_token_and_project() {
    let _env = env_lock().await;
    let fetch = FakeFetch::new(|_| Ok(ok(json!({"access_token": "a2", "expires_in": 3600}))));
    let before = cortexcode_ai_oauth::now_ms();
    let fresh = refresh_google_cloud_token(&fetch, "r1", "proj")
        .await
        .unwrap();
    assert_eq!(
        (fresh.access.as_str(), fresh.refresh.as_str()),
        ("a2", "r1")
    );
    assert_eq!(fresh.extra_str("projectId"), Some("proj"));
    assert!(fresh.expires >= before + 3_600_000 - EXPIRY_MARGIN_MS);
    let request = &fetch.requests()[0];
    assert_eq!(request.url, TOKEN_URL);
    assert_eq!(
        form(request),
        [
            ("client_id".into(), "client-id".into()),
            ("client_secret".into(), "client-secret".into()),
            ("refresh_token".into(), "r1".into()),
            ("grant_type".into(), "refresh_token".into()),
        ]
    );

    let failing = FakeFetch::new(|_| Ok(status(400, "Bad Request", "invalid_grant")));
    assert_eq!(
        refresh_antigravity_token(&failing, "r", "p")
            .await
            .unwrap_err(),
        "Antigravity token refresh failed: invalid_grant"
    );
    assert_eq!(
        refresh_google_cloud_token(&failing, "r", "p")
            .await
            .unwrap_err(),
        "Google Cloud token refresh failed: invalid_grant"
    );
}

#[tokio::test]
async fn refreshing_without_a_project_fails_before_any_request() {
    let fetch = FakeFetch::new(|_| panic!("no request expected"));
    let provider = GeminiCliOAuthProvider::with_fetch(Box::new(fetch.clone()));
    let bare = OAuthCredentials::new("r", "a", 0);
    assert_eq!(
        provider.refresh_token(&bare).await.unwrap_err(),
        "Google Cloud credentials missing projectId"
    );
    let provider = AntigravityOAuthProvider::with_fetch(Box::new(fetch));
    assert_eq!(
        provider.refresh_token(&bare).await.unwrap_err(),
        "Antigravity credentials missing projectId"
    );
}

// --- Gemini CLI project discovery ---

fn silent() -> Callbacks {
    Callbacks::default()
}

async fn gemini_discover(fetch: &FakeFetch, env_project: Option<&str>) -> Result<String, String> {
    gemini_cli::discover_project(fetch, "tok", &silent(), env_project.map(str::to_string), 0).await
}

#[tokio::test]
async fn gemini_cli_uses_the_existing_project_or_the_environment() {
    let fetch = FakeFetch::new(|_| {
        Ok(ok(
            json!({"currentTier": {"id": "standard-tier"}, "cloudaicompanionProject": "existing"}),
        ))
    });
    assert_eq!(gemini_discover(&fetch, None).await.unwrap(), "existing");
    let request = &fetch.requests()[0];
    assert_eq!(request.header_value("authorization"), Some("Bearer tok"));
    assert_eq!(
        request.header_value("x-goog-api-client"),
        Some("gl-node/22.17.0")
    );
    assert_eq!(
        request.body.as_deref(),
        Some(
            r#"{"metadata":{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}}"#
        )
    );

    let tier_only = FakeFetch::new(|_| Ok(ok(json!({"currentTier": {"id": "standard-tier"}}))));
    assert_eq!(
        gemini_discover(&tier_only, Some("mine")).await.unwrap(),
        "mine"
    );
    let load: Value =
        serde_json::from_str(tier_only.requests()[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(load["cloudaicompanionProject"], "mine");
    assert_eq!(load["metadata"]["duetProject"], "mine");
    assert!(gemini_discover(&tier_only, None)
        .await
        .unwrap_err()
        .starts_with("This account requires setting the GOOGLE_CLOUD_PROJECT"));
}

#[tokio::test]
async fn gemini_cli_treats_a_vpc_sc_rejection_as_standard_tier() {
    let fetch = FakeFetch::new(|_| {
        Ok(status(
            403,
            "Forbidden",
            r#"{"error":{"details":[{"reason":"SECURITY_POLICY_VIOLATED"}]}}"#,
        ))
    });
    assert_eq!(gemini_discover(&fetch, Some("vpc")).await.unwrap(), "vpc");
    let other = FakeFetch::new(|_| Ok(status(500, "Internal Server Error", "boom")));
    assert_eq!(
        gemini_discover(&other, None).await.unwrap_err(),
        "loadCodeAssist failed: 500 Internal Server Error: boom"
    );
}

#[tokio::test]
async fn gemini_cli_onboards_the_free_tier_and_polls_the_operation() {
    let polls = Arc::new(Mutex::new(0));
    let counter = polls.clone();
    let fetch = FakeFetch::new(move |request| {
        let path = request.url.replace(CODE_ASSIST_ENDPOINT, "");
        Ok(match path.as_str() {
            "/v1internal:loadCodeAssist" => ok(json!({"allowedTiers": [
                {"id": "legacy-tier"}, {"id": "free-tier", "isDefault": true}]})),
            "/v1internal:onboardUser" => ok(json!({"name": "operations/op1", "done": false})),
            "/v1internal/operations/op1" => {
                let mut n = counter.lock().unwrap();
                *n += 1;
                if *n < 2 {
                    ok(json!({"done": false}))
                } else {
                    ok(
                        json!({"done": true, "response": {"cloudaicompanionProject": {"id": "free-proj"}}}),
                    )
                }
            }
            other => panic!("unexpected {other}"),
        })
    });
    let callbacks = silent();
    let project = gemini_cli::discover_project(&fetch, "tok", &callbacks, Some("env".into()), 0)
        .await
        .unwrap();
    assert_eq!(project, "free-proj");
    // The free tier never sends a project.
    let onboard: Value =
        serde_json::from_str(fetch.requests()[1].body.as_deref().unwrap()).unwrap();
    assert_eq!(
        onboard,
        json!({"tierId": "free-tier", "metadata": {"ideType": "IDE_UNSPECIFIED",
               "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}})
    );
    assert_eq!(fetch.requests()[2].method, "GET");
    assert_eq!(
        *callbacks.progress.lock().unwrap(),
        [
            "Checking for existing Cloud Code Assist project...",
            "Provisioning Cloud Code Assist project (this may take a moment)...",
            "Waiting for project provisioning (attempt 2)...",
        ]
    );
}

#[tokio::test]
async fn gemini_cli_paid_tiers_need_a_project_and_send_it() {
    let no_tiers = FakeFetch::new(|_| Ok(ok(json!({}))));
    // No allowed tiers means legacy-tier, which needs a project.
    assert!(gemini_discover(&no_tiers, None)
        .await
        .unwrap_err()
        .starts_with("This account requires"));

    let fetch = FakeFetch::new(|request| {
        Ok(if request.url.ends_with(":loadCodeAssist") {
            ok(json!({}))
        } else {
            ok(json!({"done": true}))
        })
    });
    assert_eq!(gemini_discover(&fetch, Some("paid")).await.unwrap(), "paid");
    assert_eq!(
        fetch.requests()[1].body.as_deref(),
        Some(concat!(
            r#"{"tierId":"legacy-tier","metadata":{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","#,
            r#""pluginType":"GEMINI","duetProject":"paid"},"cloudaicompanionProject":"paid"}"#
        ))
    );

    let failing = FakeFetch::new(|request| {
        Ok(if request.url.ends_with(":loadCodeAssist") {
            ok(json!({"allowedTiers": [{"id": "free-tier", "isDefault": true}]}))
        } else {
            status(429, "Too Many Requests", "later")
        })
    });
    assert_eq!(
        gemini_discover(&failing, None).await.unwrap_err(),
        "onboardUser failed: 429 Too Many Requests: later"
    );
    let empty = FakeFetch::new(|request| {
        Ok(if request.url.ends_with(":loadCodeAssist") {
            ok(json!({"allowedTiers": [{"id": "free-tier", "isDefault": true}]}))
        } else {
            ok(json!({"done": true}))
        })
    });
    assert!(gemini_discover(&empty, None)
        .await
        .unwrap_err()
        .starts_with("Could not discover or provision a Google Cloud project."));
}

// --- Antigravity project discovery ---

async fn antigravity_discover(
    fetch: &FakeFetch,
    env_project: Option<&str>,
) -> (String, Vec<String>) {
    let callbacks = silent();
    let project =
        antigravity::discover_project(fetch, "tok", &callbacks, env_project.map(str::to_string), 0)
            .await;
    let progress = callbacks.progress.lock().unwrap().clone();
    (project, progress)
}

#[tokio::test]
async fn antigravity_reads_an_existing_project_in_either_shape() {
    let fetch = FakeFetch::new(|_| Ok(ok(json!({"cloudaicompanionProject": {"id": "obj-proj"}}))));
    assert_eq!(antigravity_discover(&fetch, None).await.0, "obj-proj");
    let request = &fetch.requests()[0];
    assert_eq!(
        request.header_value("client-metadata"),
        Some(
            r#"{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#
        )
    );
    assert_eq!(
        request.header_value("x-goog-api-client"),
        Some("google-cloud-sdk vscode_cloudshelleditor/0.1")
    );
    let fetch = FakeFetch::new(|_| Ok(ok(json!({"cloudaicompanionProject": "str-proj"}))));
    assert_eq!(antigravity_discover(&fetch, None).await.0, "str-proj");
}

#[tokio::test]
async fn antigravity_onboards_with_polling() {
    let fetch = FakeFetch::new(|request| {
        let path = request.url.replace(CODE_ASSIST_ENDPOINT, "");
        Ok(match path.as_str() {
            "/v1internal:loadCodeAssist" => ok(json!({"currentTier": {"id": "free-tier"}})),
            "/v1internal:onboardUser" => ok(json!({"name": "operations/o", "done": false})),
            _ => ok(json!({"done": true, "response": {"cloudaicompanionProject": "managed-1"}})),
        })
    });
    let (project, progress) = antigravity_discover(&fetch, Some("env")).await;
    assert_eq!(project, "managed-1");
    let onboard: Value =
        serde_json::from_str(fetch.requests()[1].body.as_deref().unwrap()).unwrap();
    assert_eq!(onboard["tierId"], "free-tier");
    assert_eq!(onboard["cloudaicompanionProject"], "env");
    assert_eq!(
        progress,
        [
            "Checking for an existing project...",
            "Provisioning the Antigravity project (this may take a moment)...",
            "Waiting for project provisioning (attempt 2)...",
        ]
    );
}

#[tokio::test]
async fn antigravity_falls_through_to_the_environment_then_the_managed_project() {
    // loadCodeAssist fails outright: no tier, so no onboarding.
    let offline = FakeFetch::new(|_| Err("fetch failed".to_string()));
    let (project, progress) = antigravity_discover(&offline, Some("env")).await;
    assert_eq!(project, "env");
    assert_eq!(offline.paths(), ["/v1internal:loadCodeAssist"]);
    assert_eq!(progress, ["Checking for an existing project..."]);

    // An onboarding 403 falls through too; the default tier is used.
    let refused = FakeFetch::new(|request| {
        Ok(if request.url.ends_with(":loadCodeAssist") {
            ok(json!({"allowedTiers": [{"id": "a"}, {"id": "b", "isDefault": true}]}))
        } else {
            status(403, "Forbidden", "not eligible")
        })
    });
    let (project, progress) = antigravity_discover(&refused, None).await;
    assert_eq!(project, "aicode-consumers");
    let onboard: Value =
        serde_json::from_str(refused.requests()[1].body.as_deref().unwrap()).unwrap();
    assert_eq!(onboard["tierId"], "b");
    assert_eq!(
        progress.last().unwrap(),
        "Using Google's managed Antigravity project..."
    );
}

// --- the login flow ---

#[tokio::test]
async fn gemini_cli_login_takes_a_pasted_redirect_and_builds_credentials() {
    let _env = env_lock().await;
    let fetch = FakeFetch::new(|request| {
        Ok(if request.url == TOKEN_URL {
            ok(json!({"access_token": "acc", "refresh_token": "ref", "expires_in": 3600}))
        } else if request.url.contains("userinfo") {
            ok(json!({"email": "me@example.com"}))
        } else {
            ok(json!({"currentTier": {"id": "standard-tier"}, "cloudaicompanionProject": "proj"}))
        })
    });
    let callbacks = Callbacks {
        paste: Some(|url| {
            format!(
                "http://localhost:8085/oauth2callback?code=the-code&state={}",
                param(url, "state")
            )
        }),
        ..Default::default()
    };
    let creds = login_gemini_cli(&fetch, &callbacks).await.unwrap();
    assert_eq!(
        (creds.access.as_str(), creds.refresh.as_str()),
        ("acc", "ref")
    );
    assert_eq!(creds.extra_str("projectId"), Some("proj"));
    assert_eq!(creds.extra_str("email"), Some("me@example.com"));

    let auth_url = callbacks.url.lock().unwrap().clone().unwrap();
    assert_eq!(param(&auth_url, "client_id"), "client-id");
    let exchange = form(&fetch.requests()[0]);
    let get = |k: &str| exchange.iter().find(|(n, _)| n == k).unwrap().1.clone();
    assert_eq!(get("code"), "the-code");
    assert_eq!(get("client_secret"), "client-secret");
    assert_eq!(get("grant_type"), "authorization_code");
    // The state is the PKCE verifier.
    assert_eq!(get("code_verifier"), param(&auth_url, "state"));
    assert_eq!(
        *callbacks.progress.lock().unwrap(),
        [
            "Starting local server for OAuth callback...",
            "Waiting for OAuth callback...",
            "Exchanging authorization code for tokens...",
            "Getting user info...",
            "Checking for existing Cloud Code Assist project...",
        ]
    );
}

#[tokio::test]
async fn login_rejects_a_pasted_state_mismatch_and_input_without_a_code() {
    let _env = env_lock().await;
    let fetch = FakeFetch::new(|_| panic!("no request expected"));
    let mismatch = Callbacks {
        paste: Some(|_| "http://localhost:51121/oauth-callback?code=c&state=wrong".to_string()),
        ..Default::default()
    };
    assert_eq!(
        login_antigravity(&fetch, &mismatch).await.unwrap_err(),
        STATE_MISMATCH
    );
    let bare = Callbacks {
        paste: Some(|_| "just-a-code".to_string()),
        ..Default::default()
    };
    // The first login's server is closed by aborting its task; wait until
    // the port is free again.
    for _ in 0..100 {
        if std::net::TcpListener::bind(("127.0.0.1", 51121)).is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        login_antigravity(&fetch, &bare).await.unwrap_err(),
        "No authorization code received"
    );
}
