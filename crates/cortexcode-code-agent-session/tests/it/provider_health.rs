//! provider-health.test.ts. The records are process-wide, so the cases run
//! as one test.

use std::time::Duration;

use cortexcode_code_agent_session::provider_health::*;

#[test]
fn provider_health() {
    // records and returns an active exhaustion signal
    reset_provider_health_for_testing();
    mark_provider_exhausted("anthropic", "Usage limit reached");
    let record = get_provider_exhaustion("anthropic").unwrap();
    assert_eq!(record.provider, "anthropic");
    assert!(record.message.contains("Usage limit reached"));

    // returns nothing for an unflagged provider
    assert_eq!(get_provider_exhaustion("openai"), None);

    // clears a signal on demand
    clear_provider_exhaustion("anthropic");
    assert_eq!(get_provider_exhaustion("anthropic"), None);

    // expires after its TTL and is pruned
    mark_provider_exhausted("anthropic", "boom");
    assert_eq!(
        get_provider_exhaustion_with_ttl("anthropic", Duration::ZERO),
        None
    );
    assert_eq!(get_provider_exhaustion("anthropic"), None);

    // truncates very long messages
    mark_provider_exhausted("anthropic", &"x".repeat(500));
    let record = get_provider_exhaustion("anthropic").unwrap();
    assert!(record.message.chars().count() <= 200);
    assert!(record.message.ends_with('\u{2026}'));
    reset_provider_health_for_testing();
}

#[test]
fn classifies_quota_errors_but_not_transient_blips() {
    for m in [
        "Usage limit reached",
        "rate_limit_error: too many requests",
        "HTTP 429",
        "insufficient_quota",
        "credit balance is too low",
    ] {
        assert!(is_provider_quota_error(Some(m)), "{m}");
    }
    assert!(!is_provider_quota_error(Some("socket hang up")));
    assert!(!is_provider_quota_error(Some("overloaded_error")));
    assert!(!is_provider_quota_error(None));
}
