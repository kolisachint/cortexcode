//! The `/login` flow's logic (`login-controller.ts`, `utils/open-url.ts`);
//! the panes are covered by the `login-api-key` parity scenario.

use std::sync::{Arc, Mutex};

use cortexcode_ai_oauth::{OAuthAuthInfo, OAuthLoginCallbacks, OAuthPrompt};
use cortexcode_ai_types::{AbortSignal, Model};
use cortexcode_code_auth::{AuthCredential, AuthStorage};
use cortexcode_code_models::ModelRegistry;
use cortexcode_code_tui_app::login_controller::{
    is_openable_url, logged_out_message, login_provider_options, logout_provider_options,
    post_login_model, LoginUpdate, OAuthBridge, PostLoginModel, LOGIN_CANCELLED,
};
use cortexcode_code_tui_selectors::oauth_selector::{AuthSelectorProvider, AuthType};
use serde_json::json;

fn model(provider: &str, id: &str) -> Model {
    serde_json::from_value(json!({
        "id": id, "name": id, "api": "openai-completions", "provider": provider,
        "baseUrl": "https://example.test", "reasoning": false, "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 128000, "maxTokens": 8192,
    }))
    .unwrap()
}

fn registry_with_custom_provider() -> (tempfile::TempDir, ModelRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    let config = json!({"providers": {"zz-custom": {
        "baseUrl": "https://example.com/v1", "apiKey": "KEY", "api": "openai-completions",
        "models": [{"id": "m", "name": "M", "reasoning": false, "input": ["text"],
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
            "contextWindow": 1000, "maxTokens": 100}],
    }}});
    std::fs::write(&path, config.to_string()).unwrap();
    (dir, ModelRegistry::create(path))
}

#[test]
fn api_key_options_are_the_key_providers_by_name() {
    let (_dir, registry) = registry_with_custom_provider();
    let auth = AuthStorage::in_memory([]);
    let options = login_provider_options(&auth, &registry, Some(AuthType::ApiKey));
    assert!(options.iter().all(|o| o.auth_type == AuthType::ApiKey));
    let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
    assert!(ids.contains(&"openai"));
    assert!(ids.contains(&"zz-custom"));
    let names: Vec<&str> = options.iter().map(|o| o.name.as_str()).collect();
    assert!(names.contains(&"OpenAI"));
    // A custom provider without a display name shows its id.
    assert!(names.contains(&"zz-custom"));
    let mut sorted = names.clone();
    sorted.sort_by_key(|n| n.to_lowercase());
    assert_eq!(names, sorted);
    // Each provider is listed once.
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len());
}

#[test]
fn logout_options_are_the_stored_credentials() {
    let auth = AuthStorage::in_memory([
        (
            "openai".to_string(),
            AuthCredential::ApiKey { key: "k".into() },
        ),
        (
            "mock".to_string(),
            AuthCredential::ApiKey { key: "k".into() },
        ),
    ]);
    let options = logout_provider_options(&auth);
    assert_eq!(
        options,
        vec![
            AuthSelectorProvider {
                id: "mock".into(),
                name: "mock".into(),
                auth_type: AuthType::ApiKey
            },
            AuthSelectorProvider {
                id: "openai".into(),
                name: "OpenAI".into(),
                auth_type: AuthType::ApiKey
            },
        ]
    );
    assert_eq!(
        logged_out_message(&options[0]),
        "Removed stored API key for mock. Environment variables and models.json config are unchanged."
    );
    let oauth = AuthSelectorProvider {
        auth_type: AuthType::OAuth,
        ..options[1].clone()
    };
    assert_eq!(logged_out_message(&oauth), "Logged out of OpenAI");
}

