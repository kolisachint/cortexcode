//! Credential storage for API keys and OAuth tokens (`auth.json`).
//!
//! Port of hoocode `packages/coding-agent/src/core/auth-storage.ts` (pinned v0.5.89).
//! Writes and token refreshes run under the backend lock, so several processes
//! (cortex or hoocode) refreshing the same expired token do it once.

pub mod auth_guidance;
mod backend;
pub mod provider_display_names;

pub use backend::{
    AsyncLockFn, AuthStorageBackend, FileAuthStorageBackend, InMemoryAuthStorageBackend, LockFn,
};

use cortexcode_ai_oauth::{OAuthCredentials, OAuthLoginCallbacks, OAuthProvider};
use cortexcode_ai_types::Model;
use cortexcode_code_models::{AuthLookup, ModelModifier};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

/// `AuthCredential`: one `auth.json` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthCredential {
    #[serde(rename = "api_key")]
    ApiKey { key: String },
    #[serde(rename = "oauth")]
    OAuth(OAuthCredentials),
}

/// `AuthStatus.source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSource {
    Stored,
    Runtime,
    Environment,
    Fallback,
    ModelsJsonKey,
    ModelsJsonCommand,
}

/// `AuthStatus`: whether a provider has auth, without exposing or refreshing it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthStatus {
    pub configured: bool,
    pub source: Option<AuthSource>,
    pub label: Option<String>,
}

/// `setFallbackResolver`: keys for custom providers (models.json).
pub type FallbackResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Default)]
struct State {
    /// Raw `auth.json` object; entries are parsed on read so unknown or
    /// malformed ones survive a rewrite untouched.
    data: Map<String, Value>,
    load_error: Option<String>,
    errors: Vec<String>,
}

/// `AuthStorage`.
pub struct AuthStorage {
    storage: Box<dyn AuthStorageBackend>,
    state: Mutex<State>,
    runtime_overrides: Mutex<HashMap<String, String>>,
    fallback_resolver: RwLock<Option<FallbackResolver>>,
}

fn parse_storage_data(content: Option<&str>) -> Result<Map<String, Value>, String> {
    match content {
        None | Some("") => Ok(Map::new()),
        Some(text) => match serde_json::from_str::<Value>(text).map_err(|e| e.to_string())? {
            Value::Object(map) => Ok(map),
            // `JSON.parse` of a non-object yields a value without own keys.
            _ => Ok(Map::new()),
        },
    }
}

/// `JSON.stringify(data, null, 2)`.
fn stringify(data: &Map<String, Value>) -> String {
    serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".into())
}

fn parse_credential(value: &Value) -> Option<AuthCredential> {
    if let Ok(credential) = serde_json::from_value(value.clone()) {
        return Some(credential);
    }
    // Earlier cortex builds stored OAuth entries without `"type": "oauth"`.
    match value {
        Value::Object(map) if !map.contains_key("type") => serde_json::from_value(value.clone())
            .ok()
            .map(AuthCredential::OAuth),
        _ => None,
    }
}

fn to_value(credential: &AuthCredential) -> Value {
    serde_json::to_value(credential).unwrap_or(Value::Null)
}

/// Runs `fut` to completion from sync code, inside or outside a tokio runtime
/// (on its own thread with a private runtime).
fn block_on<T: Send>(fut: impl std::future::Future<Output = T> + Send) -> T {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to start a tokio runtime")
                    .block_on(fut)
            })
            .join()
            .expect("auth refresh thread panicked")
    })
}

enum Lookup {
    Done(Option<String>),
    Refresh(Arc<dyn OAuthProvider>),
}

impl AuthStorage {
    fn new(storage: Box<dyn AuthStorageBackend>) -> Self {
        let this = Self {
            storage,
            state: Mutex::new(State::default()),
            runtime_overrides: Mutex::new(HashMap::new()),
            fallback_resolver: RwLock::new(None),
        };
        this.reload();
        this
    }

