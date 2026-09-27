//! The local OAuth callback server (`startCallbackServer` in the vendor
//! flows): serves `GET <path>?code=…&state=…`, answers every request with
//! the success or error page, and hands over the first valid code.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use crate::page::{oauth_error_html, oauth_success_html};

/// What the server needs to know.
#[derive(Debug, Clone)]
pub struct CallbackServerOptions {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub expected_state: String,
    /// The flow's name in the pages, e.g. `Anthropic`.
    pub label: String,
    pub validation: CallbackValidation,
}

/// How a flow validates the redirect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CallbackValidation {
    /// `error` → "did not complete", then both `code` and `state` required,
    /// then the state compared (anthropic.ts).
    #[default]
    CodeAndState,
    /// State compared first, then `code` required; `error` is not looked at
    /// (openai-codex.ts).
    StateThenCode,
    /// `error` → "did not complete", then both `code` and `state` required;
    /// the flow compares the state itself after the callback
    /// (google-gemini-cli.ts, google-antigravity.ts).
    CodeAndStateDeferred,
}

/// `{ code, state }` from the redirect.
#[derive(Debug, Clone, PartialEq)]
pub struct CallbackCode {
    pub code: String,
    pub state: String,
}

/// A running callback server; closed on drop (`server.close()`).
pub struct CallbackServer {
    code_rx: mpsc::Receiver<CallbackCode>,
    cancel_tx: Option<oneshot::Sender<()>>,
    cancel_rx: Option<oneshot::Receiver<()>>,
    task: tokio::task::JoinHandle<()>,
    port: u16,
}

/// Cancels [`CallbackServer::wait_for_code`] (`cancelWait`).
pub struct CancelWait(Option<oneshot::Sender<()>>);

