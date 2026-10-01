//! `setupMode` (extensions/core/modes.ts) as a native core extension: the
//! active mode's prompt is appended through `before_agent_start`, and the
//! `/mode`, `/plan`, `/grill`, `/goal`, `/approve` commands produce
//! [`ModeAction`]s that the UI carries out.

use crate::config::{self, HooConfig};
use crate::plan::{
    build_approve_message, build_goal_messages, build_grill_message, legacy_plan_path,
    load_plan_sections, parse_goal_args, parse_grill_target, plan_path, GrillTarget, PlanLoad,
};
use crate::prompts::{default_mode_prompt, DEFAULT_MODE};
pub use cortexcode_code_agent_session::NotifyLevel;
use cortexcode_code_agent_session::{
    CommandFuture, ExtensionCommandInfo, ExtensionHooks, ExtensionUiRequest, SessionEvent,
    SessionEventFuture, SessionEventResult,
};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// What a mode command asks the host to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeAction {
    /// `ctx.ui.notify(message, level)`.
    Notify(String, NotifyLevel),
    /// `hoo.sendUserMessage(text, { deliverAs: "followUp" })`.
    SendFollowUp(String),
    /// `ctx.reload()`: rebuild the session (the mode is re-resolved).
    Reload,
    /// `ctx.newSession({ withSession: (c) => c.sendUserMessage(text, followUp) })`.
    NewSessionWithMessage(String),
    /// `LOOP_AUTO_START` on the event bus (the autonomous loop, ledger 12.5).
    StartAutoLoop {
        task: String,
        max_turns: Option<u64>,
        continue_prompt: String,
    },
}

/// `KNOWN_MODES`.
pub const KNOWN_MODES: [&str; 4] = ["ask", "plan", "build", "debug"];

/// The mode commands with their `description`s.
pub const MODE_COMMANDS: [(&str, &str); 5] = [
    (
        "mode",
        "Switch active mode. Usage: /mode <ask|plan|build|debug>",
    ),
    ("plan", "Switch to plan mode. Shorthand for /mode plan."),
    (
        "grill",
        "Stress-test the current plan. Usage: /grill [me|plan]",
    ),
    (
        "goal",
        "Work autonomously toward a goal. Usage: /goal [--max-turns N] [objective]",
    ),
    (
        "approve",
        "Approve the current plan and switch to build mode to execute it.",
    ),
];

