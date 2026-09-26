//! Runtime glue between the `cortex` CLI and the agent namespace.
//!
//! This module builds an `Agent` from CLI arguments, wires up the default
//! coding tools, and dispatches to print or interactive mode. It is the
//! integration point that turns the previously stubbed CLI commands into
//! actual LLM-backed sessions.

use crate::Args;
use cortexcode_agent_core::PromptInput;
use cortexcode_agent_core::{Agent, AgentOptions, Subscription};
use cortexcode_agent_types::{AgentEvent, AgentMessage, AgentState, PermissionGate};
use cortexcode_ai_env::get_env_api_key;
use cortexcode_ai_types::{Content, Message, TextContent, UserMessage};
use cortexcode_ai_types::{Context, Model as AiModel, SimpleStreamOptions};

use cortexcode_code_print::{format_text_output, text_result, PrintFormatter, PrintMode};
use cortexcode_code_prompts::BuildSystemPromptOptions;
use cortexcode_code_settings::SettingsManager;
use cortexcode_code_tool_api::{
    wrap_tool_definitions, SessionBranch, ToolContext, ToolContextFactory, ToolDefinition,
};
use cortexcode_code_tool_bash::BashToolOptions;
use cortexcode_code_tools::{permissions::PermissionPolicy, PolicyPermissionGate};
use cortexcode_code_tools_fs::ReadToolOptions;

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Function type for creating an AI stream.
type StreamFn = Box<
    dyn Fn(
            AiModel,
            Context,
            SimpleStreamOptions,
        ) -> Result<
            cortexcode_ai_stream::AssistantMessageEventStream,
            Box<dyn std::error::Error + Send + Sync>,
        > + Send
        + Sync,
>;
use std::io::Write;
use std::sync::Arc;

