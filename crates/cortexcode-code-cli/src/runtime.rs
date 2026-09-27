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
    create_agent_session, AgentSession, AgentSessionRuntime, AgentSessionRuntimeDiagnostic,
    AgentSessionServices, BaseTools, CreateAgentSessionOptions, CreatedRuntime, DefaultResources,
    PromptOptions, ScopedModel, SessionStartEvent,
};
use cortexcode_code_models::{resolve_cli_model, resolve_model_scope, AuthLookup, ModelRegistry};
use cortexcode_code_print::{json_line, text_result, PrintMode};
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
    let auth = load_auth();
    let registry = load_registry(&auth);
    (auth, registry)
}

/// `AuthStorage.create()`.
fn load_auth() -> Arc<AuthStorage> {
    install_oauth_providers();
    Arc::new(AuthStorage::create(None))
}

/// `ModelRegistry.create(authStorage)`.
fn load_registry(auth: &Arc<AuthStorage>) -> ModelRegistry {
    let mut registry = match cortexcode_code_models::default_models_json_path() {
        Some(path) => ModelRegistry::create(path),
        None => ModelRegistry::in_memory(),
    };
    registry.set_model_modifier(auth.model_modifier());
    registry
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
fn custom_tools(
    args: &Args,
    settings: &SettingsManager,
    cwd: &std::path::Path,
) -> Vec<ToolDefinition> {
    let mut tools = vec![
        cortexcode_code_tools_optin::create_ask_options_tool_definition(Arc::new(
            cortexcode_code_tools_optin::NoUi,
        )),
    ];
    tools.extend(subagent_tools(args, settings, false, cwd));
    if args.task_id.is_none()
        && args
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

/// main.ts's subagent block of `buildSessionOptions`: seed the tree-wide depth
/// cap and nested concurrency into the environment (a root only; descendants
/// inherit them), set or clear `--delegate-allow`, and decide whether this
/// process gets the Task/TaskOutput tools (and warm workers, root only).
fn subagent_tools(
    args: &Args,
    settings: &SettingsManager,
    light: bool,
    cwd: &std::path::Path,
) -> Vec<ToolDefinition> {
    use cortexcode_code_subagents::depth::{self, ProcessEnv, SubagentEnv};
    let is_subagent_child = args.task_id.is_some();
    if ProcessEnv.var(depth::SUBAGENT_MAX_DEPTH_ENV).is_none() {
        let cap = depth::resolve_max_subagent_depth(
            Some(
                args.max_subagent_depth
                    .unwrap_or_else(|| settings.max_subagent_depth()) as f64,
            ),
            &std::collections::HashMap::<String, String>::new(),
        );
        std::env::set_var(
            format!("CORTEXCODE_{}", depth::SUBAGENT_MAX_DEPTH_ENV),
            cap.to_string(),
        );
    }
    if ProcessEnv.var(depth::NESTED_CONCURRENCY_ENV).is_none() {
        let n = depth::resolve_nested_concurrency(
            Some(settings.nested_subagent_concurrency() as f64),
            &std::collections::HashMap::<String, String>::new(),
        );
        std::env::set_var(
            format!("CORTEXCODE_{}", depth::NESTED_CONCURRENCY_ENV),
            n.to_string(),
        );
    }
    // --delegate-allow is authoritative: a restricted parent's scope never
    // leaks into a child that was not given its own.
    for prefix in cortexcode_code_paths::ENV_PREFIXES {
        std::env::remove_var(format!("{prefix}{}", depth::DELEGATE_ALLOW_ENV));
    }
    if let Some(allow) = args.delegate_allow.as_ref().filter(|a| !a.is_empty()) {
        std::env::set_var(
            format!("CORTEXCODE_{}", depth::DELEGATE_ALLOW_ENV),
            allow.join(","),
        );
    }
    let enabled = args.subagent.unwrap_or_else(|| settings.enable_subagent());
    if light || !depth::can_spawn_subagent(None, &ProcessEnv) || !enabled {
        return Vec::new();
    }
    if !is_subagent_child
        && args
            .warm_subagents
            .unwrap_or_else(|| settings.warm_subagents())
    {
        std::env::set_var(
            format!(
                "CORTEXCODE_{}",
                cortexcode_code_subagents::warm::WARM_SUBAGENTS_ENV
            ),
            "1",
        );
    }
    vec![
        cortexcode_code_subagents::tools::create_task_tool_definition(cwd),
        cortexcode_code_subagents::tools::create_task_output_tool_definition(),
    ]
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

/// Build the session for a CLI run: the initial session manager, then the
/// `createRuntime` factory on it. Diagnostics are for the caller to report.
fn build_session(args: &Args, interactive: bool) -> (AgentSession, Diagnostics) {
    let auth = load_auth();
    let session_manager = initial_session_manager(args, interactive);
    let cwd = std::path::PathBuf::from(session_manager.cwd());
    let (session, _, diagnostics) = create_runtime(
        args,
        &auth,
        cwd,
        cortexcode_code_paths::agent_dir(),
        session_manager,
        None,
        interactive,
    );
    (session, diagnostics)
}

/// The session manager a run starts on (`--session`, `--continue`, ...). A
/// session whose cwd is gone falls back to the startup cwd after asking
/// (interactive) or exits (otherwise).
fn initial_session_manager(args: &Args, interactive: bool) -> SessionManager {
    let settings = crate::load_settings();
    let startup_cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let env = crate::Env::detect();
    let session_dir = crate::session_flags::session_dir(args, &settings);
    let session_manager = crate::session_flags::create_session_manager(
        args,
        &startup_cwd.to_string_lossy(),
        session_dir.clone(),
        env,
    );
    let Some(issue) = cortexcode_code_agent_session::runtime::get_missing_session_cwd_issue(
        &session_manager,
        &startup_cwd,
    ) else {
        return session_manager;
    };
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
    SessionManager::open(
        issue.session_file.clone().unwrap_or_default(),
        session_dir,
        Some(issue.fallback_cwd.clone()),
    )
}

/// The `createRuntime` factory of main.ts: settings for the cwd, models.json,
/// the `--models` scope, the model from the flags (or `findInitialModel`
/// inside `create_agent_session`), `--api-key` as a runtime key for the chosen
/// provider, and the default or light tools. `auth` is shared across runtimes.
#[allow(clippy::type_complexity)]
fn create_runtime(
    args: &Args,
    auth: &Arc<AuthStorage>,
    cwd: std::path::PathBuf,
    agent_dir: std::path::PathBuf,
    session_manager: SessionManager,
    session_start_event: Option<SessionStartEvent>,
    interactive: bool,
) -> (AgentSession, AgentSessionServices, Diagnostics) {
    let settings = SettingsManager::create_default(&cwd);
    let registry = load_registry(auth);
    let mut diagnostics = Diagnostics::new();

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

    let (session, services) = assemble_session(
        args,
        cwd,
        agent_dir,
        settings,
        registry,
        auth.clone(),
        session_manager,
        options,
        session_start_event,
        interactive,
    );
    (session, services, diagnostics)
}

/// The concrete resource loader of the most recently assembled session: the
/// interactive listing reads skills, prompts and diagnostics from it (the
/// session only holds it as `dyn ResourceLoader`).
static LAST_RESOURCES: Mutex<Option<Arc<cortexcode_code_agent_session::DefaultResources>>> =
    Mutex::new(None);

pub(crate) fn last_resources() -> Option<Arc<cortexcode_code_agent_session::DefaultResources>> {
    LAST_RESOURCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
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
    session_start_event: Option<SessionStartEvent>,
    interactive: bool,
) -> (AgentSession, AgentSessionServices) {
    let light = args.light.unwrap_or_else(|| settings.light());
    // main.ts: the light preset is an allowlist of the four short-schema tools
    // (their order is the active order), which also keeps extension tools off.
    let (base_tools, custom, tools) = if light {
        // Still seeds the subagent env; light mode gets no Task tool.
        subagent_tools(args, &settings, true, &cwd);
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
        (None, custom_tools(args, &settings, &cwd), None)
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
    // main.ts: the main-session subagent instructions when subagents are on.
    let subagents_on = !light && args.subagent.unwrap_or_else(|| settings.enable_subagent());
    let settings = Arc::new(Mutex::new(settings));
    let resources = resource_loader(args, light, &cwd, &agent_dir, &settings);
    if subagents_on {
        resources.loader().add_append_system_prompt(
            cortexcode_code_subagents::tools::build_task_main_prompt(&cwd),
        );
    }
    // AgentSession's constructor: forward the skill paths to subagents.
    let skill_paths = resources.loader().skill_paths();
    cortexcode_code_subagents::instance::update_subagent_skill_paths(skill_paths.clone());
    cortexcode_code_subagents::warm::update_warm_subagent_skill_paths(skill_paths);
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
    let resources = Arc::new(resources);
    *LAST_RESOURCES.lock().unwrap_or_else(|e| e.into_inner()) = Some(resources.clone());
    let services = AgentSessionServices {
        cwd,
        agent_dir,
        auth,
        settings,
        model_registry: Arc::new(registry),
        resource_loader: resources,
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
            session_start_event,
            ..Default::default()
        },
    )
    .session;
    if let Some(tools) = mode_tools {
        session.set_active_tools_by_name(&tools);
    }
    (session, services)
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

    // A spawned subagent (json mode + `--task-id`): heartbeats, only the
    // events the parent pool consumes, the turn cap, and result.json.
    let task_id = args
        .task_id
        .clone()
        .filter(|id| mode == PrintMode::Json && !id.is_empty());

    // `--mode json`: the session header, then every session event as it
    // happens. Listeners run on runtime threads; lines go through a channel and
    // are written here, between polls of the running prompt.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let heartbeat = task_id.as_ref().map(|_| {
        // An immediate ping, so a child that crashes during startup surfaces
        // its error instead of stalling silently until the first heartbeat.
        let _ = tx.send(json_line(&serde_json::json!({"ping": true})));
        let tx = tx.clone();
        async_runtime().spawn(async move {
            let mut interval = tokio::time::interval(SUBAGENT_HEARTBEAT);
            interval.tick().await;
            loop {
                interval.tick().await;
                if tx
                    .send(json_line(&serde_json::json!({"ping": true})))
                    .is_err()
                {
                    break;
                }
            }
        })
    });
    let is_subagent = task_id.is_some();
    let _sub = (mode == PrintMode::Json).then(|| {
        let header =
            cortexcode_code_session::FileEntry::Session(session.session_manager().header().clone());
        let _ = tx.send(json_line(
            &serde_json::to_value(&header).unwrap_or_default(),
        ));
        let tx = tx.clone();
        session.subscribe(move |event| {
            let value = event.to_json();
            // The parent only consumes progress events and message_end usage;
            // the per-delta firehose is dropped at the source.
            if is_subagent {
                let kind = value.get("type").and_then(serde_json::Value::as_str);
                if !kind.is_some_and(|k| {
                    cortexcode_code_subagents::events::SUBAGENT_STDOUT_EVENT_TYPES.contains(&k)
                }) {
                    return;
                }
            }
            let _ = tx.send(json_line(&value));
        })
    });
    drop(tx);
    let reached_max_turns = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _turn_limit = args
        .max_turns
        .filter(|cap| is_subagent && *cap > 0)
        .map(|cap| turn_limit(&session, cap, reached_max_turns.clone()));

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

    // A spawned subagent writes the audit file the parent pool verifies.
    let mut exit_code = 0;
    if let Some(task_id) = &task_id {
        let stats = session.get_session_stats();
        let mut result = cortexcode_code_subagents::result::build_subagent_result(
            &session.messages(),
            Some(cortexcode_code_subagents::result::SubagentUsage {
                input: stats.tokens.input as f64,
                output: stats.tokens.output as f64,
                cache_read: stats.tokens.cache_read as f64,
                cache_write: stats.tokens.cache_write as f64,
                cost: stats.cost,
            }),
            cortexcode_code_subagents::result::BuildSubagentResultOptions {
                reached_max_turns: reached_max_turns.load(std::sync::atomic::Ordering::SeqCst),
            },
        );
        // This subagent's own task subtree, for the parent to render below
        // the dispatching task.
        let tree = cortexcode_code_subagents::result::build_task_forest(
            &cortexcode_code_task_store::task_store().list(),
        );
        if !tree.is_empty() {
            result.task_tree = Some(tree);
        }
        let cwd = std::path::PathBuf::from(session.session_manager().cwd());
        cortexcode_code_subagents::result::write_subagent_result(&cwd, task_id, &result);
        if result.status == cortexcode_code_subagents::result::ResultStatus::Failed {
            exit_code = 1;
        }
    }
    if let Some(heartbeat) = heartbeat {
        heartbeat.abort();
    }
    drop(_turn_limit);
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
        PrintMode::Json => Ok(exit_code),
    }
}

