//! Anthropic OAuth (Claude Pro/Max): port of hoocode
//! `utils/oauth/anthropic.ts` (v0.5.89). Authorization code + PKCE with a
//! local callback server on port 53692 and an optional pasted redirect URL.

use cortexcode_ai_oauth::{
    pkce::generate_pkce, BoxFuture, CallbackServer, CallbackServerOptions, Fetch, HttpRequest,
    OAuthAuthInfo, OAuthCredentials, OAuthLoginCallbacks, OAuthPrompt, OAuthProvider, ReqwestFetch,
};
use serde_json::{json, Value};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CALLBACK_PORT: u16 = 53692;
const CALLBACK_PATH: &str = "/callback";
const SCOPES: &str =
    "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// `REDIRECT_URI`.
pub fn redirect_uri() -> String {
    format!("http://localhost:{CALLBACK_PORT}{CALLBACK_PATH}")
}

/// `CALLBACK_HOST`: `HOOCODE_OAUTH_CALLBACK_HOST` (or the cortex spelling),
/// default `127.0.0.1`.
fn callback_host() -> String {
    [
        "CORTEXCODE_OAUTH_CALLBACK_HOST",
        "HOOCODE_OAUTH_CALLBACK_HOST",
    ]
    .iter()
    .find_map(|v| std::env::var(v).ok().filter(|h| !h.is_empty()))
    .unwrap_or_else(|| "127.0.0.1".to_string())
}

/// The authorize URL for a PKCE pair (`state` is the verifier).
pub fn authorize_url(challenge: &str, verifier: &str) -> String {
    let redirect = redirect_uri();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            ("scope", SCOPES),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", verifier),
        ])
        .finish();
    format!("{AUTHORIZE_URL}?{query}")
}

/// `parseAuthorizationInput`: a redirect URL, `code#state`, a
/// `code=…&state=…` query, or a bare code.
pub fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }
    if let Ok(url) = url::Url::parse(value) {
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        return (param("code"), param("state"));
    }
    if let Some((code, state)) = value.split_once('#') {
        return (Some(code.to_string()), Some(state.to_string()));
    }
    if value.contains("code=") {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(value.as_bytes())
            .into_owned()
            .collect();
        let param = |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        return (param("code"), param("state"));
    }
    (Some(value.to_string()), None)
}

/// `postJson`: 30 s timeout; a non-2xx status is an error with the body.
async fn post_json(fetch: &dyn Fetch, url: &str, body: Value) -> Result<String, String> {
    let request = HttpRequest {
        timeout_ms: Some(30_000),
        ..HttpRequest::new("POST", url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(body.to_string())
    };
    let response = fetch.fetch(request).await?;
    if !response.ok() {
        return Err(format!(
            "HTTP request failed. status={}; url={url}; body={}",
            response.status, response.body
        ));
    }
    Ok(response.body)
}

/// `formatErrorDetails` for our string errors (no JS stack to add).
fn details(error: &str) -> String {
    format!("Error: {error}")
}

fn credentials_from(body: &str) -> Result<OAuthCredentials, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct Token {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
    }
    let token: Token = serde_json::from_str(body)?;
    Ok(OAuthCredentials::new(
        token.refresh_token,
        token.access_token,
        cortexcode_ai_oauth::now_ms() + token.expires_in * 1000 - 5 * 60 * 1000,
    ))
}

/// `exchangeAuthorizationCode`.
pub async fn exchange_authorization_code(
    fetch: &dyn Fetch,
    code: &str,
    state: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthCredentials, String> {
    let body = post_json(
        fetch,
        TOKEN_URL,
        json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": state,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
        }),
    )
    .await
    .map_err(|e| {
        format!(
            "Token exchange request failed. url={TOKEN_URL}; redirect_uri={redirect_uri}; response_type=authorization_code; details={}",
            details(&e)
        )
    })?;
    credentials_from(&body).map_err(|e| {
        format!(
            "Token exchange returned invalid JSON. url={TOKEN_URL}; body={body}; details={}",
            details(&e.to_string())
        )
    })
}

/// `refreshAnthropicToken` (no `scope` in the request).
pub async fn refresh_anthropic_token(
    fetch: &dyn Fetch,
    refresh_token: &str,
) -> Result<OAuthCredentials, String> {
    let body = post_json(
        fetch,
        TOKEN_URL,
        json!({
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "refresh_token": refresh_token,
        }),
    )
    .await
    .map_err(|e| {
        format!(
            "Anthropic token refresh request failed. url={TOKEN_URL}; details={}",
            details(&e)
        )
    })?;
    credentials_from(&body).map_err(|e| {
        format!(
            "Anthropic token refresh returned invalid JSON. url={TOKEN_URL}; body={body}; details={}",
            details(&e.to_string())
        )
    })
}

