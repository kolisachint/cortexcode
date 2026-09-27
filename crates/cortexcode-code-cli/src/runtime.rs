//! Runtime glue between the `cortex` CLI and the agent namespace.
//!
//! This module builds an `Agent` from CLI arguments, wires up the default
//! coding tools, and dispatches to print or interactive mode. It is the
//! integration point that turns the previously stubbed CLI commands into
//! actual LLM-backed sessions.

use crate::{Args, DiagnosticKind};
use cortexcode_agent_types::PermissionGate;
use cortexcode_ai_types::{Model, ThinkingLevel};
use cortexcode_code_auth::AuthStorage;

use cortexcode_code_agent_session::{
    create_agent_session, AgentSession, AgentSessionServices, BaseTools, CreateAgentSessionOptions,
    DefaultResources, PromptOptions, ScopedModel,
};
use cortexcode_code_models::{resolve_cli_model, resolve_model_scope, AuthLookup, ModelRegistry};
use cortexcode_code_print::{format_text_output, json_line, text_result, PrintMode};
use cortexcode_code_resources::DefaultResourceLoaderOptions;
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::SettingsManager;
use cortexcode_code_tool_api::ToolDefinition;
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

/// Register the built-in OAuth providers once (hoocode's registry starts with
/// them): `AuthStorage` refreshes tokens and the registry's `modifyModels` pass
/// look them up by id.
pub(crate) fn install_oauth_providers() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        cortexcode_ai_oauth::install_builtin_oauth_providers(vec![
            Arc::new(cortexcode_ai_oauth_anthropic::AnthropicOAuthProvider::default()),
            Arc::new(cortexcode_ai_oauth_github_copilot::GitHubCopilotOAuthProvider::default()),
            Arc::new(cortexcode_ai_oauth_google::GeminiCliOAuthProvider::default()),
            Arc::new(cortexcode_ai_oauth_google::AntigravityOAuthProvider::default()),
            Arc::new(cortexcode_ai_oauth_openai_codex::OpenAICodexOAuthProvider::default()),
        ]);
    });
}

/// `AuthStorage.create()` and `ModelRegistry.create(authStorage)`: built-ins plus
/// models.json, with the OAuth providers' `modifyModels` applied.
pub(crate) fn load_auth_and_registry() -> (Arc<AuthStorage>, ModelRegistry) {
    install_oauth_providers();
    let auth = Arc::new(AuthStorage::create(None));
    let mut registry = match cortexcode_code_models::default_models_json_path() {
        Some(path) => ModelRegistry::create(path),
        None => ModelRegistry::in_memory(),
    };
    registry.set_model_modifier(auth.model_modifier());
    (auth, registry)
}

/// `AgentSessionRuntimeDiagnostic` (errors and warnings; main.ts reports them
/// after building the runtime and exits on an error).
pub(crate) type Diagnostics = Vec<(DiagnosticKind, String)>;

/// `reportDiagnostics`.
fn report_diagnostics(
    err: &mut dyn Write,
    color: bool,
    diagnostics: &Diagnostics,
) -> std::io::Result<()> {
    for (kind, message) in diagnostics {
        let line = match kind {
            DiagnosticKind::Error => crate::red(color, &format!("Error: {message}")),
            DiagnosticKind::Warning => {
                let text = format!("Warning: {message}");
                if color {
                    format!("\x1b[33m{text}\x1b[39m")
                } else {
                    text
                }
            }
        };
        writeln!(err, "{line}")?;
    }
    Ok(())
}

/// Model choices from the flags (`buildSessionOptions` in main.ts).
#[derive(Default)]
struct ModelOptions {
    model: Option<Model>,
    thinking_level: Option<ThinkingLevel>,
    scoped_models: Vec<ScopedModel>,
}

