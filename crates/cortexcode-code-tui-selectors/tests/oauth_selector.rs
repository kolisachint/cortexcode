//! Port of `oauth-selector.test.ts`, plus the login dialog's input
//! hand-off (no TS test of its own).

mod support;

use std::collections::HashSet;
use std::sync::Arc;

use cortexcode_code_auth::provider_display_names::built_in_provider_display_name;
use cortexcode_code_auth::{AuthCredential, AuthSource, AuthStatus, AuthStorage};
use cortexcode_code_tui_selectors::login_dialog::{LoginDialogComponent, LoginDialogEvent};
use cortexcode_code_tui_selectors::oauth_selector::{
    is_api_key_login_provider, AuthSelectorProvider, AuthType, LoginMode, OAuthSelectorComponent,
};
use cortexcode_tui_render::Component;

use support::{lock, strip};

fn ids(values: &[&str]) -> HashSet<String> {
    values.iter().map(|v| v.to_string()).collect()
}

fn provider(id: &str, name: &str, auth_type: AuthType) -> Vec<AuthSelectorProvider> {
    vec![AuthSelectorProvider {
        id: id.into(),
        name: name.into(),
        auth_type,
    }]
}

fn render(selector: &mut OAuthSelectorComponent) -> String {
    strip(&selector.render(120))
}

/// Restores `OPENAI_API_KEY` when dropped.
struct EnvGuard(Option<String>);
impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => std::env::set_var("OPENAI_API_KEY", v),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }
}

#[test]
fn keeps_built_in_api_key_providers_separate_from_oauth_only_providers() {
    let oauth = ids(&["anthropic", "github-copilot", "custom-oauth"]);
    let built_in = ids(&["anthropic", "github-copilot", "openai"]);
    let check = |id| is_api_key_login_provider(id, &oauth, Some(&built_in));
    assert!(check("anthropic"));
    assert_eq!(
        built_in_provider_display_name("anthropic"),
        Some("Anthropic")
    );
    assert!(check("openai"));
    assert!(!check("github-copilot"));
    assert!(!check("custom-oauth"));
    assert!(check("custom-api"));
}

#[test]
fn shows_stored_oauth_auth_distinctly_in_the_api_key_selector() {
    let _g = lock();
    let credential: AuthCredential = serde_json::from_value(serde_json::json!({
        "type": "oauth", "access": "access-token", "refresh": "refresh-token",
        "expires": 4_102_444_800_000u64
    }))
    .unwrap();
    let auth = Arc::new(AuthStorage::in_memory([(
        "anthropic".to_string(),
        credential,
    )]));
    let mut selector = OAuthSelectorComponent::new(
        LoginMode::Login,
        auth,
        provider("anthropic", "Anthropic", AuthType::ApiKey),
        None,
    );
    let output = render(&mut selector);
    assert!(output.contains("Anthropic"));
    assert!(output.contains("subscription configured"));
}

#[test]
fn shows_environment_api_key_auth_as_configured() {
    let _g = lock();
    let _env = EnvGuard(std::env::var("OPENAI_API_KEY").ok());
    std::env::set_var("OPENAI_API_KEY", "test-openai-key");
    let auth = Arc::new(AuthStorage::in_memory([]));
    let mut selector = OAuthSelectorComponent::new(
        LoginMode::Login,
        auth,
        provider("openai", "OpenAI", AuthType::ApiKey),
        None,
    );
    let output = render(&mut selector);
    assert!(output.contains("OpenAI"));
    assert!(output.contains("✓ env: OPENAI_API_KEY"));
    assert!(!output.contains("unconfigured"));
}

fn with_status(id: &str, status: AuthStatus) -> String {
    let auth = Arc::new(AuthStorage::in_memory([]));
    let mut selector = OAuthSelectorComponent::new(
        LoginMode::Login,
        auth,
        provider(id, id, AuthType::ApiKey),
        Some(Box::new(move |_: &str| status.clone())),
    );
    render(&mut selector)
}

#[test]
fn shows_custom_provider_environment_api_key_auth_from_the_status_resolver() {
    let _g = lock();
    let output = with_status(
        "ollama",
        AuthStatus {
            configured: true,
            source: Some(AuthSource::Environment),
            label: Some("OLLAMA_API_KEY".into()),
        },
    );
    assert!(output.contains("ollama"));
    assert!(output.contains("✓ env: OLLAMA_API_KEY"));
    assert!(!output.contains("unconfigured"));
}

#[test]
fn shows_models_json_api_key_auth_as_configured() {
    let _g = lock();
    let output = with_status(
        "local-proxy",
        AuthStatus {
            configured: true,
            source: Some(AuthSource::ModelsJsonKey),
            label: None,
        },
    );
    assert!(output.contains("local-proxy"));
    assert!(output.contains("✓ key in models.json"));
    assert!(!output.contains("unconfigured"));
}

#[test]
fn shows_models_json_command_auth_as_configured() {
    let _g = lock();
    let output = with_status(
        "op-proxy",
        AuthStatus {
            configured: true,
            source: Some(AuthSource::ModelsJsonCommand),
            label: None,
        },
    );
    assert!(output.contains("op-proxy"));
    assert!(output.contains("✓ command in models.json"));
    assert!(!output.contains("unconfigured"));
}

#[test]
fn the_login_dialog_hands_back_a_prompted_answer_and_aborts_on_escape() {
    let _g = lock();
    let mut dialog = LoginDialogComponent::new("OpenAI", None);
    let screen = strip(&dialog.render(80));
    assert!(screen.contains("Login to OpenAI"));
    assert!(screen.contains("Starting…"));

    // Enter before anything asks is not an answer.
    dialog.handle_input("\r");
    assert!(dialog.take_events().is_empty());

    dialog.show_prompt("Enter API key:", None);
    assert!(!strip(&dialog.render(80)).contains("Starting…"));
    for ch in ["s", "k", "-", "1"] {
        dialog.handle_input(ch);
    }
    dialog.handle_input("\r");
    assert_eq!(
        dialog.take_events(),
        [LoginDialogEvent::Submitted("sk-1".into())]
    );

    let signal = dialog.signal();
    dialog.handle_input("\x1b");
    assert_eq!(dialog.take_events(), [LoginDialogEvent::Cancelled]);
    assert!(signal.aborted());
}