/// Error type for runtime operations.
#[derive(Debug)]
pub enum RuntimeError {
    Setup(String),
    Agent(String),
    Print(cortexcode_code_print::PrintError),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::Setup(e) => write!(f, "setup error: {}", e),
            RuntimeError::Agent(e) => write!(f, "agent error: {}", e),
            RuntimeError::Print(e) => write!(f, "print error: {}", e),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<cortexcode_code_print::PrintError> for RuntimeError {
    fn from(e: cortexcode_code_print::PrintError) -> Self {
        RuntimeError::Print(e)
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for RuntimeError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        RuntimeError::Agent(e.to_string())
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(e: std::io::Error) -> Self {
        RuntimeError::Print(e.into())
    }
}

/// Resolve the provider and model from CLI arguments, falling back to the
/// `defaultProvider` / `defaultModel` settings and finally to hardcoded
/// defaults.
fn resolve_provider_model(args: &Args, settings: &SettingsManager) -> (String, String) {
    let provider = args
        .provider
        .clone()
        .or_else(|| settings.default_provider())
        .unwrap_or_else(|| "anthropic".to_string());
    let model = args
        .model
        .clone()
        .or_else(|| {
            // Only trust the default model if it was paired with the same
            // provider (or no provider override was requested at all).
            if args.provider.is_none()
                || args.provider.as_deref() == settings.default_provider().as_deref()
            {
                settings.default_model()
            } else {
                None
            }
        })
        .unwrap_or_else(|| default_model_for_provider(&provider));
    (provider, model)
}

fn default_model_for_provider(provider: &str) -> String {
    match provider {
        "anthropic" => "claude-sonnet-4-5".to_string(),
        "openai" => "gpt-4o".to_string(),
        "opencode" | "opencode-go" => "mimo-v2.5-free".to_string(),
        "google" => "gemini-2.5-pro".to_string(),
        "azure" => "gpt-4o".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Resolve the API key for the provider: CLI flag, then the environment,
/// then a stored OAuth token (auth.json precedence arrives with 10.4b).
fn resolve_api_key(provider: &str, args: &Args) -> Option<String> {
    if let Some(key) = &args.api_key {
        return Some(key.clone());
    }
    if let Some(key) = get_env_api_key(provider) {
        return Some(key);
    }
    oauth_api_key(provider)
}

/// Fall back to an OAuth access token persisted by the OAuth login flow (`auth::login`),
/// refreshing it first if it has expired.
fn oauth_api_key(provider: &str) -> Option<String> {
    let store_key = match provider {
        "anthropic" | "claude" => "anthropic",
        "github-copilot" | "github" | "copilot" => "github-copilot",
        "openai-codex" => "openai-codex",
        "google-gemini-cli" => "google-gemini-cli",
        "google-antigravity" => "google-antigravity",
        _ => return None,
    };
    let store = crate::auth::CredentialStore::default_location();
    let credentials = store.get(store_key)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    // `getApiKey`: the Google providers send `{token, projectId}`.
    let api_key = |credentials: &cortexcode_ai_oauth::OAuthCredentials| match store_key {
        "google-gemini-cli" | "google-antigravity" => {
            cortexcode_ai_oauth_google::google_api_key(credentials)
        }
        _ => credentials.access.clone(),
    };
    if !credentials.is_expired(now) {
        return Some(api_key(&credentials));
    }

    // Expired: attempt a refresh, persisting the new tokens on success.
    let refreshed = match store_key {
        "anthropic" => async_runtime()
            .block_on(cortexcode_ai_oauth_anthropic::refresh_anthropic_token(
                &cortexcode_ai_oauth::ReqwestFetch,
                &credentials.refresh,
            ))
            .ok(),
        "github-copilot" => async_runtime()
            .block_on(
                cortexcode_ai_oauth_github_copilot::refresh_github_copilot_token(
                    &cortexcode_ai_oauth::ReqwestFetch,
                    &credentials.refresh,
                    credentials.extra_str("enterpriseUrl"),
                ),
            )
            .ok(),
        "openai-codex" => async_runtime()
            .block_on(
                cortexcode_ai_oauth_openai_codex::refresh_openai_codex_token(
                    &cortexcode_ai_oauth::ReqwestFetch,
                    &credentials.refresh,
                ),
            )
            .ok(),
        "google-gemini-cli" | "google-antigravity" => {
            let fetch = cortexcode_ai_oauth::ReqwestFetch;
            let project_id = credentials.extra_str("projectId").unwrap_or_default();
            let refresh = if store_key == "google-gemini-cli" {
                async_runtime().block_on(cortexcode_ai_oauth_google::refresh_google_cloud_token(
                    &fetch,
                    &credentials.refresh,
                    project_id,
                ))
            } else {
                async_runtime().block_on(cortexcode_ai_oauth_google::refresh_antigravity_token(
                    &fetch,
                    &credentials.refresh,
                    project_id,
                ))
            };
            refresh.ok()
        }
        _ => None,
    };
    match refreshed {
        Some(fresh) => {
            let _ = store.save(store_key, &fresh);
            Some(api_key(&fresh))
        }
        // Refresh failed (offline, revoked); fall back to the stale token.
        None => Some(api_key(&credentials)),
    }
}

/// `resolvePromptInput`: a value naming an existing file means its contents.
fn resolve_prompt_input(input: Option<&str>, description: &str) -> Option<String> {
    let input = input.filter(|s| !s.is_empty())?;
    if std::path::Path::new(input).exists() {
        return match std::fs::read_to_string(input) {
            Ok(content) => Some(content),
            Err(e) => {
                eprintln!(
                    "\x1b[33mWarning: Could not read {description} file {input}: {e}\x1b[39m"
                );
                Some(input.to_string())
            }
        };
    }
    Some(input.to_string())
}

/// `AgentSession._rebuildSystemPrompt`: the built-in prompt (or `--system-prompt`)
/// over the active tools' snippets and guidelines. Skills and context files
/// (10.5), agents (10.9) and shipped docs are not loaded yet.
fn build_system_prompt(
    args: &Args,
    cwd: &std::path::Path,
    tools: &[ToolDefinition],
    light: bool,
) -> String {
    // main.ts: `systemPrompt: parsed.systemPrompt ?? (lightMode ? LIGHT_SYSTEM_PROMPT : undefined)`
    let system_prompt_source = args
        .system_prompt
        .as_deref()
        .or(light.then_some(cortexcode_code_prompts::LIGHT_SYSTEM_PROMPT));
    let mut tool_snippets = Vec::new();
    let mut prompt_guidelines = Vec::new();
    for tool in tools {
        if let Some(snippet) = &tool.prompt_snippet {
            tool_snippets.push((tool.name.clone(), snippet.clone()));
        }
        prompt_guidelines.extend(tool.prompt_guidelines.iter().cloned());
    }
    cortexcode_code_prompts::build_system_prompt(&BuildSystemPromptOptions {
        custom_prompt: resolve_prompt_input(system_prompt_source, "system prompt"),
        selected_tools: Some(tools.iter().map(|t| t.name.clone()).collect()),
        tool_snippets,
        prompt_guidelines,
        cwd: cwd.to_string_lossy().into_owned(),
        ..Default::default()
    })
}

/// The messages of this run as the session manager would persist them: each
/// one is appended on `message_end`. Stands in for the session branch that
/// tools see (read-dedup) until the session port (10.3).
#[derive(Default)]
struct LiveTranscript(std::sync::Mutex<Vec<serde_json::Value>>);

impl LiveTranscript {
    /// Record every message the agent ends. Keep the returned handle alive.
    fn follow(self: &Arc<Self>, agent: &Agent) -> Subscription {
        let transcript = self.clone();
        agent.subscribe(move |event, _signal| {
            if let AgentEvent::MessageEnd { message } = event {
                if let Ok(value) = serde_json::to_value(message) {
                    transcript
                        .0
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(value);
                }
            }
        })
    }
}

impl SessionBranch for LiveTranscript {
    fn get_branch(&self) -> Vec<serde_json::Value> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The default coding tools as definitions. The read tool takes its caps from
/// `toolOutput`, image resizing from `images.autoResize`, and read-dedup from
/// `contextGc.enabled`; bash takes `shellCommandPrefix`, `shellPath` and the
/// same caps.
fn build_tool_definitions(
    cwd: &std::path::Path,
    settings: &SettingsManager,
) -> Vec<ToolDefinition> {
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
        cwd.to_path_buf(),
        PermissionPolicy::default(),
        read,
        bash,
    )
}

/// Wrap the definitions for the agent loop; tools see the model and the live
/// transcript through their context.
fn wrap_tools(
    definitions: Vec<ToolDefinition>,
    model: &AiModel,
    transcript: Arc<LiveTranscript>,
) -> Vec<cortexcode_agent_types::AgentTool> {
    let model = model.clone();
    let ctx_factory: ToolContextFactory = Arc::new(move || ToolContext {
        model: Some(model.clone()),
        session_manager: Some(transcript.clone()),
    });
    wrap_tool_definitions(definitions, Some(ctx_factory))
}

/// Build the permission gate for the current CLI mode. Read-only tools are
/// always auto-approved.
fn build_permission_gate(interactive: bool) -> Arc<dyn PermissionGate> {
    if interactive {
        let inner = Arc::new(crate::permission_dialog::InteractivePermissionGate);
        return Arc::new(PolicyPermissionGate::new(
            PermissionPolicy::Ask,
            true,
            Some(inner),
        ));
    }

    // Non-interactive (print) mode: auto-approve dangerous tools.
    Arc::new(PolicyPermissionGate::new(
        PermissionPolicy::Auto,
        true,
        None,
    ))
}

/// Create the streaming function for a given provider.
/// Streams are dispatched on `model.api` through the API registry (ledger 8.2a),
/// so any provider whose models use a registered API works.
fn make_stream_fn() -> StreamFn {
    Box::new(cortexcode_ai_registry::stream_simple)
}

/// Build an `Agent` from CLI arguments with a configured permission gate.
/// Build the agent. The returned subscription keeps the tools' live
/// transcript current; hold it as long as the agent.
fn build_agent_with_gate(
    args: &Args,
    interactive: bool,
) -> Result<(Agent, Subscription), RuntimeError> {
    let settings = crate::load_settings();
    let (provider, model_id) = resolve_provider_model(args, &settings);
    // Built-in catalog + models.json custom providers/overrides (ledger 10.4a).
    let registry = match cortexcode_code_models::default_models_json_path() {
        Some(path) => cortexcode_code_models::ModelRegistry::create(path),
        None => cortexcode_code_models::ModelRegistry::in_memory(),
    };
    let mut model = registry
        .find(&provider, &model_id)
        .cloned()
        .ok_or_else(|| RuntimeError::Setup(format!("unknown model {}:{}", provider, model_id)))?;

    // CLI/config/env/OAuth first (existing behavior), then models.json request auth.
    // Full auth.json precedence arrives with code-auth (ledger 10.4b).
    let request_auth = registry
        .get_api_key_and_headers(&model, &cortexcode_code_models::NoAuth)
        .map_err(RuntimeError::Setup)?;
    if let Some(headers) = request_auth.headers {
        model
            .headers
            .get_or_insert_with(Default::default)
            .extend(headers);
    }
    let api_key = resolve_api_key(&provider, args).or(request_auth.api_key);
    if api_key.is_none() {
        let supported = ["anthropic", "openai", "opencode", "google", "azure"];
        let is_known = supported.contains(&provider.as_str());
        let hint = if is_known {
            format!(
                "No API key for provider '{}'.\n\
                 Set the environment variable:\n\
                   export {}_API_KEY=your-key-here",
                provider,
                provider.to_uppercase()
            )
        } else {
            let list = supported.join(", ");
            format!(
                "Provider '{}' is not supported. Use one of: {}\n\
                 Set \"defaultProvider\" in ~/.cortexcode/settings.json to one of the above,\n\
                 then set the corresponding API key, e.g.:\n\
                   export ANTHROPIC_API_KEY=your-key-here",
                provider, list
            )
        };
        return Err(RuntimeError::Setup(hint));
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    // Light preset (--light, else the "light" setting): the four light tools
    // and the terse prompt.
    let light = args.light.unwrap_or_else(|| settings.light());
    let definitions = if light {
        cortexcode_code_tools::light::light_tool_definitions(cwd.clone())
    } else {
        build_tool_definitions(&cwd, &settings)
    };
    let system_prompt = build_system_prompt(args, &cwd, &definitions, light);
    let transcript = Arc::new(LiveTranscript::default());
    let tools = wrap_tools(definitions, &model, transcript.clone());

    let state = AgentState {
        system_prompt,
        model,
        thinking_level: cortexcode_ai_types::ThinkingLevel::Off,
        tools: cortexcode_agent_types::AgentTools::new(tools),
        messages: Vec::new(),
        is_streaming: false,
        streaming_message: None,
        pending_tool_calls: std::collections::HashSet::new(),
        error_message: None,
    };

    let permission_gate = Some(build_permission_gate(interactive));
    let stream_fn = Some(std::sync::Arc::new(make_stream_fn()));

    let agent = Agent::with_options(AgentOptions {
        initial_state: Some(state),
        api_key,
        permission_gate,
        stream_fn,
        ..Default::default()
    });
    let subscription = transcript.follow(&agent);

    Ok((agent, subscription))
}

/// The tokio runtime the CLI drives async work on (agent runs, OAuth). Provider
/// streams spawn onto it (`spawn_producer` uses the current runtime).
pub(crate) fn async_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime")
    })
}

/// A user message as `session.prompt(text)` sends it: one text block, no wrapping.
fn text_message(text: &str) -> AgentMessage {
    AgentMessage::from_message(Message::User(UserMessage {
        content: vec![Content::Text(TextContent {
            text_signature: None,
            text: text.to_string(),
        })]
        .into(),
        timestamp: cortexcode_ai_types::now_ms(),
    }))
}

/// `runPrintMode` (print-mode.ts) plus the `prepareInitialMessage` step of
/// `main.ts`. Returns the process exit code.
pub fn run_print_mode(
    args: &Args,
    mode: PrintMode,
    stdin_content: Option<String>,
    color: bool,
    output: &mut dyn Write,
    err: &mut dyn Write,
) -> std::io::Result<i32> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let file_text = if args.file_args.is_empty() {
        None
    } else {
        match crate::initial_message::process_file_arguments(&args.file_args, &cwd, &home) {
            Ok(text) => Some(text),
            Err(message) => {
                writeln!(err, "{}", crate::red(color, &message))?;
                return Ok(1);
            }
        }
    };
    let mut messages = args.messages.clone();
    let initial_message = crate::initial_message::build_initial_message(
        &mut messages,
        file_text.as_deref(),
        stdin_content.as_deref(),
    );

    let (agent, _transcript) = match build_agent_with_gate(args, false) {
        Ok(built) => built,
        Err(e) => {
            writeln!(err, "{e}")?;
            return Ok(1);
        }
    };

    let formatter = std::sync::Arc::new(std::sync::Mutex::new(PrintFormatter::new(mode)));
    let formatter_for_sub = formatter.clone();
    let _sub = agent.subscribe(move |event, _signal| {
        if let Ok(mut fmt) = formatter_for_sub.lock() {
            fmt.record(event.clone());
        }
    });

    // `session.prompt(initialMessage)` then each remaining message in turn.
    for prompt in initial_message.iter().chain(messages.iter()) {
        let run = agent.prompt(PromptInput::Messages(vec![text_message(prompt)]));
        if let Err(e) = async_runtime().block_on(run) {
            writeln!(err, "{e}")?;
            return Ok(1);
        }
    }

    match mode {
        PrintMode::Text => {
            let result = text_result(&agent.state().messages);
            output.write_all(result.stdout.as_bytes())?;
            output.flush()?;
            if let Some(message) = result.stderr {
                writeln!(err, "{message}")?;
            }
            Ok(result.exit_code)
        }
        // Event-stream parity is 10.8b; json mode never inspects the final message.
        PrintMode::Json => {
            let formatter = std::sync::Arc::try_unwrap(formatter)
                .ok()
                .and_then(|m| m.into_inner().ok())
                .unwrap_or_default();
            formatter
                .finalize(output)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            Ok(0)
        }
    }
}

/// Run the agent in an interactive TUI loop.
pub fn run_interactive_mode(
    args: &Args,
    output: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), RuntimeError> {
    use crossterm::{
        cursor, event,
        style::{self, Stylize},
        terminal, QueueableCommand,
    };
    use std::io::Write as _;

    let (agent, _transcript) = build_agent_with_gate(args, true)?;
    let mut stdout = std::io::stdout();
    terminal::enable_raw_mode().map_err(|e| RuntimeError::Setup(e.to_string()))?;
    let _ = stdout
        .queue(terminal::Clear(terminal::ClearType::All))?
        .queue(cursor::MoveTo(0, 0))?
        .flush();

    writeln!(
        output,
        "{} Interactive Cortex mode. /quit or Ctrl+C to exit.",
        "TUI".bold()
    )?;

    let mut input = String::new();
    loop {
        let _ = stdout
            .queue(cursor::MoveToColumn(0))?
            .queue(terminal::Clear(terminal::ClearType::CurrentLine))?
            .queue(style::Print("cortex> "))?
            .queue(style::Print(&input))?
            .flush();

        if event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            if let Ok(event::Event::Key(key)) = event::read() {
                match key.code {
                    event::KeyCode::Enter => {
                        let line = input.trim();
                        if line == "/quit" {
                            break;
                        }
                        if !line.is_empty() {
                            writeln!(output, "\nYou: {}", line)?;
                            let user_msg = text_message(line);
                            let before = agent.state().messages.len();
                            let run = agent.prompt(PromptInput::Messages(vec![user_msg]));
                            match async_runtime().block_on(run) {
                                Ok(()) => {
                                    let messages = agent.state().messages;
                                    let text = format_text_output(&messages[before..]);
                                    if !text.is_empty() {
                                        writeln!(output, "Cortex: {}\n", text)?;
                                    } else {
                                        writeln!(output, "Cortex: (no response)\n")?;
                                    }
                                }
                                Err(e) => writeln!(output, "Cortex error: {}\n", e)?,
                            }
                        }
                        input.clear();
                    }
                    event::KeyCode::Char(c) => {
                        if key.modifiers == event::KeyModifiers::CONTROL && c == 'c' {
                            break;
                        }
                        input.push(c);
                    }
                    event::KeyCode::Backspace => {
                        input.pop();
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = terminal::disable_raw_mode();
    writeln!(err, "interactive session ended")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_mode_uses_the_terse_prompt_and_the_four_light_tools() {
        let args = crate::args::parse_args(&["--light".to_string()]);
        let cwd = std::path::Path::new("/w");
        let tools = cortexcode_code_tools::light::light_tool_definitions(cwd.to_path_buf());
        let prompt = build_system_prompt(&args, cwd, &tools, true);
        let date = chrono::Local::now().format("%Y-%m-%d");
        assert_eq!(
            prompt,
            format!(
                "{}\n\nCurrent date: {date}\nCurrent working directory: /w",
                cortexcode_code_prompts::LIGHT_SYSTEM_PROMPT
            )
        );
        // --system-prompt still wins over the preset.
        let args = crate::args::parse_args(&[
            "--light".to_string(),
            "--system-prompt".to_string(),
            "Custom.".to_string(),
        ]);
        assert!(
            build_system_prompt(&args, cwd, &tools, true).starts_with("Custom.\n\nCurrent date: ")
        );
    }

    #[test]
    fn default_prompt_lists_tools_with_snippets() {
        let args = crate::args::parse_args(&[]);
        let cwd = std::path::Path::new("/w");
        let prompt = build_system_prompt(
            &args,
            cwd,
            &build_tool_definitions(cwd, &SettingsManager::in_memory(Default::default())),
            false,
        );
        assert!(prompt.starts_with("You are an expert coding assistant operating inside cortex"));
        assert!(prompt.contains(
            "Available tools:\n- read: Read file contents\n- bash: Run builds, tests, linters, git, and package managers\n\nGuidelines:"
        ));
    }

    #[test]
    fn test_default_model_for_provider() {
        assert!(!default_model_for_provider("anthropic").is_empty());
    }

    #[test]
    fn test_text_message() {
        let msg = text_message("hello");
        assert!(msg.extract_message().is_some());
    }

    fn settings(provider: Option<&str>, model: Option<&str>) -> SettingsManager {
        let mut map = serde_json::Map::new();
        if let Some(provider) = provider {
            map.insert("defaultProvider".into(), provider.into());
        }
        if let Some(model) = model {
            map.insert("defaultModel".into(), model.into());
        }
        SettingsManager::in_memory(map)
    }

    #[test]
    fn test_resolve_provider_model_cli_args_win_over_settings() {
        let args = Args {
            provider: Some("openai".into()),
            model: Some("gpt-4".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_provider_model(&args, &settings(Some("anthropic"), Some("claude-sonnet-4"))),
            ("openai".to_string(), "gpt-4".to_string())
        );
    }

    #[test]
    fn test_resolve_provider_model_falls_back_to_settings() {
        assert_eq!(
            resolve_provider_model(
                &Args::default(),
                &settings(Some("anthropic"), Some("claude-opus-4"))
            ),
            ("anthropic".to_string(), "claude-opus-4".to_string())
        );
    }

    #[test]
    fn test_resolve_provider_model_ignores_mismatched_default_model() {
        // The default model belongs to a different provider than the one
        // requested on the CLI, so it must not leak across providers.
        let args = Args {
            provider: Some("openai".into()),
            ..Default::default()
        };
        let (provider, model) =
            resolve_provider_model(&args, &settings(Some("anthropic"), Some("claude-opus-4")));
        assert_eq!(provider, "openai");
        assert_eq!(model, default_model_for_provider("openai"));
    }

    #[test]
    fn test_resolve_provider_model_no_args_no_settings_uses_defaults() {
        let (provider, model) = resolve_provider_model(&Args::default(), &settings(None, None));
        assert_eq!(provider, "anthropic");
        assert_eq!(model, default_model_for_provider("anthropic"));
    }

    #[test]
    fn test_resolve_api_key_cli_arg_wins() {
        let args = Args {
            api_key: Some("cli-key".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_api_key("anthropic", &args),
            Some("cli-key".to_string())
        );
    }
}
