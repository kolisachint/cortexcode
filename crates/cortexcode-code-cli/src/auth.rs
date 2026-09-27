//! Interactive OAuth login wiring for the `cortex` CLI.
//!
//! The flows (callback server, device code, token exchange) live in
//! `cortexcode-ai-oauth-anthropic` / `-github-copilot` / `-openai-codex`; this
//! module supplies
//! the terminal side of their `OAuthLoginCallbacks`, opens the browser and
//! persists the credentials:
//!
//! * [`open_browser`] — best-effort platform browser launcher.
//! * credentials are stored in `auth.json` through `cortexcode_code_auth::AuthStorage`.
//! * [`login`] — the top-level driver. hoocode has no `--login` flag (the pinned
//!   flag set is exact); the `/login` selector (ledger 11.3) will call this.

use std::io::Write;
use std::sync::Arc;

use cortexcode_ai_oauth::{
    BoxFuture, OAuthAuthInfo, OAuthCredentials, OAuthLoginCallbacks, OAuthPrompt, OAuthProvider,
};
use cortexcode_code_auth::{AuthCredential, AuthStorage};

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
                "unknown login provider: {} (expected 'anthropic', 'github-copilot', 'google-gemini-cli', 'google-antigravity' or 'openai-codex')",
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
    store: &AuthStorage,
    store_key: &str,
    label: &str,
    provider: Arc<dyn OAuthProvider>,
    output: &mut dyn Write,
) -> Result<OAuthCredentials, AuthError> {
    let credentials = run_login(provider, output)?;
    store.set(store_key, AuthCredential::OAuth(credentials.clone()));
    if let Some(error) = store.drain_errors().into_iter().next() {
        return Err(AuthError::Flow(format!(
            "could not save credentials: {error}"
        )));
    }
    writeln!(
        output,
        "\nLogged in to {label}. Credentials saved to {}.",
        cortexcode_code_paths::auth_path().display()
    )?;
    Ok(credentials)
}

/// Run the interactive login for `provider`, persisting the resulting
/// credentials to the default credential store.
pub fn login(provider: &str, output: &mut dyn Write) -> Result<(), AuthError> {
    let store = AuthStorage::create(None);
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
        "google-gemini-cli" | "gemini-cli" => {
            let provider = Arc::new(cortexcode_ai_oauth_google::GeminiCliOAuthProvider::default());
            login_with(
                &store,
                "google-gemini-cli",
                "Google Cloud Code Assist (Gemini CLI)",
                provider,
                output,
            )?;
            Ok(())
        }
        "google-antigravity" | "antigravity" => {
            let provider =
                Arc::new(cortexcode_ai_oauth_google::AntigravityOAuthProvider::default());
            login_with(
                &store,
                "google-antigravity",
                "Google Antigravity (Gemini, Claude, GPT-OSS)",
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
    fn test_login_unknown_provider() {
        let mut out = Vec::new();
        let err = login("nope", &mut out).unwrap_err();
        assert!(matches!(err, AuthError::UnknownProvider(_)));
    }
}
