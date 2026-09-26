//! OpenAI Codex OAuth (ChatGPT Plus/Pro subscription): port of hoocode
//! `utils/oauth/openai-codex.ts` (v0.5.89). Authorization code + PKCE with a
//! local callback server on port 1455 racing an optional pasted code; the
//! credentials carry the `accountId` claim of the access token.

use base64::Engine as _;
use cortexcode_ai_oauth::{
    form_body, pkce::generate_pkce, BoxFuture, CallbackServer, CallbackServerOptions,
    CallbackValidation, Fetch, HttpRequest, OAuthAuthInfo, OAuthCredentials, OAuthLoginCallbacks,
    OAuthPrompt, OAuthProvider, ReqwestFetch,
};
use serde_json::Value;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const CALLBACK_PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";
const SCOPE: &str = "openid profile email offline_access";
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

/// The originator this OAuth client is registered under. Not branding: the
/// ChatGPT backend identifies the client by client id plus originator and
/// rejects an unrecognised pair (the provider sends the same value).
pub const DEFAULT_ORIGINATOR: &str = "pi";

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

/// `createState`: 16 random bytes as hex.
fn create_state() -> String {
    uuid::Uuid::new_v4().simple().to_string()
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

/// `decodeJwt`: the payload of a three-part token (`atob`, so standard
/// base64 with optional padding).
fn decode_jwt(token: &str) -> Option<Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    let bytes = engine.decode(parts[1]).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `getAccountId`.
pub fn get_account_id(access_token: &str) -> Option<String> {
    decode_jwt(access_token)?[JWT_CLAIM_PATH]["chatgpt_account_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// `createAuthorizationFlow`: the authorize URL for a PKCE challenge.
pub fn authorize_url(challenge: &str, state: &str, originator: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPE),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", originator),
        ])
        .finish();
    format!("{AUTHORIZE_URL}?{query}")
}

/// A successful token response: access, refresh, expiry (Unix ms).
struct Tokens {
    access: String,
    refresh: String,
    expires: i64,
}

/// POST a form to the token endpoint; `what` is `exchange` or `refresh`.
async fn token_request(
    fetch: &dyn Fetch,
    pairs: &[(&str, &str)],
    what: &str,
) -> Result<Tokens, String> {
    let request = HttpRequest::new("POST", TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(pairs));
    let response = fetch.fetch(request).await?;
    if !response.ok() {
        let text = if response.body.is_empty() {
            &response.status_text
        } else {
            &response.body
        };
        return Err(format!(
            "OpenAI Codex token {what} failed ({}): {text}",
            response.status
        ));
    }
    let json: Value = serde_json::from_str(&response.body).map_err(|e| e.to_string())?;
    let access = json["access_token"].as_str().filter(|s| !s.is_empty());
    let refresh = json["refresh_token"].as_str().filter(|s| !s.is_empty());
    match (access, refresh, json["expires_in"].as_f64()) {
        (Some(access), Some(refresh), Some(expires_in)) => Ok(Tokens {
            access: access.to_string(),
            refresh: refresh.to_string(),
            expires: cortexcode_ai_oauth::now_ms() + (expires_in * 1000.0) as i64,
        }),
        _ => Err(format!(
            "OpenAI Codex token {what} response missing fields: {json}"
        )),
    }
}

/// `exchangeAuthorizationCode`.
async fn exchange_authorization_code(
    fetch: &dyn Fetch,
    code: &str,
    verifier: &str,
) -> Result<Tokens, String> {
    token_request(
        fetch,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", REDIRECT_URI),
        ],
        "exchange",
    )
    .await
}

fn credentials(tokens: Tokens) -> Result<OAuthCredentials, String> {
    let account_id =
        get_account_id(&tokens.access).ok_or("Failed to extract accountId from token")?;
    let mut creds = OAuthCredentials::new(tokens.refresh, tokens.access, tokens.expires);
    creds
        .extra
        .insert("accountId".into(), Value::String(account_id));
    Ok(creds)
}

