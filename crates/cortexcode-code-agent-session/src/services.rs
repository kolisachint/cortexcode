//! `core/agent-session-services.ts` and the session-building core of `sdk.ts`
//! (`createAgentSession`): the agent's hooks (auth per request, context GC,
//! blocked images), model and thinking-level restore, and the built-in tools.
//!
//! Not yet: the default resource loader (10.5), auth storage and
//! `findInitialModel` (10.4b), extension flags and provider registration (12.3).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cortexcode_agent_core::{
    Agent, AgentOptions, ConvertToLlmFn, SharedStreamFn, TransformContextFn,
};
use cortexcode_agent_types::{AgentMessage, AgentState, AgentTools, PermissionGate};
use cortexcode_ai_types::{Content, Message, Model, ThinkingDisplay, ThinkingLevel};
use cortexcode_code_models::{
    find_initial_model, AuthLookup, InitialModelOptions, ModelRegistry, RegistryWithAuth,
};
use cortexcode_code_session::{FileEntry, SessionManager};
use cortexcode_code_settings::SettingsManager;
use cortexcode_code_tool_api::ToolDefinition;
use cortexcode_code_tool_bash::BashToolOptions;
use cortexcode_code_tools_fs::ReadToolOptions;

use crate::auth_guidance::format_no_models_available_message;
use crate::hooks::{ExtensionHooks, ResourceLoader, SessionStartEvent};
use crate::session::{
    AgentSession, AgentSessionConfig, BaseTools, BaseToolsContext, ScopedModel,
    DEFAULT_ACTIVE_TOOL_NAMES, DEFAULT_THINKING_LEVEL,
};

/// `AgentSessionRuntimeDiagnostic`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionRuntimeDiagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    Info,
    Warning,
    Error,
}

/// `AgentSessionServices`: what every session in a process shares.
#[derive(Clone)]
pub struct AgentSessionServices {
    pub cwd: PathBuf,
    pub agent_dir: PathBuf,
    pub auth: Arc<dyn AuthLookup + Send + Sync>,
    pub settings: Arc<Mutex<SettingsManager>>,
    pub model_registry: Arc<ModelRegistry>,
    pub resource_loader: Arc<dyn ResourceLoader>,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
}

/// `createAgentSessionServices`: settings and models from the agent dir.
pub fn create_agent_session_services(
    cwd: PathBuf,
    auth: Arc<dyn AuthLookup + Send + Sync>,
    resource_loader: Arc<dyn ResourceLoader>,
) -> AgentSessionServices {
    let agent_dir = cortexcode_code_paths::agent_dir();
    let settings = SettingsManager::create(&cwd, &agent_dir);
    let model_registry = ModelRegistry::create(agent_dir.join("models.json"));
    AgentSessionServices {
        cwd,
        agent_dir,
        auth,
        settings: Arc::new(Mutex::new(settings)),
        model_registry: Arc::new(model_registry),
        resource_loader,
        diagnostics: Vec::new(),
    }
}

/// `noTools`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoTools {
    /// No tools at all (`--no-tools`).
    All,
    /// No built-in tools active; extension and SDK tools stay.
    Builtin,
}

/// `CreateAgentSessionFromServicesOptions` (+ the cortex-only hooks).
#[derive(Default)]
pub struct CreateAgentSessionOptions {
    /// Default: restored from the session, else the `defaultProvider` /
    /// `defaultModel` settings when auth is configured.
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub scoped_models: Vec<ScopedModel>,
    /// Tool allowlist (`--tools`).
    pub tools: Option<Vec<String>>,
    pub no_tools: Option<NoTools>,
    pub disallowed_tools: Option<Vec<String>>,
    pub custom_tools: Vec<ToolDefinition>,
    pub enable_web_tools: bool,
    /// Default: [`default_base_tools`].
    pub base_tools: Option<BaseTools>,
    pub extensions: Option<Arc<dyn ExtensionHooks>>,
    /// cortex: the gate consulted before each tool call.
    pub permission_gate: Option<Arc<dyn PermissionGate>>,
    /// Default: the API registry's `stream_simple`.
    pub stream_fn: Option<SharedStreamFn>,
    /// Default: `startup`.
    pub session_start_event: Option<SessionStartEvent>,
}

/// `CreateAgentSessionResult`.
pub struct CreatedAgentSession {
    pub session: AgentSession,
    /// Why the requested or restored model could not be used, if so.
    pub model_fallback_message: Option<String>,
}

