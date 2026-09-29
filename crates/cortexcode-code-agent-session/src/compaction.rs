//! `core/agent-session-compaction.ts`: manual compaction (`/compact`) and
//! automatic compaction (context overflow recovery, threshold).
//!
//! The `session_before_compact` / `session_compact` extension hooks come with
//! the extension runtime (12.3).

use cortexcode_agent_compaction::{
    compact, prepare_compaction, should_compact, CompactionPreparation, CompactionResult,
    CompactionSettings, SummarizeOptions,
};
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{AbortSignal, AssistantMessage, Model, StopReason};
use cortexcode_code_session::FileEntry;

use crate::auth_guidance::{format_no_api_key_found_message, format_no_model_selected_message};
use crate::session::{AgentSession, AgentSessionError, AgentSessionEvent};

/// Why a compaction ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

/// The compaction controller's state.
#[derive(Default)]
pub(crate) struct CompactionState {
    pub(crate) manual_abort: Option<AbortSignal>,
    pub(crate) auto_abort: Option<AbortSignal>,
    /// One compact-and-retry per overflow; cleared by new input or a good response.
    pub(crate) overflow_recovery_attempted: bool,
    /// Tree navigation's branch summarization (`TreeNavigationController`).
    pub(crate) branch_summary_abort: Option<AbortSignal>,
}

/// What `checkCompaction` decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionPlan {
    /// Overflow: compact, then retry the turn.
    Overflow,
    /// Past the threshold: compact; the user continues.
    Threshold,
}

enum Applied {
    Ok(CompactionResult),
    Cancelled,
}

const OVERFLOW_GAVE_UP: &str = "Context overflow recovery failed after one compact-and-retry attempt. Try reducing context or switching to a larger-context model.";

fn timestamp_ms(iso: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|t| t.timestamp_millis())
}

impl AgentSession {
    fn compaction_settings(&self) -> CompactionSettings {
        let s = self.settings().compaction_settings();
        CompactionSettings {
            enabled: s.enabled,
            reserve_tokens: s.reserve_tokens,
            keep_recent_tokens: s.keep_recent_tokens,
            max_context_ratio: Some(s.max_context_ratio),
        }
    }

    /// Whether manual or auto compaction, or branch summarization, is running.
    pub fn is_compacting(&self) -> bool {
        let state = self.compaction_state();
        state.manual_abort.is_some()
            || state.auto_abort.is_some()
            || state.branch_summary_abort.is_some()
    }

    pub fn auto_compaction_enabled(&self) -> bool {
        self.settings().compaction_enabled()
    }

    pub fn set_auto_compaction_enabled(&self, enabled: bool) {
        self.settings().set_compaction_enabled(enabled);
    }

    pub(crate) fn reset_overflow_recovery(&self) {
        self.compaction_state().overflow_recovery_attempted = false;
    }

    /// Cancel an in-progress compaction (manual or auto).
    pub fn abort_compaction(&self) {
        let state = self.compaction_state();
        for signal in [&state.manual_abort, &state.auto_abort]
            .into_iter()
            .flatten()
        {
            signal.abort();
        }
    }

    /// `_getRequiredRequestAuth`: the key and headers for `model`, or the
    /// user-facing reason there are none.
    pub(crate) fn required_request_auth(
        &self,
        model: &Model,
    ) -> Result<(String, Option<std::collections::HashMap<String, String>>), AgentSessionError>
    {
        let auth = self
            .model_registry()
            .get_api_key_and_headers(model, self.auth().as_ref())
            .map_err(|e| {
                if e.starts_with("No API key found") {
                    AgentSessionError(format_no_api_key_found_message(&model.provider))
                } else {
                    AgentSessionError(e)
                }
            })?;
        if let Some(key) = auth.api_key {
            return Ok((key, auth.headers));
        }
        if self.auth().is_oauth(&model.provider) {
            return Err(AgentSessionError(format!(
                "Authentication failed for \"{0}\". Credentials may have expired or network is unavailable. Run '/login {0}' to re-authenticate.",
                model.provider
            )));
        }
        Err(AgentSessionError(format_no_api_key_found_message(
            &model.provider,
        )))
    }