/// `refreshOpenAICodexToken`. Failures are returned, never logged.
pub async fn refresh_openai_codex_token(
    fetch: &dyn Fetch,
    refresh_token: &str,
) -> Result<OAuthCredentials, String> {
    let tokens = token_request(
        fetch,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ],
        "refresh",
    )
    .await
    .map_err(|e| {
        if e.starts_with("OpenAI Codex token refresh") {
            e
        } else {
            format!("OpenAI Codex token refresh error: {e}")
        }
    })?;
    credentials(tokens)
}

/// Parse pasted input, checking a state it carries.
fn parse_pasted(input: &str, state: &str) -> Result<Option<String>, String> {
    let (code, pasted_state) = parse_authorization_input(input);
    if pasted_state.as_deref().is_some_and(|s| s != state) {
        return Err("State mismatch".to_string());
    }
    Ok(code)
}

/// `loginOpenAICodex`.
pub async fn login_openai_codex(
    fetch: &dyn Fetch,
    callbacks: &dyn OAuthLoginCallbacks,
    originator: Option<&str>,
) -> Result<OAuthCredentials, String> {
    let pkce = generate_pkce();
    let state = create_state();
    let url = authorize_url(
        &pkce.challenge,
        &state,
        originator.unwrap_or(DEFAULT_ORIGINATOR),
    );
    // A port already in use leaves only the manual paths (the TS server
    // resolves with a `waitForCode` of null).
    let mut server = CallbackServer::start(CallbackServerOptions {
        host: callback_host(),
        port: CALLBACK_PORT,
        path: CALLBACK_PATH.to_string(),
        expected_state: state.clone(),
        label: "OpenAI".to_string(),
        validation: CallbackValidation::StateThenCode,
    })
    .await
    .ok();

    callbacks.on_auth(OAuthAuthInfo {
        url,
        instructions: Some("A browser window should open. Complete login to finish.".to_string()),
    });

    let mut code: Option<String> = None;
    match callbacks.on_manual_code_input() {
        Some(mut manual) => {
            let mut manual_result = None;
            let from_server = match server.as_mut() {
                Some(server) => {
                    let cancel = server.cancel_handle();
                    tokio::select! {
                        result = server.wait_for_code() => result,
                        input = &mut manual => {
                            cancel.cancel();
                            manual_result = Some(input);
                            None
                        }
                    }
                }
                None => None,
            };
            if let Some(result) = from_server {
                code = Some(result.code);
            } else {
                // The manual input won, or the server gave up: wait for it.
                let input = match manual_result {
                    Some(input) => input,
                    None => manual.await,
                };
                code = parse_pasted(&input?, &state)?;
            }
        }
        None => {
            if let Some(server) = server.as_mut() {
                code = server.wait_for_code().await.map(|r| r.code);
            }
        }
    }

    if code.as_deref().is_none_or(str::is_empty) {
        let input = callbacks
            .on_prompt(OAuthPrompt {
                message: "Paste the authorization code (or full redirect URL):".to_string(),
                ..Default::default()
            })
            .await?;
        code = parse_pasted(&input, &state)?;
    }
    drop(server);

    let code = code
        .filter(|c| !c.is_empty())
        .ok_or("Missing authorization code")?;
    let tokens = exchange_authorization_code(fetch, &code, &pkce.verifier).await?;
    credentials(tokens)
}

/// `openaiCodexOAuthProvider`.
pub struct OpenAICodexOAuthProvider {
    fetch: Box<dyn Fetch>,
}

impl Default for OpenAICodexOAuthProvider {
    fn default() -> Self {
        Self::with_fetch(Box::new(ReqwestFetch))
    }
}

impl OpenAICodexOAuthProvider {
    pub fn with_fetch(fetch: Box<dyn Fetch>) -> Self {
        Self { fetch }
    }
}

impl OAuthProvider for OpenAICodexOAuthProvider {
    fn id(&self) -> &str {
        "openai-codex"
    }

    fn name(&self) -> &str {
        "ChatGPT Plus/Pro (Codex Subscription)"
    }

    fn uses_callback_server(&self) -> bool {
        true
    }

    fn login<'a>(
        &'a self,
        callbacks: &'a dyn OAuthLoginCallbacks,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(login_openai_codex(self.fetch.as_ref(), callbacks, None))
    }

    fn refresh_token<'a>(
        &'a self,
        credentials: &'a OAuthCredentials,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(refresh_openai_codex_token(
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