/// `buildSessionOptions`' model part: `--model` (with `--provider`, `provider/`
/// and `:thinking` shorthands), else the saved default when it is in the
/// `--models` scope, else the first scoped model; `--thinking` wins.
fn model_options(
    args: &Args,
    scoped_models: Vec<ScopedModel>,
    has_existing_session: bool,
    registry: &ModelRegistry,
    settings: &SettingsManager,
    diagnostics: &mut Diagnostics,
) -> ModelOptions {
    let mut options = ModelOptions::default();
    if let Some(cli_model) = args.model.as_deref() {
        let resolved = resolve_cli_model(
            args.provider.as_deref(),
            Some(cli_model),
            registry.get_all(),
        );
        if let Some(warning) = resolved.warning {
            diagnostics.push((DiagnosticKind::Warning, warning));
        }
        if let Some(error) = resolved.error {
            diagnostics.push((DiagnosticKind::Error, error));
        }
        if let Some(model) = resolved.model {
            options.model = Some(model);
            if args.thinking.is_none() {
                options.thinking_level = resolved.thinking_level;
            }
        }
    }

    if options.model.is_none() && !scoped_models.is_empty() && !has_existing_session {
        let saved = settings
            .default_provider()
            .zip(settings.default_model())
            .and_then(|(p, m)| registry.find(&p, &m).cloned());
        let chosen = saved
            .and_then(|saved| {
                scoped_models
                    .iter()
                    .find(|sm| sm.model.provider == saved.provider && sm.model.id == saved.id)
            })
            .unwrap_or(&scoped_models[0]);
        options.model = Some(chosen.model.clone());
        if args.thinking.is_none() && chosen.thinking_level.is_some() {
            options.thinking_level = chosen.thinking_level.clone();
        }
    }

    if let Some(level) = &args.thinking {
        options.thinking_level = Some(level.clone());
    }
    options.scoped_models = scoped_models;
    options
}

/// `--skill` / `--prompt-template` / `--slash-command` values (`resolveCliPaths`):
/// local paths against the cwd, package sources as given.
fn resolve_cli_paths(cwd: &std::path::Path, paths: &Option<Vec<String>>) -> Vec<String> {
    let cwd = cwd.to_string_lossy();
    paths
        .iter()
        .flatten()
        .map(|value| {
            if cortexcode_code_paths::is_local_path(value) {
                cortexcode_code_resources::node_path::resolve(&cwd, value)
            } else {
                value.clone()
            }
        })
        .collect()
}

