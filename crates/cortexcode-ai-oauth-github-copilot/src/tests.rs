//! Port of hoocode `packages/ai/test/github-copilot-oauth.test.ts` (fake
//! timers -> tokio's paused clock, `fetch` through the [`Fetch`] seam) plus
//! the domain/base-URL helpers.

use super::*;
use cortexcode_ai_oauth::HttpResponse;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Mutex;

fn json_response(body: Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        status_text: "OK".into(),
        body: body.to_string(),
    }
}

/// Answers the device flow; records access-token poll times since `start`.
struct DeviceFlowFetch {
    start: Instant,
    expires_in: u64,
    token_responses: Mutex<VecDeque<Value>>,
    poll_times: Mutex<Vec<u128>>,
}

impl Fetch for DeviceFlowFetch {
    fn fetch(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>> {
        let url = request.url.clone();
        let response = if url.ends_with("/login/device/code") {
            assert_eq!(request.method, "POST");
            assert_eq!(request.header_value("Accept"), Some("application/json"));
            assert_eq!(
                request.header_value("Content-Type"),
                Some("application/x-www-form-urlencoded")
            );
            let body = request.body.clone().unwrap();
            assert!(body.contains("client_id=") && body.contains("scope=read%3Auser"));
            json_response(json!({
                "device_code": "device-code",
                "user_code": "ABCD-EFGH",
                "verification_uri": "https://github.com/login/device",
                "interval": 5,
                "expires_in": self.expires_in,
            }))
        } else if url.ends_with("/login/oauth/access_token") {
            self.poll_times
                .lock()
                .unwrap()
                .push(Instant::now().duration_since(self.start).as_millis());
            let body = request.body.clone().unwrap();
            assert!(body.contains("client_id=") && body.contains("device_code=device-code"));
            assert!(
                body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code")
            );
            let next = self
                .token_responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("Unexpected extra access token poll");
            json_response(next)
        } else if url.contains("/copilot_internal/v2/token") {
            json_response(json!({
                "token": "tid=test;exp=9999999999;proxy-ep=proxy.individual.githubcopilot.com;",
                "expires_at": 9_999_999_999u64,
            }))
        } else if url.contains("/models/") && url.ends_with("/policy") {
            HttpResponse {
                status: 200,
                status_text: "OK".into(),
                body: String::new(),
            }
        } else {
            panic!("Unexpected fetch URL: {url}");
        };
        Box::pin(async move { Ok(response) })
    }
}

struct Silent;

impl OAuthLoginCallbacks for Silent {
    fn on_auth(&self, _info: OAuthAuthInfo) {}
    fn on_prompt(&self, _prompt: OAuthPrompt) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Ok(String::new()) })
    }
}

fn device_fetch(expires_in: u64, responses: Vec<Value>) -> DeviceFlowFetch {
    DeviceFlowFetch {
        start: Instant::now(),
        expires_in,
        token_responses: Mutex::new(responses.into()),
        poll_times: Mutex::new(Vec::new()),
    }
}

#[tokio::test(start_paused = true)]
async fn waits_before_the_first_poll_and_widens_the_margin_after_slow_down() {
    let fetch = device_fetch(
        900,
        vec![
            json!({"error": "authorization_pending", "error_description": "pending"}),
            json!({"error": "slow_down", "error_description": "slow down", "interval": 10}),
            json!({"access_token": "ghu_refresh_token"}),
        ],
    );
    let credentials = login_github_copilot(&fetch, &Silent).await.unwrap();
    assert_eq!(*fetch.poll_times.lock().unwrap(), [6000, 12000, 26000]);
    assert_eq!(credentials.refresh, "ghu_refresh_token");
    assert!(credentials.access.starts_with("tid=test;"));
    assert_eq!(credentials.expires, 9_999_999_999_000 - 5 * 60 * 1000);
    assert!(credentials.extra.is_empty());
}

#[tokio::test(start_paused = true)]
async fn uses_the_remaining_lifetime_for_a_final_poll_before_timing_out() {
    let fetch = device_fetch(
        25,
        vec![
            json!({"error": "slow_down", "error_description": "slow down", "interval": 10}),
            json!({"error": "slow_down", "error_description": "still too fast", "interval": 15}),
            json!({"error": "authorization_pending", "error_description": "pending"}),
        ],
    );
    let err = login_github_copilot(&fetch, &Silent).await.unwrap_err();
    assert!(
        err.contains("Device flow timed out after one or more slow_down responses"),
        "{err}"
    );
    assert_eq!(*fetch.poll_times.lock().unwrap(), [6000, 20000, 25000]);
}