    /// `AuthStorage.create`: `auth.json` at `auth_path`, or in the agent dir.
    pub fn create(auth_path: Option<PathBuf>) -> Self {
        let path = auth_path.unwrap_or_else(cortexcode_code_paths::auth_path);
        Self::new(Box::new(FileAuthStorageBackend::new(path)))
    }

    /// `AuthStorage.fromStorage`.
    pub fn from_storage(storage: Box<dyn AuthStorageBackend>) -> Self {
        Self::new(storage)
    }

    /// `AuthStorage.inMemory`.
    pub fn in_memory(data: impl IntoIterator<Item = (String, AuthCredential)>) -> Self {
        let map: Map<String, Value> = data.into_iter().map(|(k, v)| (k, to_value(&v))).collect();
        let storage = InMemoryAuthStorageBackend::default();
        let _ = storage.with_lock(&mut |_| Ok(Some(stringify(&map))));
        Self::new(Box::new(storage))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn overrides(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.runtime_overrides
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn fallback(&self, provider: &str) -> Option<String> {
        let resolver = self
            .fallback_resolver
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        resolver.and_then(|r| r(provider))
    }

    /// `setRuntimeApiKey`: a key for this process only (`--api-key`).
    pub fn set_runtime_api_key(&self, provider: &str, api_key: &str) {
        self.overrides()
            .insert(provider.to_string(), api_key.to_string());
    }

    /// `removeRuntimeApiKey`.
    pub fn remove_runtime_api_key(&self, provider: &str) {
        self.overrides().remove(provider);
    }

    /// `setFallbackResolver`.
    pub fn set_fallback_resolver(&self, resolver: FallbackResolver) {
        *self
            .fallback_resolver
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(resolver);
    }

    fn record_error(&self, error: String) {
        self.state().errors.push(error);
    }

    /// `reload`: re-read the storage; a failure is kept as the load error, which
    /// stops later writes from clobbering a file that could not be read.
    pub fn reload(&self) {
        let mut content = None;
        let read = self.storage.with_lock(&mut |current| {
            content = current.map(str::to_string);
            Ok(None)
        });
        let parsed = read.and_then(|()| parse_storage_data(content.as_deref()));
        let mut state = self.state();
        match parsed {
            Ok(data) => {
                state.data = data;
                state.load_error = None;
            }
            Err(e) => {
                state.load_error = Some(e.clone());
                state.errors.push(e);
            }
        }
    }

    fn persist_provider_change(&self, provider: &str, credential: Option<&AuthCredential>) {
        if self.state().load_error.is_some() {
            return;
        }
        let result = self.storage.with_lock(&mut |current| {
            let mut merged = parse_storage_data(current)?;
            match credential {
                Some(c) => {
                    merged.insert(provider.to_string(), to_value(c));
                }
                None => {
                    merged.remove(provider);
                }
            }
            Ok(Some(stringify(&merged)))
        });
        if let Err(e) = result {
            self.record_error(e);
        }
    }

    /// `get`.
    pub fn get(&self, provider: &str) -> Option<AuthCredential> {
        self.state().data.get(provider).and_then(parse_credential)
    }

    /// `set`.
    pub fn set(&self, provider: &str, credential: AuthCredential) {
        self.state()
            .data
            .insert(provider.to_string(), to_value(&credential));
        self.persist_provider_change(provider, Some(&credential));
    }

    /// `remove`.
    pub fn remove(&self, provider: &str) {
        self.state().data.remove(provider);
        self.persist_provider_change(provider, None);
    }

    /// `list`: providers with stored credentials.
    pub fn list(&self) -> Vec<String> {
        self.state().data.keys().cloned().collect()
    }

    /// `has`: a stored credential exists.
    pub fn has(&self, provider: &str) -> bool {
        self.state().data.contains_key(provider)
    }

    /// `hasAuth`: any auth (runtime, stored, environment, fallback), no refresh.
    pub fn has_auth(&self, provider: &str) -> bool {
        self.overrides().contains_key(provider)
            || self
                .state()
                .data
                .get(provider)
                .is_some_and(|v| !v.is_null())
            || cortexcode_ai_env::get_env_api_key(provider).is_some()
            || self.fallback(provider).is_some()
    }

    /// `getAuthStatus`.
    pub fn get_auth_status(&self, provider: &str) -> AuthStatus {
        if self
            .state()
            .data
            .get(provider)
            .is_some_and(|v| !v.is_null())
        {
            return AuthStatus {
                configured: true,
                source: Some(AuthSource::Stored),
                label: None,
            };
        }
        if self.overrides().contains_key(provider) {
            return AuthStatus {
                configured: false,
                source: Some(AuthSource::Runtime),
                label: Some("--api-key".into()),
            };
        }
        if let Some(key) =
            cortexcode_ai_env::find_env_keys(provider).and_then(|k| k.into_iter().next())
        {
            return AuthStatus {
                configured: false,
                source: Some(AuthSource::Environment),
                label: Some(key),
            };
        }
        if self.fallback(provider).is_some() {
            return AuthStatus {
                configured: false,
                source: Some(AuthSource::Fallback),
                label: Some("custom provider config".into()),
            };
        }
        AuthStatus::default()
    }

    /// `getAll`: every parseable stored credential.
    pub fn get_all(&self) -> HashMap<String, AuthCredential> {
        self.state()
            .data
            .iter()
            .filter_map(|(k, v)| parse_credential(v).map(|c| (k.clone(), c)))
            .collect()
    }

    /// `drainErrors`.
    pub fn drain_errors(&self) -> Vec<String> {
        std::mem::take(&mut self.state().errors)
    }

    /// `login`: run the provider's OAuth flow and store the credentials.
    pub async fn login(
        &self,
        provider_id: &str,
        callbacks: &dyn OAuthLoginCallbacks,
    ) -> Result<(), String> {
        let provider = cortexcode_ai_oauth::get_oauth_provider(provider_id)
            .ok_or_else(|| format!("Unknown OAuth provider: {provider_id}"))?;
        let credentials = provider.login(callbacks).await?;
        self.set(provider_id, AuthCredential::OAuth(credentials));
        Ok(())
    }

    /// `logout`.
    pub fn logout(&self, provider: &str) {
        self.remove(provider);
    }

    /// `getOAuthProviders`.
    pub fn get_oauth_providers(&self) -> Vec<Arc<dyn OAuthProvider>> {
        cortexcode_ai_oauth::get_oauth_providers()
    }

    /// `refreshOAuthTokenWithLock`: re-read under the lock (another process may
    /// have refreshed already), refresh if still expired, persist the result.
    async fn refresh_oauth_token_with_lock(
        &self,
        provider: &Arc<dyn OAuthProvider>,
    ) -> Result<Option<String>, String> {
        let provider_id = provider.id().to_string();
        let result: Mutex<Option<String>> = Mutex::new(None);
        let result_ref = &result;
        self.storage
            .with_lock_async(Box::new(move |current| {
                Box::pin(async move {
                    let current_data = parse_storage_data(current.as_deref())?;
                    {
                        let mut state = self.state();
                        state.data = current_data.clone();
                        state.load_error = None;
                    }
                    let Some(AuthCredential::OAuth(cred)) =
                        current_data.get(&provider_id).and_then(parse_credential)
                    else {
                        return Ok(None);
                    };
                    let now = cortexcode_ai_oauth::now_ms();
                    if !cred.is_expired(now) {
                        *result_ref.lock().unwrap() = Some(provider.get_api_key(&cred));
                        return Ok(None);
                    }
                    let oauth_creds: HashMap<String, OAuthCredentials> = current_data
                        .iter()
                        .filter_map(|(k, v)| match parse_credential(v) {
                            Some(AuthCredential::OAuth(c)) => Some((k.clone(), c)),
                            _ => None,
                        })
                        .collect();
                    let Some((new_credentials, api_key)) =
                        cortexcode_ai_oauth::get_oauth_api_key(&provider_id, &oauth_creds, now)
                            .await?
                    else {
                        return Ok(None);
                    };
                    let mut merged = current_data;
                    merged.insert(
                        provider_id.clone(),
                        to_value(&AuthCredential::OAuth(new_credentials)),
                    );
                    {
                        let mut state = self.state();
                        state.data = merged.clone();
                        state.load_error = None;
                    }
                    *result_ref.lock().unwrap() = Some(api_key);
                    Ok(Some(stringify(&merged)))
                })
            }))
            .await?;
        Ok(result.into_inner().unwrap())
    }

    /// Everything in `getApiKey` that needs no refresh.
    fn lookup(&self, provider_id: &str, include_fallback: bool) -> Lookup {
        if let Some(key) = self.overrides().get(provider_id) {
            if !key.is_empty() {
                return Lookup::Done(Some(key.clone()));
            }
        }
        match self.get(provider_id) {
            Some(AuthCredential::ApiKey { key }) => {
                return Lookup::Done(cortexcode_code_models::resolve_config_value_cached(&key));
            }
            Some(AuthCredential::OAuth(cred)) => {
                let Some(provider) = cortexcode_ai_oauth::get_oauth_provider(provider_id) else {
                    return Lookup::Done(None);
                };
                if cred.is_expired(cortexcode_ai_oauth::now_ms()) {
                    return Lookup::Refresh(provider);
                }
                return Lookup::Done(Some(provider.get_api_key(&cred)));
            }
            None => {}
        }
        if let Some(key) = cortexcode_ai_env::get_env_api_key(provider_id) {
            return Lookup::Done(Some(key));
        }
        if include_fallback {
            return Lookup::Done(self.fallback(provider_id));
        }
        Lookup::Done(None)
    }

    /// `getApiKey`: runtime override, stored API key, stored OAuth token (refreshed
    /// under the lock when expired), environment, then the fallback resolver.
    pub async fn get_api_key(&self, provider_id: &str, include_fallback: bool) -> Option<String> {
        let provider = match self.lookup(provider_id, include_fallback) {
            Lookup::Done(key) => return key,
            Lookup::Refresh(provider) => provider,
        };
        match self.refresh_oauth_token_with_lock(&provider).await {
            // `null` from the locked refresh falls through to the environment.
            Ok(Some(key)) => Some(key),
            Ok(None) => {
                if let Some(key) = cortexcode_ai_env::get_env_api_key(provider_id) {
                    return Some(key);
                }
                include_fallback
                    .then(|| self.fallback(provider_id))
                    .flatten()
            }
            Err(error) => {
                self.record_error(error);
                // Another process may have refreshed successfully.
                self.reload();
                match self.get(provider_id) {
                    Some(AuthCredential::OAuth(cred))
                        if !cred.is_expired(cortexcode_ai_oauth::now_ms()) =>
                    {
                        Some(provider.get_api_key(&cred))
                    }
                    _ => None,
                }
            }
        }
    }

    /// [`Self::get_api_key`] from sync code (a refresh runs on its own thread).
    pub fn get_api_key_blocking(
        &self,
        provider_id: &str,
        include_fallback: bool,
    ) -> Option<String> {
        match self.lookup(provider_id, include_fallback) {
            Lookup::Done(key) => key,
            Lookup::Refresh(_) => block_on(self.get_api_key(provider_id, include_fallback)),
        }
    }

    /// The registry's OAuth `modifyModels` pass over the stored OAuth credentials.
    pub fn model_modifier(self: &Arc<Self>) -> ModelModifier {
        let this = Arc::clone(self);
        Arc::new(move |models: Vec<Model>| {
            let mut models = models;
            for provider in this.get_oauth_providers() {
                if let Some(AuthCredential::OAuth(cred)) = this.get(provider.id()) {
                    models = provider.modify_models(models, &cred);
                }
            }
            models
        })
    }
}

impl AuthLookup for AuthStorage {
    /// `getApiKey(provider, { includeFallback: false })`.
    fn api_key(&self, provider: &str) -> Option<String> {
        self.get_api_key_blocking(provider, false)
    }

    fn has_auth(&self, provider: &str) -> bool {
        AuthStorage::has_auth(self, provider)
    }

    fn is_oauth(&self, provider: &str) -> bool {
        matches!(self.get(provider), Some(AuthCredential::OAuth(_)))
    }
}