impl CancelWait {
    pub fn cancel(mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

impl CallbackServer {
    /// Bind and start serving (`server.listen(port, host)`).
    pub async fn start(options: CallbackServerOptions) -> Result<Self, String> {
        let listener = TcpListener::bind((options.host.as_str(), options.port))
            .await
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let (code_tx, code_rx) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let head = read_head(&mut stream).await;
                let (status, content_type, body, code) = respond(&options, &head);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
                if let Some(code) = code {
                    let _ = code_tx.try_send(code);
                }
            }
        });
        let (cancel_tx, cancel_rx) = oneshot::channel();
        Ok(Self {
            code_rx,
            cancel_tx: Some(cancel_tx),
            cancel_rx: Some(cancel_rx),
            task,
            port,
        })
    }

    /// The port actually bound.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// A handle for `cancelWait` (take it once, before waiting).
    pub fn cancel_handle(&mut self) -> CancelWait {
        CancelWait(self.cancel_tx.take())
    }

    /// `waitForCode`: the first valid code, or `None` once cancelled.
    pub async fn wait_for_code(&mut self) -> Option<CallbackCode> {
        let cancel = self.cancel_rx.take();
        match cancel {
            Some(cancel) => tokio::select! {
                code = self.code_rx.recv() => code,
                _ = cancel => None,
            },
            None => None,
        }
    }
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_head(stream: &mut tokio::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    while !data.windows(4).any(|w| w == b"\r\n\r\n") && data.len() < 64 * 1024 {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

type Response = (&'static str, &'static str, String, Option<CallbackCode>);

fn respond(options: &CallbackServerOptions, head: &str) -> Response {
    const HTML: &str = "text/html; charset=utf-8";
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or("");
    let Ok(url) = url::Url::parse(&format!("http://localhost{target}")) else {
        return (
            "500 Internal Server Error",
            "text/plain; charset=utf-8",
            "Internal error".into(),
            None,
        );
    };
    if url.path() != options.path {
        return (
            "404 Not Found",
            HTML,
            oauth_error_html("Callback route not found.", None),
            None,
        );
    }
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    if options.validation == CallbackValidation::StateThenCode {
        if param("state").as_deref() != Some(options.expected_state.as_str()) {
            return (
                "400 Bad Request",
                HTML,
                oauth_error_html("State mismatch.", None),
                None,
            );
        }
        let Some(code) = param("code").filter(|c| !c.is_empty()) else {
            return (
                "400 Bad Request",
                HTML,
                oauth_error_html("Missing authorization code.", None),
                None,
            );
        };
        let message = format!(
            "{} authentication completed. You can close this window.",
            options.label
        );
        let state = options.expected_state.clone();
        return (
            "200 OK",
            HTML,
            oauth_success_html(&message),
            Some(CallbackCode { code, state }),
        );
    }
    if let Some(error) = param("error") {
        let message = format!("{} authentication did not complete.", options.label);
        return (
            "400 Bad Request",
            HTML,
            oauth_error_html(&message, Some(&format!("Error: {error}"))),
            None,
        );
    }
    let (Some(code), Some(state)) = (
        param("code").filter(|c| !c.is_empty()),
        param("state").filter(|s| !s.is_empty()),
    ) else {
        return (
            "400 Bad Request",
            HTML,
            oauth_error_html("Missing code or state parameter.", None),
            None,
        );
    };
    if options.validation == CallbackValidation::CodeAndState && state != options.expected_state {
        return (
            "400 Bad Request",
            HTML,
            oauth_error_html("State mismatch.", None),
            None,
        );
    }
    let message = format!(
        "{} authentication completed. You can close this window.",
        options.label
    );
    (
        "200 OK",
        HTML,
        oauth_success_html(&message),
        Some(CallbackCode { code, state }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> CallbackServerOptions {
        CallbackServerOptions {
            host: "127.0.0.1".into(),
            port: 0,
            path: "/callback".into(),
            expected_state: "s1".into(),
            label: "Test".into(),
            validation: Default::default(),
        }
    }

    async fn get(port: u16, target: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut out = String::new();
        stream.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn rejects_bad_requests_and_hands_over_the_first_valid_code() {
        let mut server = CallbackServer::start(options()).await.unwrap();
        let port = server.port();
        assert!(get(port, "/other").await.starts_with("HTTP/1.1 404"));
        assert!(get(port, "/callback?code=c")
            .await
            .contains("Missing code or state parameter."));
        assert!(get(port, "/callback?code=c&state=bad")
            .await
            .contains("State mismatch."));
        let denied = get(port, "/callback?error=access_denied").await;
        assert!(
            denied.contains("Test authentication did not complete.")
                && denied.contains("Error: access_denied")
        );
        let ok = get(port, "/callback?code=c1&state=s1").await;
        assert!(ok.starts_with("HTTP/1.1 200") && ok.contains("Test authentication completed."));
        assert_eq!(
            server.wait_for_code().await,
            Some(CallbackCode {
                code: "c1".into(),
                state: "s1".into()
            })
        );
    }

    #[tokio::test]
    async fn state_then_code_validation_matches_openai_codex() {
        let mut server = CallbackServer::start(CallbackServerOptions {
            path: "/auth/callback".into(),
            label: "OpenAI".into(),
            validation: CallbackValidation::StateThenCode,
            ..options()
        })
        .await
        .unwrap();
        let port = server.port();
        assert!(get(port, "/callback?code=c&state=s1")
            .await
            .contains("Callback route not found."));
        assert!(get(port, "/auth/callback?error=access_denied&code=c")
            .await
            .contains("State mismatch."));
        let missing = get(port, "/auth/callback?state=s1&error=access_denied").await;
        assert!(
            missing.starts_with("HTTP/1.1 400") && missing.contains("Missing authorization code.")
        );
        let ok = get(port, "/auth/callback?code=c1&state=s1").await;
        assert!(ok.contains("OpenAI authentication completed. You can close this window."));
        assert_eq!(server.wait_for_code().await.unwrap().code, "c1");
    }

    #[tokio::test]
    async fn deferred_state_validation_matches_the_google_flows() {
        let mut server = CallbackServer::start(CallbackServerOptions {
            label: "Google".into(),
            validation: CallbackValidation::CodeAndStateDeferred,
            ..options()
        })
        .await
        .unwrap();
        let port = server.port();
        let denied = get(port, "/callback?error=access_denied").await;
        assert!(denied.contains("Google authentication did not complete."));
        let missing = get(port, "/callback?code=c1").await;
        assert!(missing.contains("Missing code or state parameter."));
        // Any state is handed over; the flow compares it.
        let ok = get(port, "/callback?code=c1&state=other").await;
        assert!(ok.contains("Google authentication completed. You can close this window."));
        let code = server.wait_for_code().await.unwrap();
        assert_eq!((code.code.as_str(), code.state.as_str()), ("c1", "other"));
    }

    #[tokio::test]
    async fn cancel_wait_settles_with_none() {
        let mut server = CallbackServer::start(options()).await.unwrap();
        server.cancel_handle().cancel();
        assert_eq!(server.wait_for_code().await, None);
    }
}