/// `--mode rpc`: JSON commands on stdin, responses and session events on
/// stdout, until stdin ends. The session lives in an `AgentSessionRuntime`, so
/// new_session/switch_session/fork/clone replace it through the factory.
pub fn run_rpc_mode(args: &Args, color: bool, err: &mut dyn Write) -> std::io::Result<i32> {
    let auth = load_auth();
    let session_manager = initial_session_manager(args, false);
    let cwd = std::path::PathBuf::from(session_manager.cwd());
    let (session, services, diagnostics) = create_runtime(
        args,
        &auth,
        cwd,
        cortexcode_code_paths::agent_dir(),
        session_manager,
        None,
        false,
    );
    report_diagnostics(err, color, &diagnostics)?;
    if diagnostics
        .iter()
        .any(|(kind, _)| *kind == DiagnosticKind::Error)
    {
        return Ok(1);
    }
    let factory_args = args.clone();
    let factory: cortexcode_code_agent_session::RuntimeFactory = Arc::new(move |request| {
        let (session, services, diagnostics) = create_runtime(
            &factory_args,
            &auth,
            request.cwd,
            request.agent_dir,
            request.session_manager,
            request.session_start_event,
            false,
        );
        let created = CreatedRuntime {
            session,
            services,
            diagnostics: runtime_diagnostics(diagnostics),
            model_fallback_message: None,
        };
        Box::pin(async move { Ok(created) })
    });
    let runtime = AgentSessionRuntime::new(
        CreatedRuntime {
            session,
            services,
            diagnostics: runtime_diagnostics(diagnostics),
            model_fallback_message: None,
        },
        factory,
    );
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let output: cortexcode_code_rpc::RpcOutput = Arc::new(move |value| {
        let line = cortexcode_code_rpc::serialize_json_line(value);
        let mut stdout = stdout.lock().unwrap_or_else(|e| e.into_inner());
        let _ = stdout.write_all(line.as_bytes());
        let _ = stdout.flush();
    });
    let host = Arc::new(cortexcode_code_rpc::RuntimeHost::new(runtime));
    Ok(async_runtime().block_on(cortexcode_code_rpc::run_rpc_mode(
        host,
        tokio::io::stdin(),
        output,
    )))
}