    /// `_applyCompaction`: summarize, persist, and reload the agent context.
    async fn apply_compaction(
        &self,
        preparation: &CompactionPreparation,
        model: &Model,
        api_key: String,
        headers: Option<std::collections::HashMap<String, String>>,
        custom_instructions: Option<&str>,
        signal: &AbortSignal,
    ) -> Result<Applied, String> {
        let options = SummarizeOptions {
            api_key: Some(api_key),
            headers,
            signal: Some(signal.clone()),
            thinking_level: Some(self.thinking_level()),
        };
        // Awaited whole, as the pin does: an abort reaches the summary request
        // through its signal. A stream aborted before any text fails as an
        // empty summary; one that still returned text is "cancelled" below.
        let generated = compact(preparation, model, custom_instructions, &options).await?;
        if signal.aborted() {
            return Ok(Applied::Cancelled);
        }
        let messages = {
            let mut manager = self.session_manager();
            manager.append_compaction(
                generated.summary.clone(),
                generated.first_kept_entry_id.clone(),
                generated.tokens_before,
                generated.tokens_after,
                generated.details.clone(),
                Some(false),
            );
            manager.build_context().messages
        };
        self.agent().set_messages(messages);
        Ok(Applied::Ok(CompactionResult {
            tokens_after: Some(generated.tokens_after.unwrap_or(generated.tokens_before)),
            ..generated
        }))
    }

