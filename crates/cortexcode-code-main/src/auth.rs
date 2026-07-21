//! Interactive OAuth login wiring for the `cortex` CLI.
//!
//! The pure OAuth request/response logic lives in `cortexcode-ai-oauth`; that
//! crate deliberately leaves the interactive concerns — opening a browser,
//! running a local HTTP callback server, prompting the user, and persisting
//! the resulting tokens — to the CLI layer. This module implements exactly
//! those pieces:
//!
//! * [`open_browser`] — best-effort platform browser launcher.
//! * [`run_callback_server`] — a tiny single-request `TcpListener` server that
//!   captures the `code`/`state` from Anthropic's OAuth redirect.
//! * [`CredentialStore`] — reads/writes `~/.cortexcode/auth.json`.
//! * [`login`] — the top-level driver dispatched from `--login <provider>`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cortexcode_ai_oauth::{anthropic, github_copilot, pkce, OAuthCredentials};

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
                "unknown login provider: {} (expected 'anthropic' or 'github-copilot')",
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

/// Parse the `code`/`state` query parameters out of an HTTP request target
/// such as `/callback?code=abc&state=xyz`.
fn parse_callback_target(target: &str) -> (Option<String>, Option<String>) {
    let Some((_, query)) = target.split_once('?') else {
        return (None, None);
    };
    let mut code = None;
    let mut state = None;
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            _ => {}
        }
    }
    (code, state)
}

fn write_callback_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// The `code`/`state` pair captured from an OAuth redirect.
pub struct CallbackResult {
    pub code: String,
    pub state: Option<String>,
}

/// Run a single-request local HTTP server on `addr` and block until the
/// OAuth provider redirects the browser to it with an authorization `code`.
///
/// The listener accepts connections until one carries a `code` query
/// parameter (ignoring incidental requests such as `/favicon.ico`), replies
/// with a small confirmation page, and returns the captured values.
pub fn run_callback_server(addr: &str, timeout: Duration) -> Result<CallbackResult, AuthError> {
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(false)?;

    let deadline = std::time::Instant::now() + timeout;
    // Use a background-friendly accept loop with a per-accept timeout so the
    // overall wait is bounded even if no redirect ever arrives.
    listener.set_nonblocking(true)?;

    loop {
        if std::time::Instant::now() >= deadline {
            return Err(AuthError::Flow(
                "timed out waiting for the OAuth redirect".to_string(),
            ));
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let target = read_request_target(&stream);
                let (code, state) = target
                    .as_deref()
                    .map(parse_callback_target)
                    .unwrap_or((None, None));

                if let Some(code) = code {
                    write_callback_response(
                        &mut stream,
                        "<html><body style=\"font-family:sans-serif\"><h2>Login complete</h2>\
                         <p>You can close this tab and return to the terminal.</p></body></html>",
                    );
                    return Ok(CallbackResult { code, state });
                }

                // Not the redirect we're waiting for (e.g. favicon); keep listening.
                write_callback_response(
                    &mut stream,
                    "<html><body>Waiting for authorization…</body></html>",
                );
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(AuthError::Io(e)),
        }
    }
}

/// Read the request target (the path+query) from the first line of an HTTP
/// request: `GET /callback?code=... HTTP/1.1`.
fn read_request_target(stream: &TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    // Read only the first line; that's all we need for the target.
    if reader.read_line(&mut request_line).ok()? == 0 {
        return None;
    }
    // Drain the rest of the headers so the client sees a clean response, but
    // cap the work to avoid unbounded reads.
    let mut sink = [0u8; 1024];
    let _ = reader.get_mut().read(&mut sink);
    let mut parts = request_line.split_whitespace();
    let _method = parts.next()?;
    parts.next().map(str::to_string)
}

/// Drive the Anthropic OAuth flow end to end: build the authorize URL, open
/// the browser, run the local callback server, exchange the code for tokens,
/// and persist them.
fn login_anthropic(
    store: &CredentialStore,
    output: &mut dyn Write,
) -> Result<OAuthCredentials, AuthError> {
    let pkce = pkce::generate_pkce();
    let redirect_uri = anthropic::default_redirect_uri();
    let authorize_url = anthropic::build_authorize_url(&pkce, &redirect_uri);

    // The callback host/port must match `redirect_uri`.
    let addr = redirect_uri
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("localhost:53692")
        .replace("localhost", "127.0.0.1");

    writeln!(output, "Opening your browser to sign in with Anthropic…")?;
    writeln!(
        output,
        "If it doesn't open, visit this URL manually:\n\n  {authorize_url}\n"
    )?;
    let _ = open_browser(&authorize_url);

    let callback = run_callback_server(&addr, Duration::from_secs(300))?;
    let state = callback.state.unwrap_or_else(|| pkce.verifier.clone());

    let credentials = anthropic::exchange_authorization_code(
        &callback.code,
        &state,
        &pkce.verifier,
        &redirect_uri,
    )
    .map_err(AuthError::Flow)?;

    store.save("anthropic", &credentials)?;
    writeln!(
        output,
        "\nLogged in to Anthropic. Credentials saved to {}.",
        store.path().display()
    )?;
    Ok(credentials)
}

