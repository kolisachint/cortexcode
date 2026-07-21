//! Shared OAuth types.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `utils/oauth/types.ts`.

use serde::{Deserialize, Serialize};

/// Credentials returned by a completed OAuth flow and persisted by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthCredentials {
    pub refresh: String,
    pub access: String,
    /// Unix epoch milliseconds at which `access` should be considered expired
    /// (already shifted 5 minutes earlier than the provider's stated expiry).
    pub expires: i64,
    /// Provider-specific extra fields (e.g. GitHub Copilot's `enterprise_url`).
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

impl OAuthCredentials {
    pub fn is_expired(&self, now_millis: i64) -> bool {
        now_millis >= self.expires
    }
}