/// The `DefaultResourceLoader` main.ts builds: CLI resource paths, the built-in
/// skills (last, so a user's skill of the same name wins), and the light preset
/// (no skills or context files, the terse system prompt).
fn resource_loader(
    args: &Args,
    light: bool,
    cwd: &std::path::Path,
    agent_dir: &std::path::Path,
    settings: &Arc<Mutex<SettingsManager>>,
) -> DefaultResources {
    let no_skills = args.no_skills == Some(true) || light;
    let agent_dir_str = agent_dir.to_string_lossy().into_owned();
    let mut skill_paths = resolve_cli_paths(cwd, &args.skills);
    if !no_skills {
        let gate = cortexcode_code_resources::builtin_skills::BuiltinSkillGate {
            enable_plugin_tools: settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .enable_plugin_tools(),
        };
        skill_paths.extend(
            cortexcode_code_resources::builtin_skills::builtin_skill_paths(gate, &agent_dir_str),
        );
    }
    // main.ts: `systemPrompt: parsed.systemPrompt ?? (lightMode ? LIGHT_SYSTEM_PROMPT : undefined)`
    let system_prompt = args
        .system_prompt
        .clone()
        .or_else(|| light.then(|| cortexcode_code_prompts::LIGHT_SYSTEM_PROMPT.to_string()));
    DefaultResources::loaded(DefaultResourceLoaderOptions {
        cwd: cwd.to_string_lossy().into_owned(),
        agent_dir: agent_dir_str,
        settings: Some(settings.clone()),
        additional_skill_paths: skill_paths,
        additional_prompt_template_paths: resolve_cli_paths(cwd, &args.prompt_templates),
        additional_slash_command_paths: resolve_cli_paths(cwd, &args.slash_commands),
        no_skills,
        no_prompt_templates: args.no_prompt_templates == Some(true),
        no_slash_commands: args.no_slash_commands == Some(true),
        no_context_files: args.no_context_files == Some(true) || light,
        system_prompt,
        ..Default::default()
    })
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
/// hoocode's permission gate (hoo-core): per-mode hard rules always; prompts
/// for bash/write/edit/web tools only when there is a UI to ask.
fn build_permission_gate(interactive: bool, cwd: &std::path::Path) -> Arc<dyn PermissionGate> {
    let ui: Option<Arc<dyn cortexcode_code_permissions::PermissionUi>> =
        interactive.then(|| Arc::new(crate::permission_dialog::TerminalPermissionUi) as _);
    Arc::new(cortexcode_code_permissions::HooPermissionGate::new(
        cwd.to_path_buf(),
        ui,
    ))
}

/// Build the session for a CLI run (the `createRuntime` factory of main.ts):
/// auth.json + models.json, the `--models` scope, the model from the flags (or
/// `findInitialModel` inside `create_agent_session`), `--api-key` as a runtime
/// key for the chosen provider, the default or light tools, and a persisted
/// session unless `--no-session`. Diagnostics are for the caller to report.
fn build_session(args: &Args, interactive: bool) -> (AgentSession, Diagnostics) {
    let settings = crate::load_settings();
    let (auth, registry) = load_auth_and_registry();
    let mut diagnostics = Diagnostics::new();

    let startup_cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let env = crate::Env::detect();
    let session_dir = crate::session_flags::session_dir(args, &settings);
    let mut session_manager = crate::session_flags::create_session_manager(
        args,
        &startup_cwd.to_string_lossy(),
        session_dir.clone(),
        env,
    );
    if let Some(issue) = cortexcode_code_agent_session::runtime::get_missing_session_cwd_issue(
        &session_manager,
        &startup_cwd,
    ) {
        // hoocode asks with a selector in interactive mode (TUI, 11.3); the
        // placeholder asks on stdin.
        let message =
            cortexcode_code_agent_session::runtime::RuntimeError::MissingSessionCwd(issue.clone())
                .to_string();
        if !interactive {
            eprintln!("{}", crate::red(env.color, &message));
            std::process::exit(1);
        }
        println!(
            "{}",
            cortexcode_code_agent_session::runtime::format_missing_session_cwd_prompt(&issue)
        );
        print!("Continue? [y/N] ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let mut answer = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer);
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            std::process::exit(0);
        }
        session_manager = SessionManager::open(
            issue.session_file.clone().unwrap_or_default(),
            session_dir,
            Some(issue.fallback_cwd.clone()),
        );
    }
    // The runtime runs in the session's cwd (it may be another project's).
    let cwd = std::path::PathBuf::from(session_manager.cwd());
    let settings = if cwd == startup_cwd {
        settings
    } else {
        SettingsManager::create_default(&cwd)
    };

    let patterns = args.models.clone().or_else(|| settings.enabled_models());
    let scoped_models = match patterns.filter(|p| !p.is_empty()) {
        Some(patterns) => {
            let available: Vec<Model> = registry
                .get_available(auth.as_ref())
                .into_iter()
                .cloned()
                .collect();
            let scope = resolve_model_scope(&patterns, &available);
            // resolveModelScope warns straight to stderr, before the diagnostics.
            diagnostics.extend(
                scope
                    .warnings
                    .into_iter()
                    .map(|w| (DiagnosticKind::Warning, w)),
            );
            scope.models
        }
        None => Vec::new(),
    };
    let has_existing_session = !session_manager.build_context().messages.is_empty();
    let options = model_options(
        args,
        scoped_models,
        has_existing_session,
        &registry,
        &settings,
        &mut diagnostics,
    );

    if let Some(api_key) = &args.api_key {
        match &options.model {
            None => diagnostics.push((
                DiagnosticKind::Error,
                "--api-key requires a model to be specified via --model, --provider/--model, or --models"
                    .into(),
            )),
            Some(model) => auth.set_runtime_api_key(&model.provider, api_key),
        }
    }

    let session = assemble_session(
        args,
        cwd,
        cortexcode_code_paths::agent_dir(),
        settings,
        registry,
        auth,
        session_manager,
        options,
        interactive,
    );
    (session, diagnostics)
}

/// The session over resolved parts. Light preset (`--light`, else the
/// `light` setting): the four light tools and the terse prompt.
#[allow(clippy::too_many_arguments)]
fn assemble_session(
    args: &Args,
    cwd: std::path::PathBuf,
    agent_dir: std::path::PathBuf,
    settings: SettingsManager,
    registry: ModelRegistry,
    auth: Arc<dyn AuthLookup + Send + Sync>,
    session_manager: SessionManager,
    model_options: ModelOptions,
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
    // main.ts: explicit tool flags win over the light preset's allowlist.
    let explicit =
        args.tools.is_some() || args.no_tools == Some(true) || args.no_builtin_tools == Some(true);
    let tools = if explicit { args.tools.clone() } else { tools };
    let no_tools = if args.no_tools == Some(true) {
        Some(cortexcode_code_agent_session::NoTools::All)
    } else if args.no_builtin_tools == Some(true) {
        Some(cortexcode_code_agent_session::NoTools::Builtin)
    } else {
        None
    };
    let settings = Arc::new(Mutex::new(settings));
    let resources = resource_loader(args, light, &cwd, &agent_dir, &settings);
    // hoo-core's mode system: the active mode's prompt block and tool filter.
    let cwd_str = cwd.to_string_lossy().into_owned();
    let modes = Arc::new(cortexcode_code_modes::ModesExtension::new(
        cortexcode_code_modes::ModeSession {
            cwd: cwd.clone(),
            session_id: session_manager.session_id().to_string(),
            light,
            mode_search_paths: args
                .mode_paths
                .iter()
                .flatten()
                .map(|p| cortexcode_code_resources::node_path::resolve_config_path(p, &cwd_str))
                .collect(),
        },
    ));
    let mode_tools = modes.active().enabled_tools.clone();
    // main.ts: `--disallowed-tools` plus the persisted per-tool disables.
    let mut disallowed_tools = args.disallowed_tools.clone();
    let disabled = settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .disabled_tools();
    if !disabled.is_empty() {
        let list = disallowed_tools.get_or_insert_with(Vec::new);
        for tool in disabled {
            if !list.contains(&tool) {
                list.push(tool);
            }
        }
    }
    let services = AgentSessionServices {
        cwd,
        agent_dir,
        auth,
        settings,
        model_registry: Arc::new(registry),
        resource_loader: Arc::new(resources),
        diagnostics: Vec::new(),
    };
    let session = create_agent_session(
        &services,
        session_manager,
        CreateAgentSessionOptions {
            model: model_options.model,
            thinking_level: model_options.thinking_level,
            scoped_models: model_options.scoped_models,
            tools,
            no_tools,
            custom_tools: custom,
            base_tools,
            permission_gate: Some(build_permission_gate(interactive, &services.cwd)),
            disallowed_tools,
            extensions: Some(modes),
            ..Default::default()
        },
    )
    .session;
    if let Some(tools) = mode_tools {
        session.set_active_tools_by_name(&tools);
    }
    session
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

    let (session, diagnostics) = build_session(args, false);
    report_diagnostics(err, color, &diagnostics)?;
    if diagnostics
        .iter()
        .any(|(kind, _)| *kind == DiagnosticKind::Error)
    {
        return Ok(1);
    }
    if session.model().is_none() {
        writeln!(
            err,
            "{}",
            crate::red(
                color,
                &cortexcode_code_auth::auth_guidance::format_no_models_available_message()
            )
        )?;
        return Ok(1);
    }

    // `--mode json`: the session header, then every session event as it
    // happens. Listeners run on runtime threads; lines go through a channel and
    // are written here, between polls of the running prompt.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let _sub = (mode == PrintMode::Json).then(|| {
        let header =
            cortexcode_code_session::FileEntry::Session(session.session_manager().header().clone());
        let _ = tx.send(json_line(
            &serde_json::to_value(&header).unwrap_or_default(),
        ));
        let tx = tx.clone();
        session.subscribe(move |event| {
            let _ = tx.send(json_line(&event.to_json()));
        })
    });
    drop(tx);

    // `session.prompt(initialMessage)` then each remaining message in turn.
    for prompt in initial_message.iter().chain(messages.iter()) {
        let run = session.prompt(prompt, PromptOptions::default());
        let result = async_runtime().block_on(async {
            tokio::pin!(run);
            loop {
                tokio::select! {
                    result = &mut run => break Ok(result),
                    Some(line) = rx.recv() => {
                        if let Err(e) = output.write_all(line.as_bytes()).and_then(|_| output.flush()) {
                            break Err(e);
                        }
                    }
                }
            }
        })?;
        write_lines(&mut rx, output)?;
        if let Err(e) = result {
            writeln!(err, "{e}")?;
            session.dispose();
            return Ok(1);
        }
    }
    session.dispose();
    drop(_sub);
    write_lines(&mut rx, output)?;

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
        // json mode never inspects the final message.
        PrintMode::Json => Ok(0),
    }
}