/// `resolveModeFile`: `<cwd>/.cortexcode/modes/<name>/system.md`, the agent
/// dir's `modes/<name>/system.md`, then each external dir; trimmed, non-empty.
fn resolve_mode_file(name: &str, cwd: &Path, external_dirs: &[String]) -> Option<String> {
    let mut candidates = vec![
        cwd.join(cortexcode_code_paths::CONFIG_DIR_NAME)
            .join("modes")
            .join(name)
            .join("system.md"),
        cortexcode_code_paths::agent_dir()
            .join("modes")
            .join(name)
            .join("system.md"),
    ];
    candidates.extend(
        external_dirs
            .iter()
            .map(|d| PathBuf::from(d).join(name).join("system.md")),
    );
    candidates.iter().find_map(|path| {
        let text = std::fs::read_to_string(path).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

/// `buildSystemPrompt(mode, cwd, { modePaths })`: an override file, else the
/// built-in prompt for the four known modes.
pub fn build_mode_system_prompt(mode: &str, cwd: &Path, mode_paths: &[String]) -> Option<String> {
    resolve_mode_file(mode, cwd, mode_paths)
        .or_else(|| default_mode_prompt(mode).map(str::to_string))
}

/// `relative(cwd, path) || path`.
fn relative_or_absolute(cwd: &Path, path: &Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// The mode resolved at session start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveMode {
    pub mode: String,
    /// The mode prompt with `{{PLAN_PATH}}` substituted; `None` = no appendix.
    pub system_prompt: Option<String>,
    pub plan_path: Option<PathBuf>,
    /// `modes[mode].enabled_tools`, for the host to activate.
    pub enabled_tools: Option<Vec<String>>,
}

/// Session inputs of the mode extension.
#[derive(Debug, Clone)]
pub struct ModeSession {
    pub cwd: PathBuf,
    pub session_id: String,
    /// Light mode strips every prompt appendix.
    pub light: bool,
    /// Extra `{name}/system.md` search dirs (`--mode-path`, extension contributions).
    pub mode_search_paths: Vec<String>,
}

/// `session_start`: resolve the mode from `hoo-config.json` (skipped in light
/// mode and inside a spawned subagent, which carries its own prompt).
pub fn resolve_active_mode(session: &ModeSession, config: &HooConfig) -> ActiveMode {
    if session.light || cortexcode_code_paths::env_override("SUBAGENT_DEPTH").is_some() {
        return ActiveMode {
            mode: DEFAULT_MODE.into(),
            ..Default::default()
        };
    }
    let mode = config::active_mode(config).unwrap_or_else(|| DEFAULT_MODE.to_string());
    let mode_paths =
        config::merge_search_paths(&[&config::mode_paths(config), &session.mode_search_paths]);
    let raw = build_mode_system_prompt(&mode, &session.cwd, &mode_paths);
    let plan = plan_path(&session.cwd, &session.session_id);
    let rel_plan = relative_or_absolute(&session.cwd, &plan);
    ActiveMode {
        system_prompt: raw.map(|p| p.replace("{{PLAN_PATH}}", &rel_plan)),
        plan_path: Some(plan),
        enabled_tools: config::mode_list(config, &mode, "enabled_tools").filter(|t| !t.is_empty()),
        mode,
    }
}

/// `before_agent_start`: the base prompt plus the mode block.
pub fn append_mode_prompt(system_prompt: &str, active: &ActiveMode) -> Option<String> {
    let mode_prompt = active.system_prompt.as_ref()?;
    Some(format!(
        "{system_prompt}\n\n<!-- hoo-core: mode={} -->\n{mode_prompt}",
        active.mode
    ))
}

/// The mode system for one session.
pub struct ModesExtension {
    session: ModeSession,
    active: Mutex<ActiveMode>,
    auto_loop_active: Mutex<bool>,
    actions: Mutex<Vec<ModeAction>>,
}

impl ModesExtension {
    /// Resolve the mode for `session` from the merged `hoo-config.json`.
    pub fn new(session: ModeSession) -> Self {
        let config = config::read_merged_config(&session.cwd);
        Self::with_config(session, &config)
    }

    /// Resolve the mode from an already-read config.
    pub fn with_config(session: ModeSession, config: &HooConfig) -> Self {
        let active = resolve_active_mode(&session, config);
        Self {
            session,
            active: Mutex::new(active),
            auto_loop_active: Mutex::new(false),
            actions: Mutex::new(Vec::new()),
        }
    }

    /// The mode resolved at the last `session_start`.
    pub fn active(&self) -> ActiveMode {
        self.active_guard().clone()
    }

    fn active_guard(&self) -> MutexGuard<'_, ActiveMode> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `session_start` (a reload): re-resolve the mode from the merged
    /// `hoo-config.json`; returns the mode's tool filter to activate.
    pub fn reresolve(&self) -> Option<Vec<String>> {
        let config = config::read_merged_config(&self.session.cwd);
        let active = resolve_active_mode(&self.session, &config);
        let tools = active.enabled_tools.clone();
        *self.active_guard() = active;
        tools
    }

    /// `LOOP_AUTO_CHANGED`: whether an autonomous loop is running.
    pub fn set_auto_loop_active(&self, active: bool) {
        *self
            .auto_loop_active
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = active;
    }

    /// Actions produced by commands run through [`ExtensionHooks::run_command`].
    pub fn take_actions(&self) -> Vec<ModeAction> {
        std::mem::take(&mut *self.actions.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn plan_candidates(&self) -> (PathBuf, Vec<PathBuf>) {
        let session_plan = self
            .active_guard()
            .plan_path
            .clone()
            .unwrap_or_else(|| plan_path(&self.session.cwd, &self.session.session_id));
        let candidates = vec![session_plan.clone(), legacy_plan_path(&self.session.cwd)];
        (session_plan, candidates)
    }

    fn rel(&self, path: &Path) -> String {
        relative_or_absolute(&self.session.cwd, path)
    }

    /// Run a mode command.
    pub fn command(&self, name: &str, args: &str) -> Vec<ModeAction> {
        use ModeAction::*;
        use NotifyLevel::*;
        match name {
            "mode" => {
                let name = args.trim();
                if name.is_empty() {
                    return vec![Notify(
                        format!("Active mode: {}", self.active_guard().mode),
                        Info,
                    )];
                }
                let mut config = config::read_config();
                if name == DEFAULT_MODE {
                    config.remove("active_mode");
                } else {
                    config.insert("active_mode".into(), name.into());
                }
                let _ = config::write_config(&config);
                vec![
                    Notify(format!("Mode set to \"{name}\" — reloading…"), Info),
                    Reload,
                ]
            }
            "plan" => {
                let mut config = config::read_config();
                config.insert("active_mode".into(), "plan".into());
                let _ = config::write_config(&config);
                vec![
                    Notify("Mode set to \"plan\" — reloading…".into(), Info),
                    Reload,
                ]
            }
            "grill" => {
                let Some(requested) = parse_grill_target(args) else {
                    return vec![Notify("Usage: /grill [me|plan]".into(), Warning)];
                };
                let (session_plan, candidates) = self.plan_candidates();
                let sections = match load_plan_sections(&candidates) {
                    PlanLoad::Error(path) => {
                        return vec![Notify(format!("Could not read {}", self.rel(&path)), Error)]
                    }
                    PlanLoad::Missing => {
                        return vec![Notify(
                            format!(
                                "No plan to grill — {} not found. Run /plan first.",
                                self.rel(&session_plan)
                            ),
                            Warning,
                        )]
                    }
                    PlanLoad::Loaded(sections) => sections,
                };
                let mut actions = Vec::new();
                let mut target = requested;
                let looping = *self
                    .auto_loop_active
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if looping && target != GrillTarget::Plan {
                    target = GrillTarget::Plan;
                    actions.push(Notify(
                        "Autonomous loop active — grilling the plan only, skipping questions."
                            .into(),
                        Info,
                    ));
                }
                actions.push(SendFollowUp(build_grill_message(&sections, target)));
                actions
            }
            "goal" => {
                let Some(parsed) = parse_goal_args(args) else {
                    return vec![Notify(
                        "Usage: /goal [--max-turns N] [objective]".into(),
                        Warning,
                    )];
                };
                let (session_plan, candidates) = self.plan_candidates();
                let sections = match load_plan_sections(&candidates) {
                    PlanLoad::Error(path) => {
                        return vec![Notify(format!("Could not read {}", self.rel(&path)), Error)]
                    }
                    PlanLoad::Loaded(sections) => Some(sections),
                    PlanLoad::Missing => None,
                };
                let objective = parsed
                    .objective
                    .or_else(|| sections.as_ref().and_then(|s| s.goal.clone()));
                let Some(objective) = objective else {
                    return vec![Notify(
                        format!(
                            "No objective — pass one as /goal <objective>, or write a Goal section in {}.",
                            self.rel(&session_plan)
                        ),
                        Warning,
                    )];
                };
                let messages = build_goal_messages(
                    &objective,
                    sections.as_ref().and_then(|s| s.verification.as_deref()),
                );
                vec![StartAutoLoop {
                    task: messages.task,
                    max_turns: parsed.max_turns,
                    continue_prompt: messages.continue_prompt,
                }]
            }
            "approve" => {
                let mode = self.active_guard().mode.clone();
                if mode != "plan" {
                    return vec![Notify(
                        format!(
                            "/approve is only available in plan mode (current mode: \"{mode}\")"
                        ),
                        Warning,
                    )];
                }
                let (session_plan, candidates) = self.plan_candidates();
                let message = match load_plan_sections(&candidates) {
                    PlanLoad::Error(path) => {
                        return vec![Notify(format!("Could not read {}", self.rel(&path)), Error)]
                    }
                    PlanLoad::Loaded(sections) => Some(build_approve_message(&sections)),
                    PlanLoad::Missing => None,
                };
                let mut config = config::read_config();
                config.insert("active_mode".into(), "build".into());
                let _ = config::write_config(&config);
                match message {
                    Some(message) => vec![NewSessionWithMessage(message)],
                    None => vec![
                        Notify(
                            format!(
                                "Switched to build mode. No {} found — describe what to build.",
                                self.rel(&session_plan)
                            ),
                            Info,
                        ),
                        Reload,
                    ],
                }
            }
            _ => Vec::new(),
        }
    }
}

impl ExtensionHooks for ModesExtension {
    fn has_command(&self, name: &str) -> bool {
        MODE_COMMANDS.iter().any(|(n, _)| *n == name)
    }

    fn run_command(&self, name: &str, args: &str) -> CommandFuture {
        let actions = self.command(name, args);
        self.actions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(actions);
        Box::pin(async { Ok(()) })
    }

    fn before_agent_start(&self, _prompt: &str, system_prompt: &str) -> Option<String> {
        append_mode_prompt(system_prompt, &self.active_guard())
    }

    fn has_handlers(&self, event_type: &str) -> bool {
        matches!(event_type, "session_start" | "before_agent_start")
    }

    fn emit_session_event(&self, event: SessionEvent) -> SessionEventFuture {
        let active_tools = match event {
            SessionEvent::Start(_) => self.reresolve(),
            _ => None,
        };
        Box::pin(async move {
            SessionEventResult {
                active_tools,
                ..Default::default()
            }
        })
    }

    fn commands(&self) -> Vec<ExtensionCommandInfo> {
        MODE_COMMANDS
            .iter()
            .map(|(name, description)| ExtensionCommandInfo {
                name: (*name).to_string(),
                description: Some((*description).to_string()),
                source_info: mode_commands_source_info(),
            })
            .collect()
    }

    fn argument_completions(&self, name: &str, prefix: &str) -> Option<Vec<String>> {
        let values: &[&str] = match name {
            "mode" => &KNOWN_MODES,
            "grill" => &["me", "plan"],
            "plan" | "goal" | "approve" => &[],
            _ => return None,
        };
        Some(
            values
                .iter()
                .filter(|v| v.starts_with(prefix))
                .map(|v| (*v).to_string())
                .collect(),
        )
    }

    fn active_mode(&self) -> Option<String> {
        // `ctx.ui.setMode` runs in session_start, which light mode and
        // subagents skip.
        let skipped =
            self.session.light || cortexcode_code_paths::env_override("SUBAGENT_DEPTH").is_some();
        (!skipped).then(|| self.active_guard().mode.clone())
    }

    fn take_ui_requests(&self) -> Vec<ExtensionUiRequest> {
        self.take_actions()
            .into_iter()
            .filter_map(|action| match action {
                ModeAction::Notify(message, level) => {
                    Some(ExtensionUiRequest::Notify(message, level))
                }
                ModeAction::SendFollowUp(text) => Some(ExtensionUiRequest::SendFollowUp(text)),
                ModeAction::Reload => Some(ExtensionUiRequest::Reload),
                ModeAction::NewSessionWithMessage(text) => {
                    Some(ExtensionUiRequest::NewSessionWithMessage(text))
                }
                // LOOP_AUTO_START has no listener until the loop extension (12.5).
                ModeAction::StartAutoLoop { .. } => None,
            })
            .collect()
    }
}

/// The `sourceInfo` of the built-in `hoo-core` extension's commands: a
/// temporary-scope extension (autocomplete tags it `[t]`).
fn mode_commands_source_info() -> serde_json::Value {
    serde_json::json!({
        "path": "<builtin:hoo-core>",
        "source": "hoo-core",
        "scope": "temporary",
        "origin": "top-level",
    })
}
