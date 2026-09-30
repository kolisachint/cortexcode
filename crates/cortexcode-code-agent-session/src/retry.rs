//! `core/agent-session-retry.ts`: auto-retry of transient assistant errors
//! (overloaded, rate limit, server/network/transport failures) with
//! exponential backoff, re-driving the agent through `continue()`.

use std::sync::LazyLock;

use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{AbortSignal, AssistantMessage, Model, StopReason};

use crate::session::{AgentSession, AgentSessionEvent};

/// `RETRYABLE_ERROR_PATTERN`. Context overflow is compaction's to handle.
static RETRYABLE_ERROR_PATTERN: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(
        r"(?i)overloaded|provider.?returned.?error|rate.?limit|too many requests|429|500|502|503|504|service.?unavailable|server.?error|internal.?error|network.?error|connection.?error|connection.?refused|connection.?lost|websocket.?closed|websocket.?error|other side closed|fetch failed|upstream.?connect|reset before headers|socket hang up|ended without|http2 request did not get a response|timed? out|timeout|terminated|retry delay",
    )
    .expect("retry pattern")
});

/// The retry controller's state (`AutoRetryController` fields).
#[derive(Default)]
pub(crate) struct RetryState {
    pub(crate) attempt: u64,
    /// The retry "promise" is armed: `prompt()` waits for it.
    pub(crate) pending: bool,
    /// Cancels the backoff sleep.
    pub(crate) abort: Option<AbortSignal>,
}

/// `isRetryableError`: an errored response matching the transient-failure
/// pattern that is neither a context overflow nor a long-wait quota error.
pub fn is_retryable_error(message: &AssistantMessage, model: Option<&Model>) -> bool {
    let Some(error) = message
        .error_message
        .as_deref()
        .filter(|_| message.stop_reason == StopReason::Error)
        .filter(|e| !e.is_empty())
    else {
        return false;
    };
    let context_window = model.map_or(0, |m| m.context_window);
    if cortexcode_ai_util::is_context_overflow(
        message,
        (context_window > 0).then_some(context_window),
    ) {
        return false;
    }
    if cortexcode_ai_util::is_long_retry_delay_error(Some(error)) {
        return false;
    }
    RETRYABLE_ERROR_PATTERN.is_match(error)
}

fn last_assistant(messages: &[AgentMessage]) -> Option<&AssistantMessage> {
    messages.iter().rev().find_map(|m| match m {
        AgentMessage::Assistant(a) => Some(a),
        _ => None,
    })
}

impl AgentSession {
    /// `createPromiseForAgentEnd`: arm the retry synchronously as the agent
    /// ends, so `prompt()`'s wait can never miss it.
    pub(crate) fn arm_retry_for_agent_end(&self, messages: &[AgentMessage]) {
        if self.retry_state().pending || !self.settings().retry_settings().enabled {
            return;
        }
        let model = self.model();
        if last_assistant(messages).is_some_and(|m| is_retryable_error(m, model.as_ref())) {
            self.retry_state().pending = true;
        }
    }

    /// `onSuccessfulAssistantResponse`: end a retry streak.
    pub(crate) fn on_successful_assistant_response(&self) {
        let attempt = std::mem::take(&mut self.retry_state().attempt);
        if attempt > 0 {
            self.emit(AgentSessionEvent::AutoRetryEnd {
                success: true,
                attempt,
                final_error: None,
            });
        }
    }

    /// `resolve()`: release whoever waits for the retry.
    pub(crate) fn resolve_retry(&self) {
        let was_pending = std::mem::take(&mut self.retry_state().pending);
        if was_pending {
            self.retry_notify().notify_waiters();
        }
    }

