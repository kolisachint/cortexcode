//! Application Default Credentials: the access token `google-auth-library`
//! (`GoogleAuth` with the cloud-platform scope, via `@google/genai`'s
//! `NodeAuth`) puts in `Authorization: Bearer …` for Vertex AI.
//!
//! Sources, in google-auth-library's order: the `GOOGLE_APPLICATION_CREDENTIALS`
//! file, gcloud's well-known `application_default_credentials.json`, then the
//! GCE/GKE metadata server. Service-account and authorized-user files are
//! supported; external-account (workload identity federation) files are not.

use serde::Deserialize;

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "type")]
    kind: String,
    // service_account
    private_key: Option<String>,
    client_email: Option<String>,
    token_uri: Option<String>,
    // authorized_user
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
}

#[derive(serde::Serialize)]
struct JwtClaims {
    iss: String,
    scope: String,
    aud: String,
    exp: i64,
    iat: i64,
}

/// Mint an access token from Application Default Credentials.
pub async fn access_token() -> Result<String, String> {
    if let Some(path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
        .ok()
        .filter(|p| !p.is_empty())
    {
        return token_from_file(&path).await;
    }
    if let Some(path) = well_known_file().filter(|p| std::path::Path::new(p).exists()) {
        return token_from_file(&path).await;
    }
    metadata_server_token().await.map_err(|_| {
        "Could not load the default credentials. Browse to \
         https://cloud.google.com/docs/authentication/getting-started for more information."
            .to_string()
    })
}

/// gcloud's `application_default_credentials.json`.
fn well_known_file() -> Option<String> {
    let dir = if cfg!(windows) {
        std::env::var("APPDATA").ok()?
    } else {
        std::env::var("CLOUDSDK_CONFIG").ok().or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| format!("{h}/.config/gcloud"))
        })?
    };
    let dir = if cfg!(windows) {
        format!("{dir}/gcloud")
    } else {
        dir
    };
    Some(format!("{dir}/application_default_credentials.json"))
}

async fn token_from_file(path: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read credentials file {path}: {e}"))?;
    let file: CredentialsFile =
        serde_json::from_str(&text).map_err(|e| format!("Failed to parse {path}: {e}"))?;
    match file.kind.as_str() {
        "service_account" => service_account_token(&file).await,
        "authorized_user" => authorized_user_token(&file).await,
        other => Err(format!("Unsupported credentials type: {other}")),
    }
}

async fn post_token_form(uri: &str, form: &[(&str, &str)]) -> Result<String, String> {
    let response = reqwest::Client::new()
        .post(uri)
        .form(form)
        .send()
        .await
        .map_err(|e| format!("Failed to send token request: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Token exchange failed with status {status}: {body}"
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {e}"))?;
    body["access_token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "No access_token in token response".to_string())
}

/// Service account: an RS256 JWT exchanged for an access token.
async fn service_account_token(file: &CredentialsFile) -> Result<String, String> {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

    let (Some(private_key), Some(client_email)) = (&file.private_key, &file.client_email) else {
        return Err("The service account file is missing private_key or client_email".into());
    };
    let token_uri = file.token_uri.as_deref().unwrap_or(DEFAULT_TOKEN_URI);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let claims = JwtClaims {
        iss: client_email.clone(),
        scope: CLOUD_PLATFORM_SCOPE.to_string(),
        aud: token_uri.to_string(),
        exp: now + 3600,
        iat: now,
    };
    let key = EncodingKey::from_rsa_pem(private_key.as_bytes())
        .map_err(|e| format!("Failed to parse private key: {e}"))?;
    let jwt = encode(&Header::new(Algorithm::RS256), &claims, &key)
        .map_err(|e| format!("Failed to create JWT: {e}"))?;
    post_token_form(
        token_uri,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ],
    )
    .await
}

/// Authorized user (`gcloud auth application-default login`): refresh token.
async fn authorized_user_token(file: &CredentialsFile) -> Result<String, String> {
    let (Some(client_id), Some(client_secret), Some(refresh_token)) =
        (&file.client_id, &file.client_secret, &file.refresh_token)
    else {
        return Err(
            "The authorized_user file is missing client_id, client_secret or refresh_token".into(),
        );
    };
    post_token_form(
        DEFAULT_TOKEN_URI,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

/// The GCE/GKE metadata server's default service-account token.
async fn metadata_server_token() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token")
        .header("Metadata-Flavor", "Google")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("metadata server returned {}", response.status()));
    }
    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    body["access_token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "No access_token in metadata response".to_string())
}