/// Write the queued `--mode json` lines.
fn write_lines(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    while let Ok(line) = rx.try_recv() {
        output.write_all(line.as_bytes())?;
    }
    output.flush()
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

    let (session, diagnostics) = build_session(args, true);
    report_diagnostics(err, crate::Env::detect().color, &diagnostics)?;
    if diagnostics
        .iter()
        .any(|(kind, _)| *kind == DiagnosticKind::Error)
    {
        return Err(RuntimeError::Setup("invalid model options".into()));
    }
    let mut stdout = std::io::stdout();
    terminal::enable_raw_mode().map_err(|e| RuntimeError::Setup(e.to_string()))?;
    let _ = stdout
        .queue(terminal::Clear(terminal::ClearType::All))?
        .queue(cursor::MoveTo(0, 0))?
        .flush();

    writeln!(
        output,
        "{} Interactive Cortex mode. /compact, /quit or Ctrl+C to exit.",
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
                        if line == "/compact" || line.starts_with("/compact ") {
                            // handleCompactCommand: the session decides whether
                            // compaction is possible and reports why not.
                            let instructions = line["/compact".len()..].trim();
                            let instructions = (!instructions.is_empty()).then_some(instructions);
                            match async_runtime().block_on(session.compact(instructions)) {
                                Ok(result) => writeln!(
                                    output,
                                    "\nCompacted {} → {} tokens\n",
                                    result.tokens_before,
                                    result
                                        .tokens_after
                                        .map_or_else(|| "?".to_string(), |t| t.to_string())
                                )?,
                                Err(e) => writeln!(output, "\nCompaction failed: {}\n", e)?,
                            }
                            input.clear();
                            continue;
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
        let agent_dir = tempfile::tempdir().unwrap();
        let session = assemble_session(
            &args,
            std::path::PathBuf::from("/w"),
            agent_dir.path().to_path_buf(),
            SettingsManager::in_memory(Default::default()),
            cortexcode_code_models::ModelRegistry::in_memory(),
            Arc::new(cortexcode_code_models::NoAuth),
            SessionManager::in_memory("/w"),
            ModelOptions::default(),
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

    fn models_json_registry() -> ModelRegistry {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        std::fs::write(
            &path,
            r#"{"providers": {"mock": {"baseUrl": "http://127.0.0.1:1/v1", "api": "openai-completions", "apiKey": "k", "models": [{"id": "mock-model", "reasoning": true}, {"id": "mock-mini"}]}}}"#,
        )
        .unwrap();
        ModelRegistry::create(path)
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

    fn options_for(
        argv: &[&str],
        scoped: Vec<ScopedModel>,
        s: &SettingsManager,
    ) -> (ModelOptions, Diagnostics) {
        let args = crate::args::parse_args(&argv.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        let mut diagnostics = Diagnostics::new();
        let options = model_options(
            &args,
            scoped,
            false,
            &models_json_registry(),
            s,
            &mut diagnostics,
        );
        (options, diagnostics)
    }

    fn id(options: &ModelOptions) -> Option<String> {
        options
            .model
            .as_ref()
            .map(|m| format!("{}/{}", m.provider, m.id))
    }

    #[test]
    fn model_flag_resolves_patterns_and_thinking_shorthand() {
        let none = settings(None, None);
        let (o, d) = options_for(&["--model", "mock/mini:high"], vec![], &none);
        assert_eq!(id(&o).as_deref(), Some("mock/mock-mini"));
        assert_eq!(o.thinking_level, Some(ThinkingLevel::High));
        assert!(d.is_empty());
        // --thinking wins over the shorthand.
        let (o, _) = options_for(
            &["--model", "mock-mini:high", "--thinking", "low"],
            vec![],
            &none,
        );
        assert_eq!(o.thinking_level, Some(ThinkingLevel::Low));
        // Unknown model: the resolver's error becomes a diagnostic.
        let (o, d) = options_for(&["--model", "nope-nothing"], vec![], &none);
        assert!(o.model.is_none());
        assert_eq!(d[0].0, DiagnosticKind::Error);
        assert!(d[0].1.contains("Model \"nope-nothing\" not found"));
    }

    #[test]
    fn scoped_models_prefer_the_saved_default_then_the_first() {
        let registry = models_json_registry();
        let scoped: Vec<ScopedModel> = ["mock-model", "mock-mini"]
            .iter()
            .map(|id| ScopedModel {
                model: registry.find("mock", id).unwrap().clone(),
                thinking_level: None,
            })
            .collect();
        let (o, _) = options_for(
            &[],
            scoped.clone(),
            &settings(Some("mock"), Some("mock-mini")),
        );
        assert_eq!(id(&o).as_deref(), Some("mock/mock-mini"));
        let (o, _) = options_for(&[], scoped.clone(), &settings(None, None));
        assert_eq!(id(&o).as_deref(), Some("mock/mock-model"));
        assert_eq!(o.scoped_models.len(), 2);
    }
}