/// The CLI's diagnostics as `AgentSessionRuntimeDiagnostic`s.
fn runtime_diagnostics(diagnostics: Diagnostics) -> Vec<AgentSessionRuntimeDiagnostic> {
    diagnostics
        .into_iter()
        .map(|(kind, message)| AgentSessionRuntimeDiagnostic {
            kind: match kind {
                DiagnosticKind::Error => cortexcode_code_agent_session::DiagnosticKind::Error,
                DiagnosticKind::Warning => cortexcode_code_agent_session::DiagnosticKind::Warning,
            },
            message,
        })
        .collect()
}

/// Heartbeat cadence for spawned subagents (the parent's lifeguard stalls
/// after 60s of silence).
const SUBAGENT_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(30);

/// A spawned subagent's turn cap: near it (90%) the agent is asked to wrap
/// up; at it the run is aborted.
///
/// hoocode's session listeners see agent events asynchronously (queued behind
/// the extension handlers), so its `turn_end` handler runs after the loop has
/// already collected the next turn's pending messages and started that turn.
/// cortex listeners run synchronously; the steer/abort decided at `turn_end`
/// is therefore applied at the next `turn_start`, which is where hoocode's
/// lands: the wrap-up steer reaches the model one turn later, and the abort
/// stops the turn after the cap. If no further turn starts, hoocode's late
/// steer/abort have nothing left to act on either.
fn turn_limit(
    session: &AgentSession,
    cap: u64,
    reached: Arc<std::sync::atomic::AtomicBool>,
) -> cortexcode_code_agent_session::SessionSubscription {
    use cortexcode_agent_types::AgentEvent;
    use cortexcode_code_agent_session::AgentSessionEvent;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    enum Deferred {
        Steer(String),
        Abort,
    }
    let wrap_up_at = (cap as f64 * 0.9).floor() as u64;
    let turns = AtomicU64::new(0);
    let warned = AtomicBool::new(false);
    let deferred: Mutex<Vec<Deferred>> = Mutex::new(Vec::new());
    let target = session.clone();
    session.subscribe(move |event| {
        let AgentSessionEvent::Agent(event) = event else {
            return;
        };
        match event {
            AgentEvent::TurnStart => {
                let actions = std::mem::take(&mut *deferred.lock().unwrap_or_else(|e| e.into_inner()));
                for action in actions {
                    match action {
                        Deferred::Steer(text) => {
                            let _ = target.steer(&text, &[]);
                        }
                        Deferred::Abort => {
                            let session = target.clone();
                            async_runtime().spawn(async move { session.abort().await });
                        }
                    }
                }
            }
            AgentEvent::TurnEnd { .. } => {
                let turns = turns.fetch_add(1, Ordering::SeqCst) + 1;
                let mut deferred = deferred.lock().unwrap_or_else(|e| e.into_inner());
                if turns >= cap {
                    if !reached.swap(true, Ordering::SeqCst) {
                        deferred.push(Deferred::Abort);
                    }
                    return;
                }
                if wrap_up_at >= 1
                    && wrap_up_at < cap
                    && turns >= wrap_up_at
                    && !warned.swap(true, Ordering::SeqCst)
                {
                    deferred.push(Deferred::Steer(format!(
                        "You are at turn {turns} of your {cap}-turn limit. Stop investigating or making changes now and write your final summary of findings and results in your next message."
                    )));
                }
            }
            _ => {}
        }
    })
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

/// The startup resource listing for the interactive mode, from the concrete
/// resource loader and the agent registry.
fn resource_listing(
    session: &AgentSession,
) -> cortexcode_code_tui_app::resource_display::ResourceListing {
    use cortexcode_code_tui_app::resource_display::{ListedItem, ResourceListing};
    let cwd = session.cwd().to_string_lossy().into_owned();
    let quiet_startup = session.settings().quiet_startup();
    let mut listing = ResourceListing {
        cwd: cwd.clone(),
        quiet_startup,
        ..Default::default()
    };
    if let Some(resources) = last_resources() {
        let loader = resources.loader();
        let skills = loader.skills();
        listing.skills = skills
            .skills
            .iter()
            .map(|s| ListedItem {
                name: s.name.clone(),
                path: s.file_path.clone(),
                source_info: Some(s.source_info.clone()),
                display_name: None,
            })
            .collect();
        listing.skill_diagnostics = skills.diagnostics;
        let prompts = loader.prompts();
        listing.templates = prompts
            .prompts
            .iter()
            .map(|p| ListedItem {
                name: p.name.clone(),
                path: p.file_path.clone(),
                source_info: Some(p.source_info.clone()),
                display_name: None,
            })
            .collect();
        listing.prompt_diagnostics = prompts.diagnostics;
        let context = loader.agents_files();
        listing.context_files = context.agents_files;
        listing.context_warnings = context.warnings;
    }
    // Dispatchable agents only when the Task tool is on.
    if session.get_active_tool_names().iter().any(|t| t == "Task") {
        let registry = cortexcode_code_resources::agent_registry::load_agent_registry(
            &cortexcode_code_resources::agent_registry::LoadAgentRegistryOptions::new(cwd),
        );
        listing.agents = registry
            .list()
            .iter()
            .map(|a| (a.name.clone(), a.description.clone()))
            .collect();
    }
    listing
}

/// main.ts's semantic-index start, as far as cortex has it: the binary is
/// looked up (the setting, `PATH`, the agent's `bin/`) and, missing, reported
/// unavailable on the footer. Indexing itself is deferred (12.4), so a found
/// binary settles as skipped.
fn start_semantic_index(args: &Args, session: &AgentSession) {
    use cortexcode_code_tui_app::embsearch_progress::{report_embsearch_progress, EmbsearchState};
    let (enabled, configured) = {
        let settings = session.settings();
        (
            args.enable_semantic_index
                .unwrap_or_else(|| settings.enable_semantic_index()),
            settings.embsearch_binary_path(),
        )
    };
    if !enabled {
        return;
    }
    let exe = if cfg!(windows) {
        "embsearch.exe"
    } else {
        "embsearch"
    };
    let found = configured.is_some()
        || std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(exe).is_file()))
        || cortexcode_code_paths::bin_dir().join(exe).is_file();
    let state = if found {
        EmbsearchState::Skipped {
            reason: "semantic indexing is not available in this build".into(),
        }
    } else {
        EmbsearchState::Unavailable {
            reason: "embsearch binary not found (PATH or embsearchBinaryPath setting)".into(),
        }
    };
    report_embsearch_progress(&state, true, &mut |_| {});
}

