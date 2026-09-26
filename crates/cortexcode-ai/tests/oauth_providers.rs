//! The OAuth provider registry holds hoocode's built-ins: the registry half
//! of `google-gemini-cli.test.ts` ("Google OAuth providers").

use cortexcode_ai::oauth::{get_oauth_provider, get_oauth_providers, OAuthCredentials};
use serde_json::json;

fn credentials() -> OAuthCredentials {
    let mut creds = OAuthCredentials::new("r", "ya29.token", 0);
    creds.extra.insert("projectId".into(), json!("p"));
    creds
}

#[test]
fn builtin_providers_are_registered_in_hoocode_order() {
    cortexcode_ai::install_builtin_oauth_providers();
    let ids: Vec<String> = get_oauth_providers()
        .iter()
        .map(|p| p.id().to_string())
        .collect();
    assert_eq!(
        ids,
        [
            "anthropic",
            "github-copilot",
            "google-gemini-cli",
            "google-antigravity",
            "openai-codex"
        ]
    );
}

#[test]
fn registers_the_gemini_cli_provider_and_encodes_token_plus_project_into_the_api_key() {
    cortexcode_ai::install_builtin_oauth_providers();
    let provider = get_oauth_provider("google-gemini-cli").unwrap();
    assert!(provider.uses_callback_server());
    assert_eq!(
        provider.get_api_key(&credentials()),
        json!({"token": "ya29.token", "projectId": "p"}).to_string()
    );
}

#[test]
fn registers_the_antigravity_provider() {
    cortexcode_ai::install_builtin_oauth_providers();
    let provider = get_oauth_provider("google-antigravity").unwrap();
    assert!(provider.name().contains("Antigravity"));
    assert_eq!(
        provider.get_api_key(&credentials()),
        json!({"token": "ya29.token", "projectId": "p"}).to_string()
    );
}
