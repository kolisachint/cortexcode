//! `core/provider-health.ts`: a process-wide provider health signal.
//!
//! When a turn fails with a usage/quota/rate-limit error that does not recover
//! (retries exhausted or disabled), the session flags the provider as
//! exhausted for a short window. The subagent Task tool reads this to skip
//! pointless spawns: subagents inherit the parent's provider. The signal is
//! cleared on the next successful response and self-expires after a TTL.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long an exhaustion signal stays active before it self-expires.
pub const PROVIDER_EXHAUSTION_TTL: Duration = Duration::from_millis(45_000);

/// `ProviderExhaustion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderExhaustion {
    pub provider: String,
    /// Epoch ms when the exhaustion was recorded.
    pub at: u64,
    /// The provider error message that triggered it (truncated for display).
    pub message: String,
}

static RECORDS: LazyLock<Mutex<HashMap<String, ProviderExhaustion>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static QUOTA_ERROR: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(
        r"(?i)usage limit|quota|rate.?limit|too many requests|429|insufficient|out of credit|credit balance|billing|payment required|402|exceeded",
    )
    .expect("valid regex")
});

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn records() -> std::sync::MutexGuard<'static, HashMap<String, ProviderExhaustion>> {
    RECORDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Does this provider error indicate quota/credit/rate-limit exhaustion (as
/// opposed to a transient network/overload blip)?
pub fn is_provider_quota_error(message: Option<&str>) -> bool {
    message.is_some_and(|m| !m.is_empty() && QUOTA_ERROR.is_match(m))
}

/// Record that a provider is currently exhausted/rate-limited.
pub fn mark_provider_exhausted(provider: &str, message: &str) {
    let trimmed = message.trim();
    let message = if trimmed.chars().count() > 200 {
        format!("{}\u{2026}", trimmed.chars().take(199).collect::<String>())
    } else {
        trimmed.to_string()
    };
    records().insert(
        provider.to_string(),
        ProviderExhaustion {
            provider: provider.to_string(),
            at: now_ms(),
            message,
        },
    );
}

/// Clear any exhaustion signal for a provider (after a successful response).
pub fn clear_provider_exhaustion(provider: &str) {
    records().remove(provider);
}

/// The active exhaustion record for a provider, if any. Expired records are
/// pruned on read.
pub fn get_provider_exhaustion(provider: &str) -> Option<ProviderExhaustion> {
    get_provider_exhaustion_with_ttl(provider, PROVIDER_EXHAUSTION_TTL)
}

/// [`get_provider_exhaustion`] with an explicit TTL.
pub fn get_provider_exhaustion_with_ttl(
    provider: &str,
    ttl: Duration,
) -> Option<ProviderExhaustion> {
    let mut records = records();
    let record = records.get(provider)?;
    if now_ms().saturating_sub(record.at) >= ttl.as_millis() as u64 {
        records.remove(provider);
        return None;
    }
    Some(record.clone())
}

/// Test helper: clear all recorded exhaustion signals.
pub fn reset_provider_health_for_testing() {
    records().clear();
}
