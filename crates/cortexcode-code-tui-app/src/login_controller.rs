//! The `/login` and `/logout` flows' logic, hoocode
//! `modes/interactive/login-controller.ts`; the mode hosts the panes.
//!
//! Adaptations: the original awaits each step inside one async method; here
//! the mode drives a small state machine from pane events, and an OAuth login
//! runs on the async runtime, asking the mode for input through
//! [`LoginUpdate`]s ([`OAuthBridge`]). The model registry is shared without
//! interior mutability, so `modelRegistry.refresh()` after a login is not
//! repeated: availability already reads the credential store live.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cortexcode_ai_oauth::{
    BoxFuture, OAuthAuthInfo, OAuthLoginCallbacks, OAuthPrompt, OAuthSelectPrompt,
};
use cortexcode_ai_types::{AbortSignal, Model};
use cortexcode_code_auth::provider_display_names::provider_display_name;
use cortexcode_code_auth::{AuthCredential, AuthStorage};
use cortexcode_code_models::{ModelRegistry, DEFAULT_MODEL_PER_PROVIDER};
use cortexcode_code_tui_selectors::config_selector::locale_compare;
use cortexcode_code_tui_selectors::oauth_selector::{
    is_api_key_login_provider, AuthSelectorProvider, AuthType,
};
use tokio::sync::oneshot;

/// The auth-type pane's options.
pub const SUBSCRIPTION_LABEL: &str = "Use a subscription";
pub const API_KEY_LABEL: &str = "Use an API key";

/// Why a login stopped without an error to show.
pub const LOGIN_CANCELLED: &str = "Login cancelled";

fn sort_by_name(options: &mut [AuthSelectorProvider]) {
    options.sort_by(|a, b| locale_compare(&a.name, &b.name));
}

/// `getLoginProviderOptions`: the subscription providers, then every model
/// provider that signs in with a key, filtered to `auth_type`, by name.
pub fn login_provider_options(
    auth: &AuthStorage,
    registry: &ModelRegistry,
    auth_type: Option<AuthType>,
) -> Vec<AuthSelectorProvider> {
    let oauth = auth.get_oauth_providers();
    let oauth_ids: HashSet<String> = oauth.iter().map(|p| p.id().to_string()).collect();
    let mut options: Vec<AuthSelectorProvider> = oauth
        .iter()
        .map(|p| AuthSelectorProvider {
            id: p.id().to_string(),
            name: p.name().to_string(),
            auth_type: AuthType::OAuth,
        })
        .collect();
    let mut seen = HashSet::new();
    for model in registry.get_all() {
        let provider = model.provider.to_string();
        if !seen.insert(provider.clone()) || !is_api_key_login_provider(&provider, &oauth_ids, None)
        {
            continue;
        }
        options.push(AuthSelectorProvider {
            name: provider_display_name(auth, &provider),
            id: provider,
            auth_type: AuthType::ApiKey,
        });
    }
    if let Some(auth_type) = auth_type {
        options.retain(|o| o.auth_type == auth_type);
    }
    sort_by_name(&mut options);
    options
}

/// `getLogoutProviderOptions`: every stored credential, by name.
pub fn logout_provider_options(auth: &AuthStorage) -> Vec<AuthSelectorProvider> {
    let mut options: Vec<AuthSelectorProvider> = auth
        .list()
        .into_iter()
        .filter_map(|id| {
            let auth_type = match auth.get(&id)? {
                AuthCredential::OAuth(_) => AuthType::OAuth,
                AuthCredential::ApiKey { .. } => AuthType::ApiKey,
            };
            Some(AuthSelectorProvider {
                name: provider_display_name(auth, &id),
                id,
                auth_type,
            })
        })
        .collect();
    sort_by_name(&mut options);
    options
}

/// The status `/login <type>` shows when nothing can sign in that way.
pub fn no_providers_message(auth_type: AuthType) -> &'static str {
    match auth_type {
        AuthType::OAuth => "No subscription providers available.",
        AuthType::ApiKey => "No API key providers available.",
    }
}

/// `/logout` with nothing stored.
pub const NOTHING_TO_LOG_OUT: &str = "No stored credentials to remove. /logout only removes credentials saved by /login; environment variables and models.json config are unchanged.";

/// What `/logout` says once a credential is gone.
pub fn logged_out_message(provider: &AuthSelectorProvider) -> String {
    match provider.auth_type {
        AuthType::OAuth => format!("Logged out of {}", provider.name),
        AuthType::ApiKey => format!(
            "Removed stored API key for {}. Environment variables and models.json config are unchanged.",
            provider.name
        ),
    }
}

/// `completeProviderAuthentication`'s choice of model: with no model yet,
/// the provider's default when it is available.
pub enum PostLoginModel {
    /// A model was already selected; nothing to pick.
    Keep,
    Select(Box<Model>),
    /// Why nothing was picked (shown as an error).
    Error(String),
}

/// The record's opening words.
pub fn action_label(provider_name: &str, auth_type: AuthType) -> String {
    match auth_type {
        AuthType::OAuth => format!("Logged in to {provider_name}"),
        AuthType::ApiKey => format!("Saved API key for {provider_name}"),
    }
}