/// `createAllToolDefinitions` for the default session: read, bash, edit,
/// write and SearchCodebase with their settings.
pub fn default_base_tools(ctx: &BaseToolsContext<'_>) -> Vec<ToolDefinition> {
    let settings = ctx.settings;
    let read = ReadToolOptions {
        auto_resize_images: settings.image_auto_resize(),
        max_output_bytes: settings.tool_output_max_bytes() as usize,
        max_output_lines: settings.tool_output_max_lines() as usize,
        dedup_reads: settings.context_gc_enabled(),
        ..Default::default()
    };
    let bash = BashToolOptions {
        command_prefix: settings.shell_command_prefix(),
        shell_path: settings.shell_path(),
        max_output_bytes: Some(settings.tool_output_max_bytes() as usize),
        max_output_lines: Some(settings.tool_output_max_lines() as usize),
        ..Default::default()
    };
    cortexcode_code_tools::default_tool_definitions(
        ctx.cwd.to_path_buf(),
        cortexcode_code_tools::permissions::PermissionPolicy::default(),
        read,
        bash,
    )
}

fn is_truthy_env_flag(value: &str) -> bool {
    value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
}

/// `isInstallTelemetryEnabled`: `*_TELEMETRY`, else `enableInstallTelemetry`.
fn is_install_telemetry_enabled(settings: &SettingsManager) -> bool {
    match cortexcode_code_paths::env_override("TELEMETRY") {
        Some(value) => is_truthy_env_flag(&value),
        None => settings.enable_install_telemetry(),
    }
}

