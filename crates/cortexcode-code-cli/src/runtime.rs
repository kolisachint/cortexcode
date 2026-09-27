//! Runtime glue between the `cortex` CLI and the agent namespace.
//!
//! This module builds an `Agent` from CLI arguments, wires up the default
//! coding tools, and dispatches to print or interactive mode. It is the
//! integration point that turns the previously stubbed CLI commands into
//! actual LLM-backed sessions.

use crate::Args;
use cortexcode_agent_types::PermissionGate;
use cortexcode_ai_env::get_env_api_key;

use cortexcode_code_agent_session::{
    create_agent_session, AgentSession, AgentSessionEvent, AgentSessionServices, BaseTools,
    CreateAgentSessionOptions, PromptOptions, StaticResourceLoader,
};
use cortexcode_code_models::AuthLookup;
use cortexcode_code_print::{format_text_output, text_result, PrintFormatter, PrintMode};
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::SettingsManager;
use cortexcode_code_tool_api::ToolDefinition;
use cortexcode_code_tools::{permissions::PermissionPolicy, PolicyPermissionGate};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

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
fn resolve_api_key(provider: &str, cli_key: Option<&str>) -> Option<String> {
    if let Some(key) = cli_key {
        return Some(key.to_string());
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

/// The resource loader until 10.5: `--system-prompt` (a file or text), else
/// the light preset's terse prompt. Skills and context files are not loaded yet.
fn resource_loader(args: &Args, light: bool) -> StaticResourceLoader {
    // main.ts: `systemPrompt: parsed.systemPrompt ?? (lightMode ? LIGHT_SYSTEM_PROMPT : undefined)`
    let source = args
        .system_prompt
        .as_deref()
        .or(light.then_some(cortexcode_code_prompts::LIGHT_SYSTEM_PROMPT));
    StaticResourceLoader {
        system_prompt: resolve_prompt_input(source, "system prompt"),
        append_system_prompt: Vec::new(),
    }
}

/// Credentials by provider: `--api-key`, the environment, then stored OAuth
/// tokens (auth.json storage arrives with 10.4b). Resolved once per provider.
struct CliAuth {
    api_key: Option<String>,
    cache: Mutex<HashMap<String, Option<String>>>,
}

impl CliAuth {
    fn new(args: &Args) -> Self {
        Self {
            api_key: args.api_key.clone(),
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl AuthLookup for CliAuth {
    fn api_key(&self, provider: &str) -> Option<String> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache
            .entry(provider.to_string())
            .or_insert_with(|| resolve_api_key(provider, self.api_key.as_deref()))
            .clone()
    }
}

/// Extension-registered tools in hoocode, SDK tools here: ask_options always
/// (no UI in print mode: it says so), TodoWrite with `--enable-todowrite` or
/// the `enableTodoWrite` setting.
fn custom_tools(args: &Args, settings: &SettingsManager) -> Vec<ToolDefinition> {
    let mut tools = vec![
        cortexcode_code_tools_optin::create_ask_options_tool_definition(Arc::new(
            cortexcode_code_tools_optin::NoUi,
        )),
    ];
    if args
        .todo_write
        .unwrap_or_else(|| settings.enable_todo_write())
    {
        tools.push(
            cortexcode_code_tools_optin::create_todo_write_tool_definition(
                cortexcode_code_tools_optin::StoreRef::Global,
            ),
        );
    }
    tools
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

/// Build the session for a CLI run (`createAgentSession` in main.ts): the
/// model from the flags or settings, the default or light tools, the
/// permission gate for the mode, and a persisted session unless `--no-session`.
fn build_session(args: &Args, interactive: bool) -> Result<AgentSession, RuntimeError> {
    let settings = crate::load_settings();
    let (provider, model_id) = resolve_provider_model(args, &settings);
    // Built-in catalog + models.json custom providers/overrides (ledger 10.4a).
    let registry = match cortexcode_code_models::default_models_json_path() {
        Some(path) => cortexcode_code_models::ModelRegistry::create(path),
        None => cortexcode_code_models::ModelRegistry::in_memory(),
    };
    let model = registry
        .find(&provider, &model_id)
        .cloned()
        .ok_or_else(|| RuntimeError::Setup(format!("unknown model {}:{}", provider, model_id)))?;

    let auth = Arc::new(CliAuth::new(args));
    if !registry.has_configured_auth(&model, auth.as_ref()) {
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
    // Session flags beyond --no-session / --session-dir arrive with 10.7b.
    let cwd_str = cwd.to_string_lossy().into_owned();
    let session_manager = if args.no_session == Some(true) {
        SessionManager::in_memory(cwd_str)
    } else {
        SessionManager::create(cwd_str, args.session_dir.as_ref().map(Into::into))
    };
    Ok(assemble_session(
        args,
        cwd,
        settings,
        registry,
        auth,
        session_manager,
        Some(model),
        interactive,
    ))
}

/// The session over resolved parts. Light preset (`--light`, else the
/// `light` setting): the four light tools and the terse prompt.
#[allow(clippy::too_many_arguments)]
fn assemble_session(
    args: &Args,
    cwd: std::path::PathBuf,
    settings: SettingsManager,
    registry: cortexcode_code_models::ModelRegistry,
    auth: Arc<dyn AuthLookup + Send + Sync>,
    session_manager: SessionManager,
    model: Option<cortexcode_ai_types::Model>,
    interactive: bool,
) -> AgentSession {
    let light = args.light.unwrap_or_else(|| settings.light());
    // main.ts: the light preset is an allowlist of the four short-schema tools
    // (their order is the active order), which also keeps extension tools off.
    let (base_tools, custom, tools) = if light {
        (
            Some(BaseTools::Override(
                cortexcode_code_tools::light::light_tool_definitions(cwd.clone()),
            )),
            Vec::new(),
            Some(
                cortexcode_code_tools::light::LIGHT_TOOL_NAMES
                    .map(String::from)
                    .to_vec(),
            ),
        )
    } else {
        (None, custom_tools(args, &settings), None)
    };
    let services = AgentSessionServices {
        cwd,
        agent_dir: cortexcode_code_paths::agent_dir(),
        auth,
        settings: Arc::new(Mutex::new(settings)),
        model_registry: Arc::new(registry),
        resource_loader: Arc::new(resource_loader(args, light)),
        diagnostics: Vec::new(),
    };
    create_agent_session(
        &services,
        session_manager,
        CreateAgentSessionOptions {
            model,
            thinking_level: args.thinking.clone(),
            tools,
            custom_tools: custom,
            base_tools,
            permission_gate: Some(build_permission_gate(interactive)),
            ..Default::default()
        },
    )
    .session
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

    let session = match build_session(args, false) {
        Ok(session) => session,
        Err(e) => {
            writeln!(err, "{e}")?;
            return Ok(1);
        }
    };

    let formatter = Arc::new(Mutex::new(PrintFormatter::new(mode)));
    let formatter_for_sub = formatter.clone();
    // Session-level events join the JSON stream with 10.8b.
    let _sub = session.subscribe(move |event| {
        if let AgentSessionEvent::Agent(event) = event {
            if let Ok(mut fmt) = formatter_for_sub.lock() {
                fmt.record(event.clone());
            }
        }
    });

    // `session.prompt(initialMessage)` then each remaining message in turn.
    for prompt in initial_message.iter().chain(messages.iter()) {
        let run = session.prompt(prompt, PromptOptions::default());
        if let Err(e) = async_runtime().block_on(run) {
            writeln!(err, "{e}")?;
            session.dispose();
            return Ok(1);
        }
    }
    session.dispose();

    match mode {
        PrintMode::Text => {
            let result = text_result(&session.messages());
            output.write_all(result.stdout.as_bytes())?;
            output.flush()?;
            if let Some(message) = result.stderr {
                writeln!(err, "{message}")?;
            }
            Ok(result.exit_code)
        }
        // Event-stream parity is 10.8b; json mode never inspects the final message.
        PrintMode::Json => {
            drop(_sub);
            let formatter = Arc::try_unwrap(formatter)
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

    let session = build_session(args, true)?;
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
                            let before = session.messages().len();
                            let run = session.prompt(line, PromptOptions::default());
                            match async_runtime().block_on(run) {
                                Ok(()) => {
                                    let messages = session.messages();
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

    /// A session over in-memory settings, models and session, in `/w`.
    fn prompt_for(argv: &[&str]) -> (String, Vec<String>) {
        let args = crate::args::parse_args(&argv.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        let session = assemble_session(
            &args,
            std::path::PathBuf::from("/w"),
            SettingsManager::in_memory(Default::default()),
            cortexcode_code_models::ModelRegistry::in_memory(),
            Arc::new(CliAuth::new(&args)),
            SessionManager::in_memory("/w"),
            None,
            false,
        );
        (session.system_prompt(), session.get_active_tool_names())
    }

    #[test]
    fn light_mode_uses_the_terse_prompt_and_the_four_light_tools() {
        let (prompt, tools) = prompt_for(&["--light"]);
        let date = chrono::Local::now().format("%Y-%m-%d");
        assert_eq!(
            prompt,
            format!(
                "{}\n\nCurrent date: {date}\nCurrent working directory: /w",
                cortexcode_code_prompts::LIGHT_SYSTEM_PROMPT
            )
        );
        assert_eq!(tools, cortexcode_code_tools::light::LIGHT_TOOL_NAMES);
        // --system-prompt still wins over the preset.
        let (prompt, _) = prompt_for(&["--light", "--system-prompt", "Custom."]);
        assert!(prompt.starts_with("Custom.\n\nCurrent date: "));
    }

    #[test]
    fn default_prompt_lists_tools_with_snippets() {
        let (prompt, tools) = prompt_for(&[]);
        assert!(prompt.starts_with("You are an expert coding assistant operating inside cortex"));
        assert!(prompt.contains(
            "Available tools:\n- read: Read file contents\n- bash: Run builds, tests, linters, git, and package managers\n- edit: Make precise file edits with exact text replacement, including multiple disjoint edits in one call\n- write: Create or overwrite files\n- SearchCodebase: Ranked code search (keyword + semantic, rank-fused)\n- ask_options: Put a decision to the user as selectable options\n- TodoWrite: Plan and track multi-step work as a live todo list (use proactively; replaces the whole list each call)\n\nGuidelines:"
        ));
        assert_eq!(
            tools,
            [
                "read",
                "bash",
                "edit",
                "write",
                "SearchCodebase",
                "ask_options",
                "TodoWrite"
            ]
        );
    }

    #[test]
    fn test_default_model_for_provider() {
        assert!(!default_model_for_provider("anthropic").is_empty());
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
            resolve_api_key("anthropic", args.api_key.as_deref()),
            Some("cli-key".to_string())
        );
    }
}
