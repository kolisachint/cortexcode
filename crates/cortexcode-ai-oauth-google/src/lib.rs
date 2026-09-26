//! Google Cloud Code Assist OAuth: port of hoocode
//! `utils/oauth/google-gemini-cli.ts`, `google-antigravity.ts` and
//! `google-oauth-client.ts` (v0.5.89).
//!
//! Both logins are authorization code + PKCE (the verifier doubles as the
//! state) with a local callback server racing a pasted redirect URL, then
//! discover or provision the Cloud project requests are billed to. They
//! differ in client, port, scopes and how forgiving project discovery is.

use cortexcode_ai_oauth::{
    form_body, pkce::generate_pkce, BoxFuture, CallbackServer, CallbackServerOptions,
    CallbackValidation, Fetch, HttpRequest, OAuthAuthInfo, OAuthCredentials, OAuthLoginCallbacks,
    OAuthProvider, ReqwestFetch,
};
use serde_json::{json, Map, Value};

mod antigravity;
mod gemini_cli;

pub use antigravity::{
    login_antigravity, refresh_antigravity_token, AntigravityOAuthProvider, ANTIGRAVITY,
};
pub use gemini_cli::{
    login_gemini_cli, refresh_google_cloud_token, GeminiCliOAuthProvider, GEMINI_CLI,
};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const CODE_ASSIST_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v1/userinfo?alt=json";
const STATE_MISMATCH: &str = "OAuth state mismatch - possible CSRF attack";
/// Tokens are treated as expired five minutes early.
const EXPIRY_MARGIN_MS: i64 = 5 * 60 * 1000;

/// `GoogleOAuthClientEnv`: where a flow's client comes from.
#[derive(Debug, Clone, Copy)]
pub struct GoogleOAuthClientEnv {
    /// The client id variable (`CORTEXCODE_…`; the `HOOCODE_…` twin is
    /// honored too).
    pub id_var: &'static str,
    pub secret_var: &'static str,
    /// The provider name as `/login` shows it.
    pub product_name: &'static str,
}

/// `GoogleOAuthClient`.
#[derive(Debug, Clone, PartialEq)]
pub struct GoogleOAuthClient {
    pub client_id: String,
    pub client_secret: String,
}