    /// `handleRetryableError`: back off, drop the error from the context and
    /// continue. `false` when retries are disabled, exhausted or cancelled.
    pub(crate) async fn handle_retryable_error(&self, message: &AssistantMessage) -> bool {
        let settings = self.settings().retry_settings();
        if !settings.enabled {
            self.resolve_retry();
            return false;
        }
        let attempt = {
            let mut state = self.retry_state();
            state.pending = true;
            state.attempt += 1;
            state.attempt
        };
        if attempt > settings.max_retries {
            self.emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt: attempt - 1,
                final_error: message.error_message.clone(),
            });
            self.retry_state().attempt = 0;
            self.resolve_retry();
            return false;
        }

        let delay_ms = settings
            .base_delay_ms
            .saturating_mul(2u64.saturating_pow((attempt - 1) as u32));
        // Armed before the start event so a listener can cancel right away.
        let signal = AbortSignal::new();
        self.retry_state().abort = Some(signal.clone());
        self.emit(AgentSessionEvent::AutoRetryStart {
            attempt,
            max_attempts: settings.max_retries,
            delay_ms,
            error_message: message
                .error_message
                .clone()
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "Unknown error".into()),
        });

        // Drop the error from the context (the session keeps it for history).
        let mut messages = self.messages();
        if matches!(messages.last(), Some(AgentMessage::Assistant(_))) {
            messages.pop();
            self.agent().set_messages(messages);
        }

        let cancelled = tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => false,
            _ = signal.cancelled() => true,
        };
        self.retry_state().abort = None;
        if cancelled {
            let attempt = std::mem::take(&mut self.retry_state().attempt);
            self.emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: Some("Retry cancelled".into()),
            });
            self.resolve_retry();
            return false;
        }

        self.continue_agent(std::time::Duration::ZERO);
        true
    }

    /// `continueAgent`: once the current run has settled, continue from the
    /// transcript (errors surface on the next `agent_end`).
    pub(crate) fn continue_agent(&self, delay: std::time::Duration) {
        let session = self.clone();
        self.spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            session.agent().wait_for_idle().await;
            let _ = session.agent().r#continue().await;
        });
    }

    /// Cancel an in-progress retry (`abortRetry`).
    pub fn abort_retry(&self) {
        if let Some(signal) = &self.retry_state().abort {
            signal.abort();
        }
        self.resolve_retry();
    }

    /// Wait for any retry in progress, then for the agent to go idle.
    pub(crate) async fn wait_for_retry(&self) {
        loop {
            let notified = self.retry_notify().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.retry_state().pending {
                break;
            }
            notified.await;
        }
        self.agent().wait_for_idle().await;
    }

    /// Whether a retry is in progress.
    pub fn is_retrying(&self) -> bool {
        self.retry_state().pending
    }

    /// The current retry attempt (0 when not retrying).
    pub fn retry_attempt(&self) -> u64 {
        self.retry_state().attempt
    }

    pub fn auto_retry_enabled(&self) -> bool {
        self.settings().retry_enabled()
    }

    pub fn set_auto_retry_enabled(&self, enabled: bool) {
        self.settings().set_retry_enabled(enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errored(message: &str) -> AssistantMessage {
        AssistantMessage {
            stop_reason: StopReason::Error,
            error_message: Some(message.into()),
            ..Default::default()
        }
    }

    #[test]
    fn classifies_retryable_errors() {
        for e in [
            "overloaded_error",
            "Provider finish_reason: network_error",
            "429 Too Many Requests",
            "fetch failed",
            "request timed out",
            "WebSocket closed",
        ] {
            assert!(is_retryable_error(&errored(e), None), "{e}");
        }
        for e in ["invalid_api_key", "prompt is too long", ""] {
            assert!(!is_retryable_error(&errored(e), None), "{e}");
        }
        let ok = AssistantMessage {
            error_message: Some("overloaded".into()),
            ..Default::default()
        };
        assert!(!is_retryable_error(&ok, None));
    }

    // Ports of `coding-agent/test/retry-quota-classification.test.ts`. Its
    // `sleep` half guards a JS timer clamp (setTimeout overflows past 2^31 ms);
    // tokio's sleep takes such delays as they are, checked below.

    fn describe_429(message: &str, retry_after: &str) -> String {
        let headers = [("retry-after".to_string(), retry_after.to_string())];
        cortexcode_ai_util::describe_provider_error(message, Some(&headers), None)
    }

    #[test]
    fn does_not_retry_a_quota_that_resets_in_weeks() {
        let quota = describe_429("429 quota exceeded", "2472352");
        assert!(!is_retryable_error(&errored(&quota), None), "{quota}");
    }

    #[test]
    fn still_retries_a_burst_rate_limit() {
        let transient = describe_429("429 rate limit exceeded", "30");
        assert!(is_retryable_error(&errored(&transient), None));
    }

    #[test]
    fn still_retries_the_transient_failures_it_always_did() {
        for text in [
            "overloaded",
            "500 internal error",
            "fetch failed",
            "socket hang up",
        ] {
            assert!(is_retryable_error(&errored(text), None), "{text}");
        }
    }

    #[test]
    fn still_ignores_a_message_that_is_not_an_error() {
        let ok = AssistantMessage {
            stop_reason: StopReason::Stop,
            ..errored("429 quota exceeded")
        };
        assert!(!is_retryable_error(&ok, None));
    }

    #[tokio::test]
    async fn a_delay_too_large_for_a_js_timer_stays_pending() {
        let long = tokio::time::sleep(std::time::Duration::from_millis(2_472_352_000));
        let fired = tokio::time::timeout(std::time::Duration::from_millis(25), long).await;
        assert!(fired.is_err());
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}