/// `getAttributionHeaders`: OpenRouter app attribution when telemetry is on.
fn attribution_headers(
    model: &Model,
    settings: &SettingsManager,
) -> Option<std::collections::HashMap<String, String>> {
    if !is_install_telemetry_enabled(settings) {
        return None;
    }
    if model.provider == "openrouter" || model.base_url.contains("openrouter.ai") {
        return Some(
            [
                ("HTTP-Referer", "https://github.com/kolisachint/cortexcode"),
                ("X-OpenRouter-Title", cortexcode_code_paths::APP_NAME),
                ("X-OpenRouter-Categories", "cli-agent"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        );
    }
    None
}

type Headers = std::collections::HashMap<String, String>;

/// Attribution defaults, then the provider's (models.json) headers, then the
/// request's own: later layers win. `None` when there are none at all.
fn merge_request_headers(
    attribution: Option<Headers>,
    provider: Option<Headers>,
    request: Option<Headers>,
) -> Option<Headers> {
    if attribution.is_none() && provider.is_none() && request.is_none() {
        return None;
    }
    let mut headers = attribution.unwrap_or_default();
    headers.extend(provider.unwrap_or_default());
    headers.extend(request.unwrap_or_default());
    Some(headers)
}

const IMAGE_DISABLED: &str = "Image reading is disabled.";

/// `convertToLlm` with `images.blockImages`: images become a placeholder
/// (consecutive placeholders collapse). Read per call so changes apply live.
fn block_images(messages: Vec<Message>) -> Vec<Message> {
    let filter = |content: &mut Vec<Content>| {
        if !content.iter().any(|c| matches!(c, Content::Image(_))) {
            return;
        }
        let mut out: Vec<Content> = Vec::with_capacity(content.len());
        for block in content.drain(..) {
            let block = match block {
                Content::Image(_) => Content::text(IMAGE_DISABLED),
                other => other,
            };
            let is_placeholder = matches!(&block, Content::Text(t) if t.text == IMAGE_DISABLED);
            let prev_placeholder =
                matches!(out.last(), Some(Content::Text(t)) if t.text == IMAGE_DISABLED);
            if !(is_placeholder && prev_placeholder) {
                out.push(block);
            }
        }
        *content = out;
    };
    messages
        .into_iter()
        .map(|mut message| {
            match &mut message {
                Message::User(user) => {
                    if let cortexcode_ai_types::UserContent::Blocks(blocks) = &mut user.content {
                        filter(blocks);
                    }
                }
                Message::ToolResult(result) => filter(&mut result.content),
                Message::Assistant(_) => {}
            }
            message
        })
        .collect()
}

fn to_ai_thinking_display(display: cortexcode_code_settings::ThinkingDisplay) -> ThinkingDisplay {
    match display {
        cortexcode_code_settings::ThinkingDisplay::Summarized => ThinkingDisplay::Summarized,
        cortexcode_code_settings::ThinkingDisplay::Omitted => ThinkingDisplay::Omitted,
    }
}

fn to_agent_queue_mode(
    mode: cortexcode_code_settings::QueueMode,
) -> cortexcode_agent_core::QueueMode {
    match mode {
        cortexcode_code_settings::QueueMode::All => cortexcode_agent_core::QueueMode::All,
        cortexcode_code_settings::QueueMode::OneAtATime => {
            cortexcode_agent_core::QueueMode::OneAtATime
        }
    }
}

fn parse_thinking_level(value: &str) -> Option<ThinkingLevel> {
    cortexcode_code_settings::ThinkingLevelSetting::parse(value).map(ThinkingLevel::from)
}

/// `createAgentSession` over shared services.
pub fn create_agent_session(
    services: &AgentSessionServices,
    mut session_manager: SessionManager,
    options: CreateAgentSessionOptions,
) -> CreatedAgentSession {
    let settings = services.settings.clone();
    let registry = services.model_registry.clone();
    let auth = services.auth.clone();
    let has_auth = |model: &Model| registry.has_configured_auth(model, auth.as_ref());

    let existing = session_manager.build_context();
    let has_existing_session = !existing.messages.is_empty();
    let has_thinking_entry = session_manager
        .branch(None)
        .iter()
        .any(|e| matches!(e, FileEntry::ThinkingLevelChange { .. }));

    let mut model = options.model;
    let mut model_fallback_message = None;
    if model.is_none() && has_existing_session {
        if let Some(restored) = &existing.model {
            model = registry
                .find(&restored.provider, &restored.model_id)
                .filter(|m| has_auth(m))
                .cloned();
            if model.is_none() {
                model_fallback_message = Some(format!(
                    "Could not restore model {}/{}",
                    restored.provider, restored.model_id
                ));
            }
        }
    }
    if model.is_none() {
        // findInitialModel: the settings default when it has auth, else the first
        // available known-provider default, else the first available model.
        // (hoocode also falls back to hoo-config.json `llm.default_provider` /
        // `default_model`; that file is not read by cortex.)
        let (default_provider, default_model_id, default_thinking_level) = {
            let s = lock(&settings);
            (
                s.default_provider(),
                s.default_model(),
                s.default_thinking_level().map(ThinkingLevel::from),
            )
        };
        let source = RegistryWithAuth {
            registry: &registry,
            auth: auth.as_ref(),
        };
        model = find_initial_model(
            InitialModelOptions {
                is_continuing: has_existing_session,
                default_provider: default_provider.as_deref(),
                default_model_id: default_model_id.as_deref(),
                default_thinking_level,
                ..Default::default()
            },
            &source,
        )
        .ok()
        .and_then(|r| r.model);
        match (&model, &mut model_fallback_message) {
            (None, message) => *message = Some(format_no_models_available_message()),
            (Some(m), Some(message)) => {
                message.push_str(&format!(". Using {}/{}", m.provider, m.id))
            }
            (Some(_), None) => {}
        }
    }

    let default_thinking = || {
        lock(&settings)
            .default_thinking_level()
            .map(ThinkingLevel::from)
            .unwrap_or(DEFAULT_THINKING_LEVEL)
    };
    let mut thinking_level = options.thinking_level;
    if thinking_level.is_none() && has_existing_session {
        thinking_level = Some(if has_thinking_entry {
            parse_thinking_level(&existing.thinking_level).unwrap_or(DEFAULT_THINKING_LEVEL)
        } else {
            default_thinking()
        });
    }
    let thinking_level = thinking_level.unwrap_or_else(default_thinking);
    let thinking_level = match &model {
        Some(model) => cortexcode_ai_models::clamp_thinking_level(model, &thinking_level),
        None => ThinkingLevel::Off,
    };

    let mut initial_active: Vec<String> = DEFAULT_ACTIVE_TOOL_NAMES.map(String::from).to_vec();
    if options.enable_web_tools {
        initial_active.extend(["webfetch".to_string(), "websearch".to_string()]);
    }
    let allowed_tool_names = options
        .tools
        .clone()
        .or_else(|| (options.no_tools == Some(NoTools::All)).then(Vec::new));
    let initial_active_tool_names = match (&options.tools, options.no_tools) {
        (Some(tools), _) => tools.clone(),
        (None, Some(_)) => Vec::new(),
        (None, None) => initial_active,
    };

    // convertToLlm: harness messages to LLM messages, then blocked images.
    let convert_settings = settings.clone();
    let convert_to_llm: ConvertToLlmFn = Arc::new(move |messages: Vec<AgentMessage>| {
        let converted = cortexcode_agent_harness::messages::convert_to_llm(&messages);
        if !lock(&convert_settings).block_images() {
            return Ok(converted);
        }
        Ok(block_images(converted))
    });

    // Context GC on the outgoing copy, with latched budget pressure measured
    // against the agent's current model.
    let agent_slot: Arc<OnceLock<Weak<Agent>>> = Arc::new(OnceLock::new());
    let gc_settings = settings.clone();
    let gc_cwd = services.cwd.clone();
    let gc_agent = agent_slot.clone();
    let latch = Mutex::new(cortexcode_code_tools_fs::BudgetPressureLatch::new());
    let transform_context: TransformContextFn = Arc::new(move |messages, _signal| {
        if !lock(&gc_settings).context_gc_enabled() {
            return Ok(messages);
        }
        let context_window = gc_agent
            .get()
            .and_then(Weak::upgrade)
            .map_or(0, |agent| agent.with_state(|s| s.model.context_window));
        let tokens = cortexcode_agent_compaction::estimate_context_tokens(&messages).tokens;
        let budget_pressure = lock(&latch).update(tokens, context_window);
        let options = cortexcode_code_tools_fs::ContextGcOptions {
            cwd: gc_cwd.clone(),
            budget_pressure,
        };
        Ok(
            cortexcode_code_tools_fs::evict_superseded_reads(&messages, &options)
                .unwrap_or(messages),
        )
    });

    // Resolve auth per request (models.json keys and headers, attribution).
    let base_stream = options
        .stream_fn
        .unwrap_or_else(|| Arc::new(Box::new(cortexcode_ai_registry::stream_simple)));
    let stream_registry = registry.clone();
    let stream_auth = auth.clone();
    let stream_settings = settings.clone();
    let stream_fn: SharedStreamFn =
        Arc::new(Box::new(move |model, context, mut stream_options| {
            let request_auth = stream_registry
                .get_api_key_and_headers(&model, stream_auth.as_ref())
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
            let (retry, attribution) = {
                let s = lock(&stream_settings);
                (s.provider_retry_settings(), attribution_headers(&model, &s))
            };
            if let Some(key) = request_auth.api_key {
                stream_options.api_key = Some(key);
            }
            stream_options.timeout_ms = stream_options.timeout_ms.or(retry.timeout_ms);
            stream_options.max_retries = stream_options.max_retries.or(retry.max_retries);
            stream_options.max_retry_delay_ms = stream_options
                .max_retry_delay_ms
                .or(Some(retry.max_retry_delay_ms));
            stream_options.headers = merge_request_headers(
                attribution,
                request_auth.headers,
                stream_options.headers.take(),
            );
            base_stream(model, context, stream_options)
        }));

    let (steering_mode, follow_up_mode, transport, budgets, display, max_retry_delay_ms) = {
        let s = lock(&settings);
        (
            s.steering_mode(),
            s.follow_up_mode(),
            s.transport(),
            s.thinking_budgets(),
            s.thinking_display(),
            s.provider_retry_settings().max_retry_delay_ms,
        )
    };
    let placeholder_model = Agent::new().state().model;
    let agent = Arc::new(Agent::with_options(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: String::new(),
            model: model.clone().unwrap_or(placeholder_model),
            thinking_level: thinking_level.clone(),
            tools: AgentTools::new(vec![]),
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        convert_to_llm: Some(convert_to_llm),
        transform_context: Some(transform_context),
        stream_fn: Some(stream_fn),
        session_id: Some(session_manager.session_id().to_string()),
        steering_mode: Some(to_agent_queue_mode(steering_mode)),
        follow_up_mode: Some(to_agent_queue_mode(follow_up_mode)),
        transport: Some(transport),
        thinking_budgets: budgets,
        thinking_display: display.map(to_ai_thinking_display),
        max_retry_delay_ms: Some(max_retry_delay_ms),
        permission_gate: options.permission_gate,
        ..Default::default()
    }));
    let _ = agent_slot.set(Arc::downgrade(&agent));

    if has_existing_session {
        agent.set_messages(existing.messages);
        if !has_thinking_entry {
            session_manager
                .append_thinking_level_change(crate::session::thinking_level_str(&thinking_level));
        }
    } else {
        if let Some(model) = &model {
            session_manager.append_model_change(&model.provider, &model.id);
        }
        session_manager
            .append_thinking_level_change(crate::session::thinking_level_str(&thinking_level));
    }

    let session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings,
        cwd: services.cwd.clone(),
        scoped_models: options.scoped_models,
        resource_loader: services.resource_loader.clone(),
        custom_tools: options.custom_tools,
        model_registry: registry,
        auth,
        initial_active_tool_names: Some(initial_active_tool_names),
        allowed_tool_names,
        disallowed_tool_names: options.disallowed_tools,
        base_tools: options
            .base_tools
            .unwrap_or_else(|| BaseTools::Factory(Arc::new(default_base_tools))),
        extensions: options.extensions,
        session_start_event: options.session_start_event,
    });
    CreatedAgentSession {
        session,
        model_fallback_message,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::{ImageContent, TextContent, ToolResultMessage, UserMessage};

    fn image() -> Content {
        Content::Image(ImageContent {
            data: "ZmFrZQ==".into(),
            media_type: "image/png".into(),
        })
    }

    #[test]
    fn blocked_images_become_one_placeholder_per_run() {
        let messages = vec![
            Message::User(UserMessage {
                content: vec![
                    Content::text("look"),
                    image(),
                    image(),
                    Content::text("x"),
                    image(),
                ]
                .into(),
                timestamp: 0,
            }),
            Message::ToolResult(ToolResultMessage {
                content: vec![image()],
                ..Default::default()
            }),
        ];
        let out = block_images(messages);
        let texts = |c: &[Content]| -> Vec<String> {
            c.iter()
                .map(|b| match b {
                    Content::Text(TextContent { text, .. }) => text.clone(),
                    _ => "<other>".into(),
                })
                .collect()
        };
        let Message::User(user) = &out[0] else {
            panic!()
        };
        assert_eq!(
            texts(&user.content.blocks()),
            ["look", IMAGE_DISABLED, "x", IMAGE_DISABLED]
        );
        let Message::ToolResult(result) = &out[1] else {
            panic!()
        };
        assert_eq!(texts(&result.content), [IMAGE_DISABLED]);
    }

    #[test]
    fn truthy_env_flags() {
        for v in ["1", "true", "TRUE", "yes", "Yes"] {
            assert!(is_truthy_env_flag(v));
        }
        for v in ["", "0", "no", "on"] {
            assert!(!is_truthy_env_flag(v));
        }
    }

    // Ports of `coding-agent/test/sdk-openrouter-attribution.test.ts`
    // (the TS drives the same logic through `createAgentSession`'s streamFn).

    fn model(provider: &str, base_url: &str) -> Model {
        Model {
            id: format!("{provider}-test-model"),
            name: format!("{provider} Test Model"),
            api: "openai-completions".into(),
            provider: provider.into(),
            base_url: base_url.into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: Default::default(),
            context_window: 128_000,
            max_tokens: 4096,
            headers: None,
            compat: None,
        }
    }

    fn telemetry(enabled: bool) -> SettingsManager {
        let mut settings = serde_json::Map::new();
        settings.insert("enableInstallTelemetry".into(), enabled.into());
        SettingsManager::in_memory(settings)
    }

    fn assert_default_attribution(headers: Option<Headers>) {
        let h = headers.expect("attribution headers");
        assert_eq!(
            h["HTTP-Referer"],
            "https://github.com/kolisachint/cortexcode"
        );
        assert_eq!(h["X-OpenRouter-Title"], cortexcode_code_paths::APP_NAME);
        assert_eq!(h["X-OpenRouter-Categories"], "cli-agent");
    }

    #[test]
    fn adds_default_attribution_headers_for_openrouter_models() {
        let m = model("openrouter", "https://openrouter.ai/api/v1");
        assert_default_attribution(attribution_headers(&m, &telemetry(true)));
    }

    #[test]
    fn no_attribution_headers_when_telemetry_is_disabled() {
        let m = model("openrouter", "https://openrouter.ai/api/v1");
        assert_eq!(attribution_headers(&m, &telemetry(false)), None);
    }

    #[test]
    fn adds_attribution_headers_for_custom_providers_routed_through_openrouter() {
        let m = model("custom-openrouter", "https://openrouter.ai/api/v1");
        assert_default_attribution(attribution_headers(&m, &telemetry(true)));
    }

    #[test]
    fn provider_and_request_headers_override_the_defaults() {
        let m = model("openrouter", "https://openrouter.ai/api/v1");
        let map = |pairs: &[(&str, &str)]| -> Headers {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let h = merge_request_headers(
            attribution_headers(&m, &telemetry(true)),
            Some(map(&[
                ("HTTP-Referer", "https://provider.example"),
                ("X-OpenRouter-Categories", "provider-category"),
            ])),
            Some(map(&[("X-OpenRouter-Title", "request-title")])),
        )
        .unwrap();
        assert_eq!(h["HTTP-Referer"], "https://provider.example");
        assert_eq!(h["X-OpenRouter-Title"], "request-title");
        assert_eq!(h["X-OpenRouter-Categories"], "provider-category");
        assert_eq!(merge_request_headers(None, None, None), None);
    }
}