/// The value of `var` or its `HOOCODE_` twin, trimmed; `None` when blank.
fn env_value(var: &str) -> Option<String> {
    let twin = var.replacen("CORTEXCODE_", "HOOCODE_", 1);
    [var, twin.as_str()].iter().find_map(|v| {
        std::env::var(v)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

/// `readGoogleOAuthClient`: both halves from the environment, or an error
/// naming what to set (it is the only instruction the user gets).
pub fn read_google_oauth_client(env: &GoogleOAuthClientEnv) -> Result<GoogleOAuthClient, String> {
    read_google_oauth_client_with(env, env_value)
}

fn read_google_oauth_client_with(
    env: &GoogleOAuthClientEnv,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<GoogleOAuthClient, String> {
    match (lookup(env.id_var), lookup(env.secret_var)) {
        (Some(client_id), Some(client_secret)) => Ok(GoogleOAuthClient {
            client_id,
            client_secret,
        }),
        (id, secret) => {
            let missing = match (id, secret) {
                (None, None) => format!("{} and {}", env.id_var, env.secret_var),
                (None, _) => env.id_var.to_string(),
                _ => env.secret_var.to_string(),
            };
            Err(format!(
                "{} needs an OAuth client, and cortexcode does not ship one. Set {missing} to the \
                 installed-app credentials of the client you are signing in as, then run /login again. \
                 See docs/providers.md for where those values come from.",
                env.product_name
            ))
        }
    }
}

/// `CALLBACK_HOST`: `CORTEXCODE_OAUTH_CALLBACK_HOST` / `HOOCODE_…`, default
/// `127.0.0.1`.
fn callback_host() -> String {
    [
        "CORTEXCODE_OAUTH_CALLBACK_HOST",
        "HOOCODE_OAUTH_CALLBACK_HOST",
    ]
    .iter()
    .find_map(|v| std::env::var(v).ok().filter(|h| !h.is_empty()))
    .unwrap_or_else(|| "127.0.0.1".to_string())
}

/// `GOOGLE_CLOUD_PROJECT || GOOGLE_CLOUD_PROJECT_ID`.
fn env_project_id() -> Option<String> {
    ["GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_PROJECT_ID"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
}

/// `parseRedirectUrl`: `code` and `state` of a pasted redirect URL; nothing
/// for input that is not a URL.
pub fn parse_redirect_url(input: &str) -> (Option<String>, Option<String>) {
    let Ok(url) = url::Url::parse(input.trim()) else {
        return (None, None);
    };
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    (param("code"), param("state"))
}

/// What differs between the two logins.
#[derive(Debug, Clone, Copy)]
pub struct GoogleFlow {
    pub client_env: GoogleOAuthClientEnv,
    pub callback_port: u16,
    pub callback_path: &'static str,
    pub redirect_uri: &'static str,
    pub scopes: &'static [&'static str],
    /// Prefix of the refresh error (`Google Cloud`, `Antigravity`).
    pub refresh_label: &'static str,
}

impl GoogleFlow {
    /// The authorization URL (`state` is the PKCE verifier).
    pub fn authorize_url(&self, client_id: &str, challenge: &str, verifier: &str) -> String {
        let scope = self.scopes.join(" ");
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("client_id", client_id),
                ("response_type", "code"),
                ("redirect_uri", self.redirect_uri),
                ("scope", scope.as_str()),
                ("code_challenge", challenge),
                ("code_challenge_method", "S256"),
                ("state", verifier),
                ("access_type", "offline"),
                ("prompt", "consent"),
            ])
            .finish();
        format!("{AUTH_URL}?{query}")
    }
}

fn parse_json(body: &str) -> Result<Value, String> {
    serde_json::from_str(body).map_err(|e| e.to_string())
}

/// `refreshGoogleCloudToken` / `refreshAntigravityToken`.
async fn refresh_token(
    flow: &GoogleFlow,
    fetch: &dyn Fetch,
    refresh_token: &str,
    project_id: &str,
) -> Result<OAuthCredentials, String> {
    let client = read_google_oauth_client(&flow.client_env)?;
    let request = HttpRequest::new("POST", TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&[
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ]));
    let response = fetch.fetch(request).await?;
    if !response.ok() {
        return Err(format!(
            "{} token refresh failed: {}",
            flow.refresh_label, response.body
        ));
    }
    let data = parse_json(&response.body)?;
    let refresh = data["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(refresh_token);
    let mut credentials = OAuthCredentials::new(
        refresh,
        data["access_token"].as_str().unwrap_or_default(),
        expires_at(&data),
    );
    credentials
        .extra
        .insert("projectId".into(), Value::String(project_id.to_string()));
    Ok(credentials)
}

/// `Date.now() + expires_in * 1000 - 5 min`.
fn expires_at(token: &Value) -> i64 {
    let expires_in = token["expires_in"].as_f64().unwrap_or(0.0);
    cortexcode_ai_oauth::now_ms() + (expires_in * 1000.0) as i64 - EXPIRY_MARGIN_MS
}

/// `getUserEmail`: best effort.
async fn get_user_email(fetch: &dyn Fetch, access_token: &str) -> Option<String> {
    let request = HttpRequest::new("GET", USERINFO_URL)
        .header("Authorization", format!("Bearer {access_token}"));
    let response = fetch.fetch(request).await.ok()?;
    if !response.ok() {
        return None;
    }
    parse_json(&response.body).ok()?["email"]
        .as_str()
        .map(str::to_string)
}

/// The code from the callback or a pasted redirect URL, the state checked
/// against the verifier.
async fn wait_for_code(
    server: &mut CallbackServer,
    callbacks: &dyn OAuthLoginCallbacks,
    verifier: &str,
) -> Result<Option<String>, String> {
    let check = |state: Option<&str>| match state {
        Some(state) if state != verifier => Err(STATE_MISMATCH.to_string()),
        _ => Ok(()),
    };
    match callbacks.on_manual_code_input() {
        Some(mut manual) => {
            tokio::select! {
                result = server.wait_for_code() => match result {
                    Some(result) => {
                        check(Some(&result.state))?;
                        Ok(Some(result.code))
                    }
                    None => Ok(None),
                },
                input = &mut manual => {
                    let (code, state) = parse_redirect_url(&input?);
                    check(state.as_deref())?;
                    Ok(code)
                }
            }
        }
        None => match server.wait_for_code().await {
            Some(result) => {
                check(Some(&result.state))?;
                Ok(Some(result.code))
            }
            None => Ok(None),
        },
    }
}

/// `loginGeminiCli` / `loginAntigravity` up to the project: the flow's
/// credentials with `projectId` set by `discover`.
async fn login<'a, D>(
    flow: &GoogleFlow,
    fetch: &'a dyn Fetch,
    callbacks: &'a dyn OAuthLoginCallbacks,
    discover: D,
) -> Result<OAuthCredentials, String>
where
    D: FnOnce(
        &'a dyn Fetch,
        String,
        &'a dyn OAuthLoginCallbacks,
    ) -> BoxFuture<'a, Result<String, String>>,
{
    let pkce = generate_pkce();
    // Read before the browser opens: a missing client fails here.
    let client = read_google_oauth_client(&flow.client_env)?;

    callbacks.on_progress("Starting local server for OAuth callback...");
    let mut server = CallbackServer::start(CallbackServerOptions {
        host: callback_host(),
        port: flow.callback_port,
        path: flow.callback_path.to_string(),
        expected_state: pkce.verifier.clone(),
        label: "Google".to_string(),
        validation: CallbackValidation::CodeAndStateDeferred,
    })
    .await?;

    callbacks.on_auth(OAuthAuthInfo {
        url: flow.authorize_url(&client.client_id, &pkce.challenge, &pkce.verifier),
        instructions: Some("Complete the sign-in in your browser.".to_string()),
    });
    callbacks.on_progress("Waiting for OAuth callback...");
    let code = wait_for_code(&mut server, callbacks, &pkce.verifier).await;
    drop(server);
    let code = code?
        .filter(|c| !c.is_empty())
        .ok_or("No authorization code received")?;

    callbacks.on_progress("Exchanging authorization code for tokens...");
    let request = HttpRequest::new("POST", TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&[
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
            ("code", &code),
            ("grant_type", "authorization_code"),
            ("redirect_uri", flow.redirect_uri),
            ("code_verifier", &pkce.verifier),
        ]));
    let response = fetch.fetch(request).await?;
    if !response.ok() {
        return Err(format!("Token exchange failed: {}", response.body));
    }
    let token = parse_json(&response.body)?;
    let refresh = token["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("No refresh token received. Please try again.")?
        .to_string();
    let access = token["access_token"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    callbacks.on_progress("Getting user info...");
    let email = get_user_email(fetch, &access).await;
    let project_id = discover(fetch, access.clone(), callbacks).await?;

    let mut credentials = OAuthCredentials::new(refresh, access, expires_at(&token));
    credentials
        .extra
        .insert("projectId".into(), Value::String(project_id));
    if let Some(email) = email {
        credentials
            .extra
            .insert("email".into(), Value::String(email));
    }
    Ok(credentials)
}

/// `getApiKey`: `JSON.stringify({token, projectId})` (`projectId` left out
/// when missing).
pub fn google_api_key(credentials: &OAuthCredentials) -> String {
    let mut key = Map::new();
    key.insert("token".into(), json!(credentials.access));
    if let Some(project_id) = credentials.extra.get("projectId") {
        key.insert("projectId".into(), project_id.clone());
    }
    Value::Object(key).to_string()
}

/// A JSON POST/GET to the Code Assist API with `headers`.
fn code_assist_request(
    method: &'static str,
    path: &str,
    headers: &[(&str, String)],
    body: Option<Value>,
) -> HttpRequest {
    let mut request = HttpRequest::new(method, format!("{CODE_ASSIST_ENDPOINT}{path}"));
    for (k, v) in headers {
        request = request.header(k, v.clone());
    }
    if let Some(body) = body {
        request = request.body(body.to_string());
    }
    request
}

fn default_fetch() -> Box<dyn Fetch> {
    Box::new(ReqwestFetch)
}

/// Shared `OAuthProvider` plumbing for the two flows.
macro_rules! google_provider {
    ($name:ident, $id:expr, $label:expr, $login:path, $refresh:path, $missing:expr) => {
        pub struct $name {
            fetch: Box<dyn Fetch>,
        }

        impl Default for $name {
            fn default() -> Self {
                Self::with_fetch($crate::default_fetch())
            }
        }

        impl $name {
            pub fn with_fetch(fetch: Box<dyn Fetch>) -> Self {
                Self { fetch }
            }
        }

        impl OAuthProvider for $name {
            fn id(&self) -> &str {
                $id
            }

            fn name(&self) -> &str {
                $label
            }

            fn uses_callback_server(&self) -> bool {
                true
            }

            fn login<'a>(
                &'a self,
                callbacks: &'a dyn OAuthLoginCallbacks,
            ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
                Box::pin($login(self.fetch.as_ref(), callbacks))
            }

            fn refresh_token<'a>(
                &'a self,
                credentials: &'a OAuthCredentials,
            ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
                Box::pin(async move {
                    let project_id = credentials
                        .extra_str("projectId")
                        .filter(|p| !p.is_empty())
                        .ok_or($missing)?;
                    $refresh(self.fetch.as_ref(), &credentials.refresh, project_id).await
                })
            }

            fn get_api_key(&self, credentials: &OAuthCredentials) -> String {
                $crate::google_api_key(credentials)
            }
        }
    };
}
use google_provider;

#[cfg(test)]
mod tests;