/// Run the interactive mode on the TUI (`InteractiveMode`).
pub fn run_interactive_mode(
    args: &Args,
    _output: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), RuntimeError> {
    let (session, diagnostics) = build_session(args, true);
    report_diagnostics(err, crate::Env::detect().color, &diagnostics)?;
    if diagnostics
        .iter()
        .any(|(kind, _)| *kind == DiagnosticKind::Error)
    {
        return Err(RuntimeError::Setup("invalid model options".into()));
    }
    start_semantic_index(args, &session);

    let mut messages = args.messages.clone();
    let initial_message = crate::initial_message::build_initial_message(&mut messages, None, None);
    let auth = load_auth();
    let listing_session = session.clone();
    cortexcode_code_tui_app::interactive_mode::run_interactive(
        cortexcode_code_tui_app::interactive_mode::InteractiveOptions {
            session,
            runtime: async_runtime().handle().clone(),
            listing: Box::new(move || resource_listing(&listing_session)),
            is_oauth: Box::new(move |provider| {
                cortexcode_code_models::AuthLookup::is_oauth(auth.as_ref(), provider)
            }),
            version: cortexcode_code_paths::VERSION.to_string(),
            verbose: args.verbose == Some(true),
            initial_message,
            initial_messages: messages,
            model_fallback_message: None,
            terminal: None,
        },
    )
    .map_err(RuntimeError::Setup)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session over in-memory settings, models and session, in `/w`.
    fn prompt_for(argv: &[&str]) -> (String, Vec<String>) {
        let args = crate::args::parse_args(&argv.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        let agent_dir = tempfile::tempdir().unwrap();
        let (session, _) = assemble_session(
            &args,
            std::path::PathBuf::from("/w"),
            agent_dir.path().to_path_buf(),
            SettingsManager::in_memory(Default::default()),
            cortexcode_code_models::ModelRegistry::in_memory(),
            Arc::new(cortexcode_code_models::NoAuth),
            SessionManager::in_memory("/w"),
            ModelOptions::default(),
            None,
            false,
        );
        (session.system_prompt(), session.get_active_tool_names())
    }

    /// A terminal that records what is drawn and types a script of keys.
    struct ScriptedTerminal {
        script: Vec<(u64, &'static str)>,
        output: Arc<Mutex<String>>,
        title: Arc<Mutex<String>>,
    }

    impl cortexcode_tui_terminal::Terminal for ScriptedTerminal {
        fn start(
            &mut self,
            mut on_input: Box<dyn FnMut(&str) + Send>,
            _on_resize: Box<dyn FnMut() + Send>,
        ) {
            let script = std::mem::take(&mut self.script);
            std::thread::spawn(move || {
                for (delay, keys) in script {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    on_input(keys);
                }
            });
        }
        fn stop(&mut self) {}
        fn drain_input(&mut self, _max: std::time::Duration, _idle: std::time::Duration) {}
        fn write(&mut self, data: &str) {
            self.output.lock().unwrap().push_str(data);
        }
        fn columns(&self) -> u16 {
            80
        }
        fn rows(&self) -> u16 {
            30
        }
        fn kitty_protocol_active(&self) -> bool {
            false
        }
        fn move_by(&mut self, _lines: i32) {}
        fn hide_cursor(&mut self) {}
        fn show_cursor(&mut self) {}
        fn clear_line(&mut self) {}
        fn clear_from_cursor(&mut self) {}
        fn clear_screen(&mut self) {}
        fn set_title(&mut self, title: &str) {
            *self.title.lock().unwrap() = title.to_string();
        }
        fn set_progress(&mut self, _active: bool) {}
    }

    #[test]
    fn interactive_mode_draws_the_idle_screen_takes_input_and_exits_on_ctrl_d() {
        let args = crate::args::parse_args(&[]);
        let agent_dir = tempfile::tempdir().unwrap();
        let (session, _) = assemble_session(
            &args,
            std::path::PathBuf::from("/w"),
            agent_dir.path().to_path_buf(),
            SettingsManager::in_memory(Default::default()),
            cortexcode_code_models::ModelRegistry::in_memory(),
            Arc::new(cortexcode_code_models::NoAuth),
            SessionManager::in_memory("/w"),
            ModelOptions::default(),
            None,
            true,
        );
        let output = Arc::new(Mutex::new(String::new()));
        let title = Arc::new(Mutex::new(String::new()));
        let terminal = ScriptedTerminal {
            // Type, clear with ctrl+c, then leave with ctrl+d on the empty prompt.
            script: vec![(300, "hi"), (100, "\x03"), (600, "\x04")],
            output: output.clone(),
            title: title.clone(),
        };
        let listing_session = session.clone();
        let result = cortexcode_code_tui_app::interactive_mode::run_interactive(
            cortexcode_code_tui_app::interactive_mode::InteractiveOptions {
                session,
                runtime: async_runtime().handle().clone(),
                listing: Box::new(move || resource_listing(&listing_session)),
                is_oauth: Box::new(|_| false),
                version: "0.0.1".into(),
                verbose: false,
                initial_message: None,
                initial_messages: Vec::new(),
                model_fallback_message: None,
                terminal: Some(Box::new(terminal)),
            },
        );
        assert!(result.is_ok());
        let drawn = cortexcode_tui_util::strip_vt_control_characters(&output.lock().unwrap());
        assert!(drawn.contains("❯"), "{drawn}");
        assert!(drawn.contains("⬢ BUILD"), "{drawn}");
        assert!(drawn.contains("coding agent · v0.0.1"), "{drawn}");
        assert!(drawn.contains("hi"), "{drawn}");
        assert_eq!(*title.lock().unwrap(), "Cortex - w");
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
            "Available tools:\n- read: Read file contents\n- bash: Run builds, tests, linters, git, and package managers\n- edit: Make precise file edits with exact text replacement, including multiple disjoint edits in one call\n- write: Create or overwrite files\n- SearchCodebase: Ranked code search (keyword + semantic, rank-fused)\n- ask_options: Put a decision to the user as selectable options\n- Task: delegate a self-contained task to a specialized subagent (choose via subagent_type)\n- TaskOutput: check status / list / collect the results of background subagents\n- TodoWrite: Plan and track multi-step work as a live todo list (use proactively; replaces the whole list each call)\n\nGuidelines:"
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
                "Task",
                "TaskOutput",
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