#[test]
fn post_login_model_selects_the_provider_default_only_without_a_model() {
    let label = "Saved API key for OpenAI";
    let available = vec![model("openai", "gpt-5.4"), model("openai", "other")];
    assert!(matches!(
        post_login_model(true, "openai", label, &available),
        PostLoginModel::Keep
    ));
    match post_login_model(false, "openai", label, &available) {
        PostLoginModel::Select(m) => assert_eq!(m.id, "gpt-5.4"),
        _ => panic!("expected the default model"),
    }
    match post_login_model(false, "openai", label, &[model("openai", "other")]) {
        PostLoginModel::Error(e) => assert_eq!(
            e,
            "Saved API key for OpenAI, but its default model \"gpt-5.4\" is not available. Use /model to select a model."
        ),
        _ => panic!("expected an error"),
    }
    match post_login_model(false, "openai", label, &[]) {
        PostLoginModel::Error(e) => assert!(e.contains("no models are available")),
        _ => panic!("expected an error"),
    }
    match post_login_model(false, "zz-custom", label, &available) {
        PostLoginModel::Error(e) => {
            assert!(e.contains("no default model is configured for provider \"zz-custom\""))
        }
        _ => panic!("expected an error"),
    }
}

#[test]
fn only_web_and_mail_urls_are_openable() {
    assert!(is_openable_url("https://example.com/auth?x=1"));
    assert!(is_openable_url("http://localhost:1455/cb"));
    assert!(is_openable_url("mailto:a@b.c"));
    assert!(!is_openable_url("file:///etc/passwd"));
    assert!(!is_openable_url("javascript:alert(1)"));
    assert!(!is_openable_url("vscode://open"));
    assert!(!is_openable_url("not a url"));
    assert!(!is_openable_url("https:"));
}

fn bridge(uses_callback_server: bool) -> (OAuthBridge, Arc<Mutex<Vec<LoginUpdate>>>) {
    let updates: Arc<Mutex<Vec<LoginUpdate>>> = Arc::default();
    let sink = updates.clone();
    let bridge = OAuthBridge::new(
        Box::new(move |u| sink.lock().unwrap().push(u)),
        AbortSignal::new(),
        uses_callback_server,
    );
    (bridge, updates)
}

#[tokio::test]
async fn prompts_are_answered_by_the_mode_and_a_dropped_answer_cancels() {
    let (bridge, updates) = bridge(false);
    let answer = bridge.on_prompt(OAuthPrompt {
        message: "Code:".into(),
        ..Default::default()
    });
    let reply = match updates.lock().unwrap().pop() {
        Some(LoginUpdate::Prompt { prompt, reply }) => {
            assert_eq!(prompt.message, "Code:");
            reply
        }
        _ => panic!("expected a prompt"),
    };
    reply.send("abc".into()).unwrap();
    assert_eq!(answer.await, Ok("abc".into()));

    let answer = bridge.on_prompt(OAuthPrompt::default());
    drop(updates.lock().unwrap().pop());
    assert_eq!(answer.await, Err(LOGIN_CANCELLED.into()));
    assert!(bridge.signal().is_some());
}

#[tokio::test]
async fn a_callback_server_login_takes_a_pasted_redirect_url() {
    let (bridge, updates) = bridge(true);
    bridge.on_auth(OAuthAuthInfo {
        url: "https://example.com".into(),
        instructions: None,
    });
    let manual = match updates.lock().unwrap().pop() {
        Some(LoginUpdate::Auth { info, manual }) => {
            assert_eq!(info.url, "https://example.com");
            manual.expect("manual input armed")
        }
        _ => panic!("expected auth"),
    };
    let pasted = bridge.on_manual_code_input().expect("manual input");
    manual.send("http://localhost/cb?code=1".into()).unwrap();
    assert_eq!(pasted.await, Ok("http://localhost/cb?code=1".into()));

    let (plain, updates) = self::bridge(false);
    plain.on_auth(OAuthAuthInfo::default());
    assert!(matches!(
        updates.lock().unwrap().pop(),
        Some(LoginUpdate::Auth { manual: None, .. })
    ));
    assert!(plain.on_manual_code_input().is_none());
}