    /// Compact the context now (`/compact`): aborts the current run first.
    pub async fn compact(
        &self,
        custom_instructions: Option<&str>,
    ) -> Result<CompactionResult, AgentSessionError> {
        self.disconnect_from_agent();
        self.abort().await;
        let signal = AbortSignal::new();
        self.compaction_state().manual_abort = Some(signal.clone());
        self.emit(AgentSessionEvent::CompactionStart {
            reason: CompactionReason::Manual,
        });

        let result = self
            .run_manual_compaction(custom_instructions, &signal)
            .await;
        match &result {
            Ok(result) => self.emit(AgentSessionEvent::CompactionEnd {
                reason: CompactionReason::Manual,
                result: Some(result.clone()),
                aborted: false,
                will_retry: false,
                error_message: None,
            }),
            Err(error) => {
                let aborted = error.0 == "Compaction cancelled";
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Manual,
                    result: None,
                    aborted,
                    will_retry: false,
                    error_message: (!aborted).then(|| format!("Compaction failed: {}", error.0)),
                });
            }
        }
        self.compaction_state().manual_abort = None;
        self.connect_to_agent();
        result
    }

    async fn run_manual_compaction(
        &self,
        custom_instructions: Option<&str>,
        signal: &AbortSignal,
    ) -> Result<CompactionResult, AgentSessionError> {
        let Some(model) = self.model() else {
            return Err(AgentSessionError(format_no_model_selected_message()));
        };
        let (api_key, headers) = self.required_request_auth(&model)?;
        let (preparation, ends_in_compaction) = {
            let manager = self.session_manager();
            let branch: Vec<FileEntry> = manager.branch(None).into_iter().cloned().collect();
            (
                prepare_compaction(&branch, &self.compaction_settings()),
                matches!(branch.last(), Some(FileEntry::Compaction { .. })),
            )
        };
        let Some(preparation) = preparation else {
            return Err(AgentSessionError(if ends_in_compaction {
                "Already compacted".into()
            } else {
                "Nothing to compact (session too small)".into()
            }));
        };
        match self
            .apply_compaction(
                &preparation,
                &model,
                api_key,
                headers,
                custom_instructions,
                signal,
            )
            .await
            .map_err(AgentSessionError)?
        {
            Applied::Ok(result) => Ok(result),
            Applied::Cancelled => Err(AgentSessionError("Compaction cancelled".into())),
        }
    }

    /// The decision half of `checkCompaction`: whether `assistant` calls for
    /// overflow recovery or a threshold compaction. Overflow recovery is
    /// one-shot (a second overflow emits the give-up `compaction_end`) and
    /// drops the error message from the context.
    pub fn plan_compaction(
        &self,
        assistant: &AssistantMessage,
        skip_aborted_check: bool,
    ) -> Option<CompactionPlan> {
        let settings = self.compaction_settings();
        if !settings.enabled {
            return None;
        }
        if skip_aborted_check && assistant.stop_reason == StopReason::Aborted {
            return None;
        }
        let model = self.model();
        let context_window = model.as_ref().map_or(0, |m| m.context_window);
        let same_model = model
            .as_ref()
            .is_some_and(|m| assistant.provider == m.provider && assistant.model == m.id);

        // Nothing older than the latest compaction boundary retriggers it.
        let compaction_ms = {
            let manager = self.session_manager();
            manager.branch(None).iter().rev().find_map(|e| match e {
                FileEntry::Compaction { timestamp, .. } => Some(timestamp_ms(timestamp)),
                _ => None,
            })
        };
        let compaction_ms = compaction_ms.map(|t| t.unwrap_or(i64::MIN));
        if compaction_ms.is_some_and(|t| assistant.timestamp <= t) {
            return None;
        }

        if same_model
            && cortexcode_ai_util::is_context_overflow(
                assistant,
                (context_window > 0).then_some(context_window),
            )
        {
            if self.compaction_state().overflow_recovery_attempted {
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Overflow,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: Some(OVERFLOW_GAVE_UP.into()),
                });
                return None;
            }
            self.compaction_state().overflow_recovery_attempted = true;
            let mut messages = self.messages();
            if matches!(messages.last(), Some(AgentMessage::Assistant(_))) {
                messages.pop();
                self.agent().set_messages(messages);
            }
            return Some(CompactionPlan::Overflow);
        }

        let context_tokens = if assistant.stop_reason == StopReason::Error {
            // No usage on an error: estimate from the last successful response,
            // if that response came after the latest compaction.
            let messages = self.messages();
            let estimate = cortexcode_agent_compaction::estimate_context_tokens(&messages);
            let index = estimate.last_usage_index?;
            if let (Some(t), Some(AgentMessage::Assistant(usage_message))) =
                (compaction_ms, messages.get(index))
            {
                if usage_message.timestamp <= t {
                    return None;
                }
            }
            estimate.tokens
        } else {
            cortexcode_agent_compaction::calculate_context_tokens(&assistant.usage)
        };
        should_compact(context_tokens, context_window, &settings)
            .then_some(CompactionPlan::Threshold)
    }

    /// `checkCompaction`: after `agent_end`, and before a prompt (with
    /// `skip_aborted_check` false so aborted responses count).
    pub async fn check_compaction(&self, assistant: &AssistantMessage, skip_aborted_check: bool) {
        match self.plan_compaction(assistant, skip_aborted_check) {
            Some(CompactionPlan::Overflow) => {
                self.run_auto_compaction(CompactionReason::Overflow, true)
                    .await
            }
            Some(CompactionPlan::Threshold) => {
                self.run_auto_compaction(CompactionReason::Threshold, false)
                    .await
            }
            None => {}
        }
    }

    fn end_auto(&self, reason: CompactionReason, aborted: bool, error_message: Option<String>) {
        self.emit(AgentSessionEvent::CompactionEnd {
            reason,
            result: None,
            aborted,
            will_retry: false,
            error_message,
        });
    }

    /// `_runAutoCompaction`: compact with events; after an overflow retry the
    /// turn, and otherwise kick the loop when messages are queued.
    pub async fn run_auto_compaction(&self, reason: CompactionReason, will_retry: bool) {
        self.emit(AgentSessionEvent::CompactionStart { reason });
        let signal = AbortSignal::new();
        self.compaction_state().auto_abort = Some(signal.clone());
        self.auto_compaction(reason, will_retry, &signal).await;
        self.compaction_state().auto_abort = None;
    }

    async fn auto_compaction(
        &self,
        reason: CompactionReason,
        will_retry: bool,
        signal: &AbortSignal,
    ) {
        let Some(model) = self.model() else {
            return self.end_auto(reason, false, None);
        };
        let auth = self
            .model_registry()
            .get_api_key_and_headers(&model, self.auth().as_ref());
        let Ok(cortexcode_code_models::RequestAuth {
            api_key: Some(api_key),
            headers,
        }) = auth
        else {
            return self.end_auto(reason, false, None);
        };
        let preparation = {
            let manager = self.session_manager();
            let branch: Vec<FileEntry> = manager.branch(None).into_iter().cloned().collect();
            prepare_compaction(&branch, &self.compaction_settings())
        };
        let Some(preparation) = preparation else {
            return self.end_auto(reason, false, None);
        };
        match self
            .apply_compaction(&preparation, &model, api_key, headers, None, signal)
            .await
        {
            Ok(Applied::Cancelled) => self.end_auto(reason, true, None),
            Ok(Applied::Ok(result)) => {
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: Some(result),
                    aborted: false,
                    will_retry,
                    error_message: None,
                });
                if will_retry {
                    let mut messages = self.messages();
                    if matches!(
                        messages.last(),
                        Some(AgentMessage::Assistant(a)) if a.stop_reason == StopReason::Error
                    ) {
                        messages.pop();
                        self.agent().set_messages(messages);
                    }
                    self.continue_agent(std::time::Duration::from_millis(100));
                } else if self.agent().has_queued_messages() {
                    // Deliver follow-up/steering messages that queued meanwhile.
                    self.continue_agent(std::time::Duration::from_millis(100));
                }
            }
            Err(error) => {
                let prefix = match reason {
                    CompactionReason::Overflow => "Context overflow recovery failed",
                    _ => "Auto-compaction failed",
                };
                self.end_auto(reason, false, Some(format!("{prefix}: {error}")));
            }
        }
    }
}