/// Which model to select after signing in to `provider_id`, when
/// `had_model` is false. `available` is the models with auth now.
pub fn post_login_model(
    had_model: bool,
    provider_id: &str,
    action_label: &str,
    available: &[Model],
) -> PostLoginModel {
    if had_model {
        return PostLoginModel::Keep;
    }
    let Some((_, default_id)) = DEFAULT_MODEL_PER_PROVIDER
        .iter()
        .find(|(p, _)| *p == provider_id)
    else {
        return PostLoginModel::Error(format!(
            "{action_label}, but no default model is configured for provider \"{provider_id}\". Use /model to select a model."
        ));
    };
    let provider_models: Vec<&Model> = available
        .iter()
        .filter(|m| m.provider == provider_id)
        .collect();
    if provider_models.is_empty() {
        return PostLoginModel::Error(format!(
            "{action_label}, but no models are available for that provider. Use /model to select a model."
        ));
    }
    match provider_models.into_iter().find(|m| m.id == *default_id) {
        Some(model) => PostLoginModel::Select(Box::new(model.clone())),
        None => PostLoginModel::Error(format!(
            "{action_label}, but its default model \"{default_id}\" is not available. Use /model to select a model."
        )),
    }
}

/// What an OAuth login asks of the mode.
pub enum LoginUpdate {
    /// `onAuth`: show the URL; with `manual`, also take a pasted redirect
    /// URL while the callback server waits.
    Auth {
        info: OAuthAuthInfo,
        manual: Option<oneshot::Sender<String>>,
    },
    /// `onPrompt`.
    Prompt {
        prompt: OAuthPrompt,
        reply: oneshot::Sender<String>,
    },
    /// `onProgress`.
    Progress(String),
    /// `onSelect`: the chosen option id, `None` on cancel.
    Select {
        prompt: OAuthSelectPrompt,
        reply: oneshot::Sender<Option<String>>,
    },
    /// The flow ended: saved, or why not.
    Done(Result<(), String>),
}

/// `OAuthLoginCallbacks` that route every step to the mode. A dropped reply
/// sender reads as the user cancelling.
pub struct OAuthBridge {
    send: Box<dyn Fn(LoginUpdate) + Send + Sync>,
    signal: AbortSignal,
    manual_tx: Mutex<Option<oneshot::Sender<String>>>,
    manual_rx: Mutex<Option<oneshot::Receiver<String>>>,
}

impl OAuthBridge {
    /// `uses_callback_server`: the provider races a pasted redirect URL
    /// against its callback server.
    pub fn new(
        send: Box<dyn Fn(LoginUpdate) + Send + Sync>,
        signal: AbortSignal,
        uses_callback_server: bool,
    ) -> Self {
        let (tx, rx) = if uses_callback_server {
            let (tx, rx) = oneshot::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        Self {
            send,
            signal,
            manual_tx: Mutex::new(tx),
            manual_rx: Mutex::new(rx),
        }
    }
}

async fn answer<T>(rx: oneshot::Receiver<T>) -> Result<T, String> {
    rx.await.map_err(|_| LOGIN_CANCELLED.to_string())
}

impl OAuthLoginCallbacks for OAuthBridge {
    fn on_auth(&self, info: OAuthAuthInfo) {
        let manual = self
            .manual_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        (self.send)(LoginUpdate::Auth { info, manual });
    }

    fn on_prompt(&self, prompt: OAuthPrompt) -> BoxFuture<'_, Result<String, String>> {
        let (reply, rx) = oneshot::channel();
        (self.send)(LoginUpdate::Prompt { prompt, reply });
        Box::pin(answer(rx))
    }

    fn on_progress(&self, message: &str) {
        (self.send)(LoginUpdate::Progress(message.to_string()));
    }

    fn on_manual_code_input(&self) -> Option<BoxFuture<'_, Result<String, String>>> {
        let rx = self
            .manual_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        Some(Box::pin(answer(rx)))
    }

    fn on_select(&self, prompt: OAuthSelectPrompt) -> BoxFuture<'_, Option<String>> {
        let (reply, rx) = oneshot::channel();
        (self.send)(LoginUpdate::Select { prompt, reply });
        Box::pin(async move { rx.await.ok().flatten() })
    }

    fn signal(&self) -> Option<AbortSignal> {
        Some(self.signal.clone())
    }
}

/// `authStorage.login`: run the provider's flow and store what it returns.
pub async fn run_oauth_login(
    auth: Arc<AuthStorage>,
    provider_id: String,
    bridge: OAuthBridge,
) -> Result<(), String> {
    let provider = auth
        .get_oauth_providers()
        .into_iter()
        .find(|p| p.id() == provider_id)
        .ok_or_else(|| format!("Unknown OAuth provider: {provider_id}"))?;
    let credentials = provider.login(&bridge).await?;
    auth.set(&provider_id, AuthCredential::OAuth(credentials));
    Ok(())
}

/// `isOpenableUrl`: only schemes a click may hand to the desktop.
pub fn is_openable_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    let valid_scheme = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid_scheme {
        return false;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => rest
            .strip_prefix("//")
            .is_some_and(|r| !r.is_empty() && !r.starts_with('/')),
        "mailto" => true,
        _ => false,
    }
}

/// `openUrl`: the desktop's default handler, no shell, detached; failures
/// are silent (the URL is on screen either way).
pub fn open_url(url: &str) {
    if !is_openable_url(url) {
        return;
    }
    #[cfg(target_os = "macos")]
    let (command, args) = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let (command, args) = ("cmd", vec!["/c", "start", "", url]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let (command, args) = ("xdg-open", vec![url]);
    let spawned = std::process::Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    // Reap it off the UI thread so it does not linger as a zombie.
    if let Ok(mut child) = spawned {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}