#[tokio::test(start_paused = true)]
async fn other_device_errors_and_cancellation() {
    let fetch = device_fetch(
        900,
        vec![json!({"error": "access_denied", "error_description": "denied"})],
    );
    assert_eq!(
        login_github_copilot(&fetch, &Silent).await.unwrap_err(),
        "Device flow failed: access_denied: denied"
    );
    let signal = AbortSignal::new();
    signal.abort();
    let fetch = device_fetch(900, vec![]);
    assert_eq!(
        poll_for_github_access_token(&fetch, "github.com", "d", 5.0, 900.0, Some(&signal))
            .await
            .unwrap_err(),
        "Login cancelled"
    );
}

#[test]
fn domains_and_base_urls() {
    assert_eq!(normalize_domain(""), None);
    assert_eq!(
        normalize_domain("company.ghe.com").as_deref(),
        Some("company.ghe.com")
    );
    assert_eq!(
        normalize_domain("https://company.ghe.com/some/path").as_deref(),
        Some("company.ghe.com")
    );
    assert_eq!(
        get_github_copilot_base_url(
            Some("tid=1;proxy-ep=proxy.enterprise.githubcopilot.com;x=y"),
            None
        ),
        "https://api.enterprise.githubcopilot.com"
    );
    assert_eq!(
        get_github_copilot_base_url(Some("tid=1"), Some("company.ghe.com")),
        "https://copilot-api.company.ghe.com"
    );
    assert_eq!(
        get_github_copilot_base_url(None, None),
        "https://api.individual.githubcopilot.com"
    );
}

#[test]
fn modify_models_points_copilot_models_at_the_token_base_url() {
    let provider = GitHubCopilotOAuthProvider::default();
    let mut creds =
        OAuthCredentials::new("r", "tid=1;proxy-ep=proxy.business.githubcopilot.com;", 0);
    creds
        .extra
        .insert("enterpriseUrl".into(), json!("company.ghe.com"));
    let models = vec![
        cortexcode_ai_models::get_model("github-copilot", "claude-opus-5")
            .unwrap()
            .clone(),
        cortexcode_ai_models::get_model("anthropic", "claude-opus-5")
            .unwrap()
            .clone(),
    ];
    let modified = provider.modify_models(models, &creds);
    assert_eq!(
        modified[0].base_url,
        "https://api.business.githubcopilot.com"
    );
    assert_eq!(modified[1].base_url, "https://api.anthropic.com");
}

#[tokio::test]
async fn refresh_keeps_the_enterprise_domain_and_reports_bad_responses() {
    struct Once(Mutex<Option<HttpResponse>>, Mutex<Option<HttpRequest>>);
    impl Fetch for Once {
        fn fetch(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>> {
            *self.1.lock().unwrap() = Some(request);
            let response = self.0.lock().unwrap().take().unwrap();
            Box::pin(async move { Ok(response) })
        }
    }
    let fetch = Once(
        Mutex::new(Some(json_response(
            json!({"token": "tok", "expires_at": 1000}),
        ))),
        Mutex::new(None),
    );
    let creds = refresh_github_copilot_token(&fetch, "gh", Some("company.ghe.com"))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&creds).unwrap(),
        json!({"refresh": "gh", "access": "tok", "expires": 1_000_000 - 300_000, "enterpriseUrl": "company.ghe.com"})
    );
    let request = fetch.1.lock().unwrap().take().unwrap();
    assert_eq!(
        request.url,
        "https://api.company.ghe.com/copilot_internal/v2/token"
    );
    assert_eq!(request.header_value("Authorization"), Some("Bearer gh"));
    assert_eq!(
        request.header_value("Copilot-Integration-Id"),
        Some("vscode-chat")
    );

    let fetch = Once(
        Mutex::new(Some(HttpResponse {
            status: 401,
            status_text: "Unauthorized".into(),
            body: "bad token".into(),
        })),
        Mutex::new(None),
    );
    assert_eq!(
        refresh_github_copilot_token(&fetch, "gh", None)
            .await
            .unwrap_err(),
        "401 Unauthorized: bad token"
    );
}
