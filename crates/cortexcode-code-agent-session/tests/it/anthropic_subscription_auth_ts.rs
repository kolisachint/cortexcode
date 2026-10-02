//! The session half of the pin's
//! `test/interactive-mode-anthropic-warning.test.ts`:
//! `usesAnthropicSubscriptionAuth` (here `AgentSession::uses_anthropic_subscription_auth`).
//! The latch and the notice text are tested in
//! `cortexcode-code-tui-app/tests/anthropic_warning_ts.rs`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::common::{Harness, HarnessOptions};
use cortexcode_ai_types::Model;
use cortexcode_code_models::AuthLookup;
use serde_json::json;

/// A stored credential for `anthropic` only; counts key lookups.
struct Auth {
    key: Option<&'static str>,
    oauth: bool,
    lookups: AtomicUsize,
}

impl AuthLookup for Auth {
    fn api_key(&self, provider: &str) -> Option<String> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        (provider == "anthropic")
            .then(|| self.key.map(str::to_string))
            .flatten()
    }

    fn is_oauth(&self, provider: &str) -> bool {
        provider == "anthropic" && self.oauth
    }
}

fn auth(key: Option<&'static str>, oauth: bool) -> Arc<Auth> {
    Arc::new(Auth {
        key,
        oauth,
        lookups: AtomicUsize::new(0),
    })
}

fn harness(auth: Arc<Auth>, settings: serde_json::Value) -> Harness {
    Harness::new(HarnessOptions {
        auth: Some(auth),
        settings: settings.as_object().unwrap().clone(),
        ..Default::default()
    })
}

fn model(h: &Harness, provider: &str) -> Model {
    Model {
        provider: provider.into(),
        ..h.faux.get_model()
    }
}

#[test]
fn an_oauth_subscription_key_counts_as_subscription_auth() {
    let a = auth(Some("sk-ant-oat01-test"), false);
    let h = harness(a.clone(), json!({}));
    assert!(h
        .session
        .uses_anthropic_subscription_auth(&model(&h, "anthropic")));
    assert_eq!(a.lookups.load(Ordering::SeqCst), 1);
}

#[test]
fn stored_anthropic_oauth_counts_even_if_the_key_lookup_would_fail() {
    let a = auth(None, true);
    let h = harness(a.clone(), json!({}));
    assert!(h
        .session
        .uses_anthropic_subscription_auth(&model(&h, "anthropic")));
    assert_eq!(a.lookups.load(Ordering::SeqCst), 0);
}

#[test]
fn non_anthropic_models_never_count() {
    let a = auth(Some("sk-ant-oat01-test"), true);
    let h = harness(a.clone(), json!({}));
    assert!(!h
        .session
        .uses_anthropic_subscription_auth(&model(&h, "openai")));
    assert_eq!(a.lookups.load(Ordering::SeqCst), 0);
}

#[test]
fn the_disabled_extra_usage_warning_skips_the_auth_lookup() {
    let a = auth(Some("sk-ant-oat01-test"), true);
    let h = harness(
        a.clone(),
        json!({ "warnings": { "anthropicExtraUsage": false } }),
    );
    assert!(!h
        .session
        .uses_anthropic_subscription_auth(&model(&h, "anthropic")));
    assert_eq!(a.lookups.load(Ordering::SeqCst), 0);
}

#[test]
fn a_plain_api_key_is_not_subscription_auth() {
    let a = auth(Some("sk-ant-api03-test"), false);
    let h = harness(a, json!({}));
    assert!(!h
        .session
        .uses_anthropic_subscription_auth(&model(&h, "anthropic")));
}
