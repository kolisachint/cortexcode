//! The InteractiveMode half of the pin's
//! `test/interactive-mode-anthropic-warning.test.ts`: the once-per-session
//! latch and the notice. Which auth counts is tested on the session
//! (`cortexcode-code-agent-session/tests/anthropic_subscription_auth_ts.rs`).

use std::cell::Cell;

use cortexcode_code_tui_app::interactive_mode::{
    claim_anthropic_subscription_warning, ANTHROPIC_SUBSCRIPTION_AUTH_BODY,
    ANTHROPIC_SUBSCRIPTION_AUTH_TITLE,
};

#[test]
fn warns_once_when_anthropic_subscription_auth_is_detected() {
    let mut shown = false;
    let lookups = Cell::new(0);
    let uses = || {
        lookups.set(lookups.get() + 1);
        true
    };
    let first = claim_anthropic_subscription_warning(&mut shown, uses);
    let second = claim_anthropic_subscription_warning(&mut shown, uses);
    assert!(first);
    assert!(!second);
    assert_eq!(lookups.get(), 1);
}

#[test]
fn states_the_billing_consequence_and_the_way_to_turn_it_off() {
    assert!(ANTHROPIC_SUBSCRIPTION_AUTH_TITLE.contains("Anthropic"));
    let body = ANTHROPIC_SUBSCRIPTION_AUTH_BODY.join(" ");
    assert!(body.contains("extra usage"));
    assert!(body.contains("/settings"));
}

#[test]
fn leaves_the_latch_unclaimed_for_non_subscription_auth() {
    let mut shown = false;
    assert!(!claim_anthropic_subscription_warning(&mut shown, || false));
    assert!(!shown);
}
