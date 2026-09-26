//! Port of hoocode `utils/oauth/types.ts` (v0.5.89).

use std::future::Future;
use std::pin::Pin;

use cortexcode_ai_types::{AbortSignal, Model};
use serde::{Deserialize, Serialize};

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// `OAuthCredentials`: `{refresh, access, expires, [key]: unknown}`. Extra
/// provider fields (e.g. Copilot's `enterpriseUrl`) sit next to the three
/// known ones, as in hoocode's `auth.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthCredentials {
    pub refresh: String,
    pub access: String,
    /// Unix milliseconds after which `access` is treated as expired (the
    /// providers already subtract a 5 minute margin).
    pub expires: i64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl OAuthCredentials {
    /// Credentials without extra fields.
    pub fn new(refresh: impl Into<String>, access: impl Into<String>, expires: i64) -> Self {
        Self {
            refresh: refresh.into(),
            access: access.into(),
            expires,
            extra: serde_json::Map::new(),
        }
    }

    /// `Date.now() >= expires`.
    pub fn is_expired(&self, now_millis: i64) -> bool {
        now_millis >= self.expires
    }

    /// An extra string field.
    pub fn extra_str(&self, key: &str) -> Option<&str> {
        self.extra.get(key).and_then(|v| v.as_str())
    }
}

/// `OAuthPrompt`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OAuthPrompt {
    pub message: String,
    pub placeholder: Option<String>,
    pub allow_empty: bool,
}

/// `OAuthAuthInfo`: where the user authenticates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OAuthAuthInfo {
    pub url: String,
    pub instructions: Option<String>,
}

/// `OAuthSelectOption`.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthSelectOption {
    pub id: String,
    pub label: String,
}

/// `OAuthSelectPrompt`.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthSelectPrompt {
    pub message: String,
    pub options: Vec<OAuthSelectOption>,
}

/// `OAuthLoginCallbacks`: how a login flow talks to the user.
pub trait OAuthLoginCallbacks: Send + Sync {
    /// `onAuth`: show the URL (and instructions) to open.
    fn on_auth(&self, info: OAuthAuthInfo);

    /// `onPrompt`: ask for a line of input.
    fn on_prompt(&self, prompt: OAuthPrompt) -> BoxFuture<'_, Result<String, String>>;

    /// `onProgress`.
    fn on_progress(&self, _message: &str) {}

    /// `onManualCodeInput`: `None` when the caller cannot take a pasted
    /// code while the callback server waits.
    fn on_manual_code_input(&self) -> Option<BoxFuture<'_, Result<String, String>>> {
        None
    }

    /// `onSelect`: the chosen option id, `None` on cancel.
    fn on_select(&self, _prompt: OAuthSelectPrompt) -> BoxFuture<'_, Option<String>> {
        Box::pin(async { None })
    }

    /// `signal`.
    fn signal(&self) -> Option<AbortSignal> {
        None
    }
}

/// `OAuthProviderInterface`.
pub trait OAuthProvider: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;

    /// `usesCallbackServer`.
    fn uses_callback_server(&self) -> bool {
        false
    }

    /// Run the login flow; the credentials are for the caller to persist.
    fn login<'a>(
        &'a self,
        callbacks: &'a dyn OAuthLoginCallbacks,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>>;

    /// Refresh expired credentials.
    fn refresh_token<'a>(
        &'a self,
        credentials: &'a OAuthCredentials,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>>;

    /// The API key the provider's requests use.
    fn get_api_key(&self, credentials: &OAuthCredentials) -> String;

    /// `modifyModels` (e.g. the Copilot base URL from the token).
    fn modify_models(&self, models: Vec<Model>, _credentials: &OAuthCredentials) -> Vec<Model> {
        models
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn credentials_serialize_flat_like_auth_json() {
        let mut creds = OAuthCredentials::new("r", "a", 5);
        creds
            .extra
            .insert("enterpriseUrl".into(), json!("company.ghe.com"));
        let value = serde_json::to_value(&creds).unwrap();
        assert_eq!(
            value,
            json!({"refresh": "r", "access": "a", "expires": 5, "enterpriseUrl": "company.ghe.com"})
        );
        // hoocode's auth.json entries carry `type: "oauth"`; it round-trips.
        let stored = json!({"type": "oauth", "refresh": "r", "access": "a", "expires": 5});
        let parsed: OAuthCredentials = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(parsed.extra_str("type"), Some("oauth"));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), stored);
        assert!(parsed.is_expired(5) && !parsed.is_expired(4));
    }
}
