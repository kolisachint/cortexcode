//! Interactive OAuth login wiring for the `cortex` CLI.
//!
//! The flows (callback server, device code, token exchange) live in
//! `cortexcode-ai-oauth-anthropic` / `-github-copilot` / `-openai-codex`; this
//! module supplies
//! the terminal side of their `OAuthLoginCallbacks`, opens the browser and
//! persists the credentials:
//!
//! * [`open_browser`] — best-effort platform browser launcher.
//! * [`CredentialStore`] — reads/writes `~/.cortexcode/auth.json`.
//! * [`login`] — the top-level driver. hoocode has no `--login` flag (the pinned
//!   flag set is exact); the `/login` selector (ledger 11.3) will call this.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cortexcode_ai_oauth::{
    BoxFuture, OAuthAuthInfo, OAuthCredentials, OAuthLoginCallbacks, OAuthPrompt, OAuthProvider,
};

/// Error type for interactive login operations.
#[derive(Debug)]
pub enum AuthError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Flow(String),
    UnknownProvider(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Io(e) => write!(f, "io error: {}", e),
            AuthError::Json(e) => write!(f, "json error: {}", e),
            AuthError::Flow(e) => write!(f, "login failed: {}", e),
            AuthError::UnknownProvider(p) => write!(
                f,
                "unknown login provider: {} (expected 'anthropic', 'github-copilot' or 'openai-codex')",
                p
            ),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<std::io::Error> for AuthError {
    fn from(e: std::io::Error) -> Self {
        AuthError::Io(e)
    }
}

impl From<serde_json::Error> for AuthError {
    fn from(e: serde_json::Error) -> Self {
        AuthError::Json(e)
    }
}

/// Persistent store for OAuth credentials, keyed by provider id.
///
/// Backed by `~/.cortexcode/auth.json` — a JSON object mapping a provider id
/// (`anthropic`, `github-copilot`) to its [`OAuthCredentials`].
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    /// Create a store backed by the default `~/.cortexcode/auth.json` path.
    pub fn default_location() -> Self {
        Self {
            path: cortexcode_code_config::default_config_dir().join("auth.json"),
        }
    }

    /// Create a store backed by an explicit path (used in tests).
    #[cfg(test)]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file backing this store.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load all persisted credentials. A missing or malformed file yields an
    /// empty map rather than an error, matching the config crate's behavior.
    pub fn load_all(&self) -> HashMap<String, OAuthCredentials> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return HashMap::new();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// Load the credentials for a single provider, if present.
    pub fn get(&self, provider: &str) -> Option<OAuthCredentials> {
        self.load_all().remove(provider)
    }

    /// Persist credentials for a provider, merging into any existing file.
    pub fn save(&self, provider: &str, credentials: &OAuthCredentials) -> Result<(), AuthError> {
        let mut all = self.load_all();
        all.insert(provider.to_string(), credentials.clone());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&all)?;
        std::fs::write(&self.path, text)?;
        Ok(())
    }
}

/// Best-effort launch of the user's default browser at `url`.
///
/// Returns `Ok(())` if a launcher was spawned; the caller should always also
/// print the URL so the user can open it manually when this fails or when
/// running headless.
pub fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };

    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Terminal callbacks: lines go to the caller's output through a channel
/// (the flow runs on the async runtime), prompts read a line from stdin.
struct CliCallbacks {
    lines: std::sync::mpsc::Sender<String>,
}

impl OAuthLoginCallbacks for CliCallbacks {
    fn on_auth(&self, info: OAuthAuthInfo) {
        let mut text = format!("Open this URL to sign in:\n\n  {}\n", info.url);
        if let Some(instructions) = &info.instructions {
            text.push_str(&format!("\n{instructions}\n"));
        }
        let _ = self.lines.send(text);
        let _ = open_browser(&info.url);
    }

    fn on_prompt(&self, prompt: OAuthPrompt) -> BoxFuture<'_, Result<String, String>> {
        let _ = self.lines.send(format!("{} ", prompt.message));
        Box::pin(async move {
            tokio::task::spawn_blocking(|| {
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).map(|_| line)
            })
            .await
            .map_err(|e| e.to_string())?
            .map(|line| line.trim_end_matches(['\r', '\n']).to_string())
            .map_err(|e| e.to_string())
        })
    }

    fn on_progress(&self, message: &str) {
        let _ = self.lines.send(message.to_string());
    }
}