/// Drive the GitHub Copilot device flow end to end: start the device flow,
/// show the user the verification URL + code, poll for completion, exchange
/// for a Copilot token, and persist it.
fn login_github_copilot(
    store: &CredentialStore,
    output: &mut dyn Write,
) -> Result<OAuthCredentials, AuthError> {
    let domain = "github.com";
    let (auth, device) = github_copilot::start_login(domain).map_err(AuthError::Flow)?;

    writeln!(
        output,
        "To sign in with GitHub Copilot, open:\n\n  {}\n\nand enter the code: {}\n",
        auth.verification_uri, auth.user_code
    )?;
    let _ = open_browser(&auth.verification_uri);
    writeln!(output, "Waiting for you to authorize…")?;

    let credentials =
        github_copilot::complete_login(domain, &device, None).map_err(AuthError::Flow)?;

    store.save("github-copilot", &credentials)?;
    writeln!(
        output,
        "\nLogged in to GitHub Copilot. Credentials saved to {}.",
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
            login_anthropic(&store, output)?;
            Ok(())
        }
        "github-copilot" | "github" | "copilot" => {
            login_github_copilot(&store, output)?;
            Ok(())
        }
        other => Err(AuthError::UnknownProvider(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_callback_target() {
        let (code, state) = parse_callback_target("/callback?code=abc123&state=xyz789");
        assert_eq!(code, Some("abc123".to_string()));
        assert_eq!(state, Some("xyz789".to_string()));
    }

    #[test]
    fn test_parse_callback_target_no_query() {
        let (code, state) = parse_callback_target("/callback");
        assert_eq!(code, None);
        assert_eq!(state, None);
    }

    #[test]
    fn test_parse_callback_target_url_encoded() {
        let (code, _) = parse_callback_target("/callback?code=a%2Bb%2Fc");
        assert_eq!(code, Some("a+b/c".to_string()));
    }

    #[test]
    fn test_credential_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cortex-auth-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = CredentialStore::at(dir.join("auth.json"));

        assert!(store.get("anthropic").is_none());

        let creds = OAuthCredentials {
            refresh: "r".into(),
            access: "a".into(),
            expires: 123,
            extra: HashMap::new(),
        };
        store.save("anthropic", &creds).unwrap();
        assert_eq!(store.get("anthropic"), Some(creds.clone()));

        // A second provider merges rather than overwriting the file.
        let other = OAuthCredentials {
            refresh: "r2".into(),
            access: "a2".into(),
            expires: 456,
            extra: HashMap::new(),
        };
        store.save("github-copilot", &other).unwrap();
        assert_eq!(store.get("anthropic"), Some(creds));
        assert_eq!(store.get("github-copilot"), Some(other));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_credential_store_missing_file_is_empty() {
        let store = CredentialStore::at("/nonexistent/path/auth.json");
        assert!(store.load_all().is_empty());
        assert!(store.get("anthropic").is_none());
    }

    #[test]
    fn test_run_callback_server_captures_code() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener); // free the port; the server rebinds it below

        let server_addr = addr.to_string();
        let handle =
            std::thread::spawn(move || run_callback_server(&server_addr, Duration::from_secs(5)));

        // Give the server a moment to bind, then send a redirect request.
        std::thread::sleep(Duration::from_millis(200));
        let mut client = TcpStream::connect(addr).unwrap();
        client
            .write_all(
                b"GET /callback?code=abc123&state=xyz789 HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .unwrap();
        let mut response = String::new();
        let _ = client.read_to_string(&mut response);

        let result = handle.join().unwrap().unwrap();
        assert_eq!(result.code, "abc123");
        assert_eq!(result.state, Some("xyz789".to_string()));
        assert!(response.contains("Login complete"));
    }

    #[test]
    fn test_login_unknown_provider() {
        let mut out = Vec::new();
        let err = login("nope", &mut out).unwrap_err();
        assert!(matches!(err, AuthError::UnknownProvider(_)));
    }
}
