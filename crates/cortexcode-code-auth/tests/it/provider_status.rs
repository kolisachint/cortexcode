//! Ports of `model-registry.test.ts`'s `getProviderDisplayName` and
//! `getProviderAuthStatus` cases (the registry's lookups live here, next to
//! the credential store they read). `registerProvider` names join once
//! extension-registered providers are ported.

use std::sync::Arc;

use cortexcode_ai_oauth::{
    register_oauth_provider, BoxFuture, OAuthCredentials, OAuthLoginCallbacks, OAuthProvider,
};
use cortexcode_code_auth::provider_display_names::{provider_auth_status, provider_display_name};
use cortexcode_code_auth::{AuthSource, AuthStatus, AuthStorage};
use cortexcode_code_models::ModelRegistry;
use serde_json::json;

struct Named(&'static str, &'static str);

impl OAuthProvider for Named {
    fn id(&self) -> &str {
        self.0
    }
    fn name(&self) -> &str {
        self.1
    }
    fn login<'a>(
        &'a self,
        _callbacks: &'a dyn OAuthLoginCallbacks,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(async { Err("unused".to_string()) })
    }
    fn refresh_token<'a>(
        &'a self,
        credentials: &'a OAuthCredentials,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(async move { Ok(credentials.clone()) })
    }
    fn get_api_key(&self, credentials: &OAuthCredentials) -> String {
        credentials.access.clone()
    }
}

fn registry_with_api_key(api_key: &str) -> (tempfile::TempDir, ModelRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    let providers = json!({"providers": {"custom-provider": {
        "baseUrl": "https://example.com/v1",
        "apiKey": api_key,
        "api": "anthropic-messages",
        "models": [{
            "id": "test-model", "name": "Test Model", "reasoning": false, "input": ["text"],
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
            "contextWindow": 100000, "maxTokens": 8000,
        }],
    }}});
    std::fs::write(&path, providers.to_string()).unwrap();
    (dir, ModelRegistry::create(path))
}

#[test]
fn display_name_resolves_oauth_built_in_and_fallback_names() {
    register_oauth_provider(Arc::new(Named("github-copilot", "GitHub Copilot")));
    let auth = AuthStorage::in_memory([]);
    assert_eq!(provider_display_name(&auth, "openai"), "OpenAI");
    assert_eq!(
        provider_display_name(&auth, "github-copilot"),
        "GitHub Copilot"
    );
    assert_eq!(
        provider_display_name(&auth, "unknown-provider"),
        "unknown-provider"
    );
}

#[test]
fn provider_auth_status_reports_api_key_environment_variables_from_models_json() {
    let name = "TEST_API_KEY_STATUS_TEST_98765";
    std::env::set_var(name, "status-test-key");
    let (_dir, registry) = registry_with_api_key(name);
    let auth = AuthStorage::in_memory([]);
    let status = provider_auth_status(&auth, &registry, "custom-provider");
    std::env::remove_var(name);
    assert_eq!(
        status,
        AuthStatus {
            configured: true,
            source: Some(AuthSource::Environment),
            label: Some(name.into()),
        }
    );
}

#[test]
fn provider_auth_status_reports_non_env_api_key_values_as_a_config_key() {
    let (_dir, registry) = registry_with_api_key("literal_api_key_value");
    let auth = AuthStorage::in_memory([]);
    assert_eq!(
        provider_auth_status(&auth, &registry, "custom-provider"),
        AuthStatus {
            configured: true,
            source: Some(AuthSource::ModelsJsonKey),
            label: None,
        }
    );
}

#[test]
fn provider_auth_status_reports_command_api_keys_without_executing_them() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("status-counter");
    std::fs::write(&counter, "0").unwrap();
    let command = format!(
        "!sh -c 'echo 1 > \"{}\"; echo key-value'",
        counter.display()
    );
    let (_models, registry) = registry_with_api_key(&command);
    let auth = AuthStorage::in_memory([]);
    assert_eq!(
        provider_auth_status(&auth, &registry, "custom-provider"),
        AuthStatus {
            configured: true,
            source: Some(AuthSource::ModelsJsonCommand),
            label: None,
        }
    );
    assert_eq!(std::fs::read_to_string(&counter).unwrap(), "0");
}

#[test]
fn a_stored_credential_wins_over_models_json() {
    let (_dir, registry) = registry_with_api_key("literal_api_key_value");
    let auth = AuthStorage::in_memory([(
        "custom-provider".to_string(),
        cortexcode_code_auth::AuthCredential::ApiKey { key: "k".into() },
    )]);
    let status = provider_auth_status(&auth, &registry, "custom-provider");
    assert_eq!(status.source, Some(AuthSource::Stored));
}