/// Run `provider`'s login flow, streaming its messages to `output`.
fn run_login(
    provider: Arc<dyn OAuthProvider>,
    output: &mut dyn Write,
) -> Result<OAuthCredentials, AuthError> {
    let (lines, received) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    crate::runtime::async_runtime().spawn(async move {
        let callbacks = CliCallbacks { lines };
        let result = provider.login(&callbacks).await;
        let _ = done_tx.send(result);
    });
    // Forward messages until the flow finishes (its sender drops with it).
    for line in received {
        writeln!(output, "{line}")?;
        output.flush()?;
    }
    done_rx
        .recv()
        .map_err(|_| AuthError::Flow("login task ended unexpectedly".into()))?
        .map_err(AuthError::Flow)
}

/// Log in with `provider` and persist the credentials under `store_key`.
fn login_with(
    store: &CredentialStore,
    store_key: &str,
    label: &str,
    provider: Arc<dyn OAuthProvider>,
    output: &mut dyn Write,
) -> Result<OAuthCredentials, AuthError> {
    let credentials = run_login(provider, output)?;
    store.save(store_key, &credentials)?;
    writeln!(
        output,
        "\nLogged in to {label}. Credentials saved to {}.",
        store.path().display()
    )?;
    Ok(credentials)
}

/// Run the interactive login for `provider`, persisting the resulting
/// credentials to the default credential store.
pub fn login(provider: &str, output: &mut dyn Write) -> Result<(), AuthError> {
    let store = CredentialStore::default_location();
    match provider {
        "anthropic" | "claude" => {
            let provider =
                Arc::new(cortexcode_ai_oauth_anthropic::AnthropicOAuthProvider::default());
            login_with(&store, "anthropic", "Anthropic", provider, output)?;
            Ok(())
        }
        "github-copilot" | "github" | "copilot" => {
            let provider =
                Arc::new(cortexcode_ai_oauth_github_copilot::GitHubCopilotOAuthProvider::default());
            login_with(&store, "github-copilot", "GitHub Copilot", provider, output)?;
            Ok(())
        }
        "openai-codex" | "codex" | "chatgpt" => {
            let provider =
                Arc::new(cortexcode_ai_oauth_openai_codex::OpenAICodexOAuthProvider::default());
            login_with(
                &store,
                "openai-codex",
                "ChatGPT Plus/Pro (Codex Subscription)",
                provider,
                output,
            )?;
            Ok(())
        }
        other => Err(AuthError::UnknownProvider(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credential_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cortex-auth-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = CredentialStore::at(dir.join("auth.json"));

        assert!(store.get("anthropic").is_none());

        let creds = OAuthCredentials::new("r", "a", 123);
        store.save("anthropic", &creds).unwrap();
        assert_eq!(store.get("anthropic"), Some(creds.clone()));

        // A second provider merges rather than overwriting the file.
        let mut other = OAuthCredentials::new("r2", "a2", 456);
        other
            .extra
            .insert("enterpriseUrl".into(), serde_json::json!("company.ghe.com"));
        store.save("github-copilot", &other).unwrap();
        assert_eq!(store.get("anthropic"), Some(creds));
        assert_eq!(store.get("github-copilot"), Some(other));

        // hoocode's auth.json shape: flat entries.
        let text = std::fs::read_to_string(dir.join("auth.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["github-copilot"]["enterpriseUrl"], "company.ghe.com");
        assert!(json["anthropic"].get("extra").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_credential_store_missing_file_is_empty() {
        let store = CredentialStore::at("/nonexistent/path/auth.json");
        assert!(store.load_all().is_empty());
        assert!(store.get("anthropic").is_none());
    }

    #[test]
    fn test_login_unknown_provider() {
        let mut out = Vec::new();
        let err = login("nope", &mut out).unwrap_err();
        assert!(matches!(err, AuthError::UnknownProvider(_)));
    }
}