/// Parse pasted input against the expected state.
fn parse_pasted(input: &str, verifier: &str) -> Result<(Option<String>, Option<String>), String> {
    let (code, state) = parse_authorization_input(input);
    if state.as_deref().is_some_and(|s| s != verifier) {
        return Err("OAuth state mismatch".to_string());
    }
    Ok((code, Some(state.unwrap_or_else(|| verifier.to_string()))))
}

/// `loginAnthropic`.
pub async fn login_anthropic(
    fetch: &dyn Fetch,
    callbacks: &dyn OAuthLoginCallbacks,
) -> Result<OAuthCredentials, String> {
    let pkce = generate_pkce();
    let verifier = pkce.verifier.clone();
    let mut server = CallbackServer::start(CallbackServerOptions {
        host: callback_host(),
        port: CALLBACK_PORT,
        path: CALLBACK_PATH.to_string(),
        expected_state: verifier.clone(),
        label: "Anthropic".to_string(),
        validation: cortexcode_ai_oauth::CallbackValidation::CodeAndState,
    })
    .await?;
    let redirect = redirect_uri();

    callbacks.on_auth(OAuthAuthInfo {
        url: authorize_url(&pkce.challenge, &verifier),
        instructions: Some(
            "Complete login in your browser. If the browser is on another machine, paste the final redirect URL here."
                .to_string(),
        ),
    });

    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    match callbacks.on_manual_code_input() {
        Some(mut manual) => {
            let mut cancel = Some(server.cancel_handle());
            let mut manual_result = None;
            let from_server = tokio::select! {
                result = server.wait_for_code() => result,
                input = &mut manual => {
                    if let Some(cancel) = cancel.take() {
                        cancel.cancel();
                    }
                    manual_result = Some(input);
                    None
                }
            };
            match (from_server, manual_result) {
                (Some(result), _) => {
                    code = Some(result.code);
                    state = Some(result.state);
                }
                (None, Some(input)) => (code, state) = parse_pasted(&input?, &verifier)?,
                // The server settled without a code: wait for the paste.
                (None, None) => (code, state) = parse_pasted(&manual.await?, &verifier)?,
            }
        }
        None => {
            if let Some(result) = server.wait_for_code().await {
                code = Some(result.code);
                state = Some(result.state);
            }
        }
    }

    if code.is_none() {
        let input = callbacks
            .on_prompt(OAuthPrompt {
                message: "Paste the authorization code or full redirect URL:".to_string(),
                placeholder: Some(redirect.clone()),
                allow_empty: false,
            })
            .await?;
        (code, state) = parse_pasted(&input, &verifier)?;
    }
    drop(server);

    let code = code
        .filter(|c| !c.is_empty())
        .ok_or("Missing authorization code")?;
    let state = state
        .filter(|s| !s.is_empty())
        .ok_or("Missing OAuth state")?;
    callbacks.on_progress("Exchanging authorization code for tokens...");
    exchange_authorization_code(fetch, &code, &state, &verifier, &redirect).await
}

/// `anthropicOAuthProvider`.
pub struct AnthropicOAuthProvider {
    fetch: Box<dyn Fetch>,
}

impl Default for AnthropicOAuthProvider {
    fn default() -> Self {
        Self::with_fetch(Box::new(ReqwestFetch))
    }
}

impl AnthropicOAuthProvider {
    pub fn with_fetch(fetch: Box<dyn Fetch>) -> Self {
        Self { fetch }
    }
}

impl OAuthProvider for AnthropicOAuthProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn name(&self) -> &str {
        "Anthropic (Claude Pro/Max)"
    }

    fn uses_callback_server(&self) -> bool {
        true
    }

    fn login<'a>(
        &'a self,
        callbacks: &'a dyn OAuthLoginCallbacks,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(login_anthropic(self.fetch.as_ref(), callbacks))
    }

    fn refresh_token<'a>(
        &'a self,
        credentials: &'a OAuthCredentials,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(refresh_anthropic_token(
            self.fetch.as_ref(),
            &credentials.refresh,
        ))
    }

    fn get_api_key(&self, credentials: &OAuthCredentials) -> String {
        credentials.access.clone()
    }
}

#[cfg(test)]
mod tests;
