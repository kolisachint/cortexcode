//! The provider registry of `utils/oauth/index.ts`. The built-in providers
//! live in their own crates, so whoever assembles them installs them once
//! with [`install_builtin_oauth_providers`] (hoocode lists them statically).

use std::sync::{Arc, OnceLock, RwLock};

use crate::types::{OAuthCredentials, OAuthProvider};

type Provider = Arc<dyn OAuthProvider>;

fn builtins() -> &'static RwLock<Vec<Provider>> {
    static BUILTINS: OnceLock<RwLock<Vec<Provider>>> = OnceLock::new();
    BUILTINS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Registered providers in registration order (a JS `Map`).
fn registry() -> &'static RwLock<Vec<Provider>> {
    static REGISTRY: OnceLock<RwLock<Vec<Provider>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

fn set(list: &mut Vec<Provider>, provider: Provider) {
    match list.iter_mut().find(|p| p.id() == provider.id()) {
        Some(slot) => *slot = provider,
        None => list.push(provider),
    }
}

/// Set the built-in providers (`BUILT_IN_OAUTH_PROVIDERS`) and register them.
pub fn install_builtin_oauth_providers(providers: Vec<Provider>) {
    *builtins().write().unwrap() = providers.clone();
    let mut registry = registry().write().unwrap();
    for provider in providers {
        set(&mut registry, provider);
    }
}

/// `getOAuthProvider`.
pub fn get_oauth_provider(id: &str) -> Option<Provider> {
    registry()
        .read()
        .unwrap()
        .iter()
        .find(|p| p.id() == id)
        .cloned()
}

/// `registerOAuthProvider`.
pub fn register_oauth_provider(provider: Provider) {
    set(&mut registry().write().unwrap(), provider);
}

/// `unregisterOAuthProvider`: a built-in id gets its built-in back.
pub fn unregister_oauth_provider(id: &str) {
    let builtin = builtins()
        .read()
        .unwrap()
        .iter()
        .find(|p| p.id() == id)
        .cloned();
    let mut registry = registry().write().unwrap();
    match builtin {
        Some(provider) => set(&mut registry, provider),
        None => registry.retain(|p| p.id() != id),
    }
}

/// `resetOAuthProviders`.
pub fn reset_oauth_providers() {
    *registry().write().unwrap() = builtins().read().unwrap().clone();
}

/// `getOAuthProviders`.
pub fn get_oauth_providers() -> Vec<Provider> {
    registry().read().unwrap().clone()
}

/// `refreshOAuthToken`.
pub async fn refresh_oauth_token(
    provider_id: &str,
    credentials: &OAuthCredentials,
) -> Result<OAuthCredentials, String> {
    let provider = get_oauth_provider(provider_id)
        .ok_or_else(|| format!("Unknown OAuth provider: {provider_id}"))?;
    provider.refresh_token(credentials).await
}

/// `getOAuthApiKey`: refresh when expired; `Ok(None)` without credentials.
pub async fn get_oauth_api_key(
    provider_id: &str,
    credentials: &std::collections::HashMap<String, OAuthCredentials>,
    now_millis: i64,
) -> Result<Option<(OAuthCredentials, String)>, String> {
    let provider = get_oauth_provider(provider_id)
        .ok_or_else(|| format!("Unknown OAuth provider: {provider_id}"))?;
    let Some(mut creds) = credentials.get(provider_id).cloned() else {
        return Ok(None);
    };
    if creds.is_expired(now_millis) {
        creds = provider
            .refresh_token(&creds)
            .await
            .map_err(|_| format!("Failed to refresh OAuth token for {provider_id}"))?;
    }
    let api_key = provider.get_api_key(&creds);
    Ok(Some((creds, api_key)))
}
