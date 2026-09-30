//! The interactive mode (`interactive-mode.ts`): builds the TUI tree —
//! banner, the rows nobody uses, transcript, notification band, prompt,
//! footer — wires the keys, and runs the submit loop against the session.
//!
//! What is here is the shell and its idle screen. The transcript widgets
//! (assistant markdown, tool blocks, the working loader) arrive in 11.2; until
//! then a turn is shown as the user's text and the agent's final text.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cortexcode_agent_compaction::CompactionResult;
use cortexcode_agent_types::{AgentEvent, AgentMessage, CompactionSummaryMessage, CustomMessage};
use cortexcode_ai_types::{AssistantMessage, Content, ImageContent, StopReason, UserContent};
use cortexcode_ai_types::{Model, ThinkingLevel, Transport};
use cortexcode_ai_util::is_long_retry_delay_error;
use cortexcode_code_agent_session::format::{format_duration_secs, format_tokens};
use cortexcode_code_agent_session::runtime::{format_missing_session_cwd_prompt, RuntimeError};
use cortexcode_code_agent_session::stats::{sum_assistant_usage, AssistantUsageTotals};
use cortexcode_code_agent_session::ToolSource;
use cortexcode_code_agent_session::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, CompactionReason, ForkPosition,
    NavigateTreeOptions, NavigateTreeResult, NewSessionRequest, PromptOptions, StreamingBehavior,
    TranscriptSelection,
};
use cortexcode_code_auth::provider_display_names::provider_auth_status;
use cortexcode_code_auth::{AuthCredential, AuthStorage};
use cortexcode_code_media::clipboard::{NativeWriter, SystemClipboardHost};
use cortexcode_code_media::clipboard_image::{
    extension_for_image_mime_type, read_clipboard_image, rgba_to_png, ClipboardImage,
    SystemClipboardImageHost,
};
use cortexcode_code_media::markdown_to_html::markdown_to_html;
use cortexcode_code_media::rich_clipboard::{copy_rich_to_clipboard, CopyFlavour, RichPayload};
use cortexcode_code_models::{
    find_exact_model_reference_match, parse_thinking_level, resolve_model_scope,
};
use cortexcode_code_paths::{APP_NAME, APP_TITLE, VERSION};
use cortexcode_code_resources::agent_registry::{load_agent_registry, LoadAgentRegistryOptions};
use cortexcode_code_resources::BUILTIN_SLASH_COMMANDS;
use cortexcode_code_session::identity::{
    cycle_session_color_slot, parse_session_color_slot, session_color_name,
    session_color_name_list, CycleDirection as SessionCycleDirection, SESSION_COLOR_SLOTS,
};
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::platform_targets::{get_workspace_platforms, set_platforms};
use cortexcode_code_settings::{ChromeDensity, DoubleEscapeAction, EditorBorder, ToolOutputView};
use cortexcode_code_subagents::agent_log::set_terminal_owned_by_tui;
use cortexcode_code_subagents::instance::get_subagent_pool;
use cortexcode_code_subagents::pool::DispatchOptions;
use cortexcode_code_task_store::{task_store, TaskStatus};
use cortexcode_code_tool_api::{truncate_tail, TruncationOptions, TruncationResult};
use cortexcode_code_tool_bash::BashResult;
use cortexcode_code_tools::external_tools::{describe_external_tools, get_tool_path};
use cortexcode_code_tools::light::{measure_prompt_surface, measure_tool_schema_tokens};
use cortexcode_code_tools_optin::todo::settle_dangling_main_tasks;
use cortexcode_code_tools_optin::AskQuestion;
use cortexcode_code_tui_keybindings::{
    app_key_label, key_display_text, key_hint, key_text, raw_key_hint, AppKeybindingsManager,
};
use cortexcode_code_tui_selectors::ask_options::{AskOptionsComponent, AskOptionsOptions};
use cortexcode_code_tui_selectors::login_dialog::{LoginDialogComponent, LoginDialogEvent};
use cortexcode_code_tui_selectors::model_selector::{ModelSelectorComponent, ModelSelectorEvent};
use cortexcode_code_tui_selectors::oauth_selector::{
    AuthSelectorProvider, AuthType, LoginMode, OAuthSelectorComponent, OAuthSelectorEvent,
};
use cortexcode_code_tui_selectors::scoped_models_selector::{
    ScopedModelsEvent, ScopedModelsSelectorComponent,
};
use cortexcode_code_tui_selectors::session_selector::{
    SessionSelectorComponent, SessionSelectorOptions,
};
use cortexcode_code_tui_selectors::settings_selector::{
    SettingsChange, SettingsConfig, SettingsSelectorComponent, ToolGroupInfo, ToolToggleInfo,
};
use cortexcode_code_tui_selectors::small_selectors::session_color_selector;
use cortexcode_code_tui_selectors::tree_selector::{TreeEvent, TreeSelectorComponent};
use cortexcode_code_tui_selectors::user_message_selector::{
    UserMessageEvent, UserMessageItem, UserMessageSelectorComponent,
};
use cortexcode_code_tui_theme::get_available_themes;
use cortexcode_code_tui_theme::{apply_block_fill, BlockFill};
use cortexcode_code_tui_theme::{
    get_editor_theme, get_markdown_theme, init_theme, on_theme_change, set_registered_themes,
    set_theme, theme, ThinkingBorderLevel,
};
use cortexcode_code_tui_widgets::bash_execution::BashExecutionComponent;
use cortexcode_code_tui_widgets::custom_message::{
    BranchSummaryMessageComponent, CompactionSummaryMessageComponent, CustomMessageComponent,
};
use cortexcode_code_tui_widgets::dynamic_border::DynamicBorder;
use cortexcode_code_tui_widgets::task_panel::{
    TaskPanelComponent, TaskPanelDensity, TaskPanelView,
};
use cortexcode_code_tui_widgets::tool_chain::ToolChainComponent;
use cortexcode_code_tui_widgets::tool_chain_summary::ChainState;
use cortexcode_code_tui_widgets::tool_execution::{ToolExecutionComponent, ToolExecutionOptions};
use cortexcode_code_tui_widgets::tool_output_view::{
    cycle_tool_output_view, DEFAULT_TOOL_OUTPUT_VIEW, MAX_TOOL_OUTPUT_VIEW,
};
use cortexcode_code_tui_widgets::tool_signal::ToolResult;
use cortexcode_code_tui_widgets::tools::registered_tool_definition;
use cortexcode_code_tui_widgets::{
    AssistantMessageComponent, ThinkingDisplay, UserMessageComponent,
};
use cortexcode_tui_components::BoxComponent;
use cortexcode_tui_components::TruncatedText;
use cortexcode_tui_components::{
    ArgumentCompletionsFn, AutocompleteItem, CombinedAutocompleteProvider, CommandEntry, Editor,
    EditorHost, EditorOptions, FrameBorderStyle, Loader, Markdown, MarkdownTheme, SelectItem,
    SlashCommand, Spacer, Text,
};
use cortexcode_tui_images::get_capabilities;
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::{
    Component, ComponentHandle, Container, FlexSpacer, Slot, Tui, TuiEvent,
};
use cortexcode_tui_terminal::Terminal;
use cortexcode_tui_util::visible_width;

use crate::changelog::{changelog_for_display, changelog_path, parse_changelog};
use crate::chrome_layout::{
    ChromeLayoutController, ChromeSurfaces, FooterLayout, TasksLayout, SMALL_TERMINAL_ROWS,
};
use crate::dialog_bridge::{set_dialog_sink, DialogRequest};
use crate::expandable_text::{Expandable, ExpandableText};
use crate::extension_editor::{EditorOutcome, ExtensionEditorComponent};
use crate::extension_selector::{ExtensionSelectorComponent, SelectorOutcome};
use crate::footer::{FooterComponent, FooterDensity, FooterModel, FooterSource};
use crate::footer_data::FooterDataProvider;
use crate::hotkeys::hotkeys_markdown;
use crate::input_frame::set_input_frame_border;
use crate::login_controller::{
    action_label, logged_out_message, login_provider_options, logout_provider_options,
    no_providers_message, open_url, post_login_model, run_oauth_login, LoginUpdate, OAuthBridge,
    PostLoginModel, API_KEY_LABEL, LOGIN_CANCELLED, NOTHING_TO_LOG_OUT, SUBSCRIPTION_LABEL,
};
use crate::notification_panel::{NotificationKind, NotificationPanel};
use crate::resource_display::{format_display_path, show_loaded_resources, ResourceListing};
use crate::session_chip::render_session_chip;
use crate::session_picker;
use crate::startup_progress;
use crate::tips::{
    render_tip, tip_ttl, TipRotation, TipRotationOptions, TipsController, TipsControllerOptions,
};
use crate::wordmark::{build_compact_wordmark, CompactWordmarkOptions};

/// How the mode is started.
pub struct InteractiveOptions {
    pub session: AgentSession,
    /// The owner of `session` that replaces it (`/resume`, alt+h). Without
    /// one the session cannot be switched.
    pub session_runtime: Option<AgentSessionRuntime>,
    /// Where agent turns run.
    pub runtime: tokio::runtime::Handle,
    /// The resource listing for a session, read when it is drawn.
    pub listing: Box<dyn Fn(&AgentSession) -> ResourceListing>,
    /// Whether a provider's stored credential is an OAuth token.
    pub is_oauth: Box<dyn Fn(&str) -> bool>,
    /// The credential store `/login` and `/logout` write (the session's own).
    pub auth_storage: Arc<AuthStorage>,
    pub version: String,
    /// Force the verbose startup banner.
    pub verbose: bool,
    pub initial_message: Option<String>,
    /// Sent with `initial_message` (the `@file` images).
    pub initial_images: Vec<ImageContent>,
    pub initial_messages: Vec<String>,
    /// Shown as a notice (the only place the remedy is named).
    pub model_fallback_message: Option<String>,
    pub terminal: Option<Box<dyn Terminal>>,
}

/// The footer's view of the session.
struct SessionFooter {
    session: AgentSession,
    is_oauth: Rc<dyn Fn(&str) -> bool>,
}

impl FooterSource for SessionFooter {
    fn usage_totals(&self) -> (u64, u64, u64, u64, f64) {
        let manager = self.session.session_manager();
        let totals = cortexcode_code_agent_session::stats::sum_assistant_usage(manager.entries());
        (
            totals.input,
            totals.output,
            totals.cache_read,
            totals.cache_write,
            totals.cost,
        )
    }

    fn context_usage(&self) -> Option<(u64, Option<f64>)> {
        self.session
            .get_context_usage()
            .map(|u| (u.context_window, u.percent))
    }

    fn model(&self) -> Option<FooterModel> {
        self.session.model().map(|m| FooterModel {
            id: m.id.clone(),
            provider: m.provider.to_string(),
            context_window: m.context_window,
            reasoning: m.reasoning,
        })
    }

    fn thinking_level(&self) -> String {
        self.session.thinking_level().as_str().to_string()
    }

    fn cwd(&self) -> String {
        self.session.cwd().to_string_lossy().into_owned()
    }

    fn display_name(&self) -> String {
        self.session.display_name()
    }

    fn reserve_tokens(&self) -> u64 {
        self.session.settings().compaction_reserve_tokens()
    }

    fn is_using_oauth(&self) -> bool {
        self.session
            .model()
            .is_some_and(|m| (self.is_oauth)(&m.provider.to_string()))
    }
}

const DEFAULT_WORKING_MESSAGE: &str = "Working...";
const DEFAULT_HIDDEN_THINKING_LABEL: &str = "Thinking...";
/// Finished tool blocks kept live; older ones are frozen.
const LIVE_TOOL_WINDOW: usize = 50;
/// Minimum gap between re-renders of the streaming message.
const STREAM_RENDER_THROTTLE: Duration = Duration::from_millis(100);

/// The open extension selector and the channel its answer goes back on.
type OpenSelector = (
    Rc<RefCell<ExtensionSelectorComponent>>,
    mpsc::Sender<Option<String>>,
);

/// The open options pane and the channel its answers go back on.
type OpenAskOptions = (
    Rc<RefCell<AskOptionsComponent>>,
    mpsc::Sender<Option<Vec<String>>>,
);

/// App actions the prompt editor raises; handled by the mode after the
/// keystroke (the editor is borrowed while it dispatches).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Interrupt,
    Clear,
    Exit,
    Suspend,
    ToolsExpand,
    ChromeForward,
    ChromeBackward,
    ThinkingForward,
    ThinkingBackward,
    ModelForward,
    ModelBackward,
    ModelSelect,
    SettingsOpen,
    ViewForward,
    ViewBackward,
    /// The editor submitted this text (it has already cleared itself).
    Submit(String),
    AutocompleteVisibility(bool),
    /// The extension selector closed.
    SelectorDone(SelectorOutcome),
    /// `app.session.resume`: open the session selector.
    ResumeSession,
    /// The session selector closed, with the chosen file.
    SessionSelectorDone(Option<PathBuf>),
    /// The options pane closed: the answers, or `None` when skipped.
    AskOptionsDone(Option<Vec<String>>),
    /// The multi-line editor dialog closed.
    EditorDialogDone(EditorOutcome),
    /// The prompt text changed (`onChange`).
    EditorChanged(String),
    /// `app.clipboard.copyMessage`: `/copy` with no argument.
    CopyMessage,
    /// `app.clipboard.pasteImage`.
    PasteImage,
    /// `app.hotkeys.open`: `/hotkeys`.
    HotkeysOpen,
    /// `app.session.color.cycleForward` / `cycleBackward`.
    SessionColorForward,
    SessionColorBackward,
    /// The colour picker moved to this slot (live preview).
    SessionColorPreview(u8),
    /// The colour picker closed: the chosen slot, or `None` when cancelled.
    SessionColorDone(Option<u8>),
    /// `app.session.changeDirectory`: prefill `/cd `.
    ChangeDirectoryPrefill,
    /// `app.session.fork`: `/fork`.
    ForkOpen,
    /// `app.message.followUp`: queue the prompt as a follow-up.
    FollowUp,
    /// `app.message.dequeue`: every queued message back to the prompt.
    Dequeue,
    /// `app.tasks.cycleForward` / `cycleBackward`: step the task panel lens.
    TasksForward,
    TasksBackward,
}

/// A question waiting on a dialog: the tree entry it is about, and where
/// the answer arrives.
type PendingTreeAnswer = (String, mpsc::Receiver<Option<String>>);

/// A summarizing tree navigation in flight: the target and its result.
type TreeNavigation = (String, mpsc::Receiver<Result<NavigateTreeResult, String>>);

/// The open editor dialog and where its text goes.
type OpenEditorDialog = (
    Rc<RefCell<ExtensionEditorComponent>>,
    mpsc::Sender<Option<String>>,
);

/// Bindings the prompt answers to, and their action (`CustomEditor.onAction`).
const EDITOR_ACTIONS: [(&str, Action); 23] = [
    ("app.tasks.cycleForward", Action::TasksForward),
    ("app.tasks.cycleBackward", Action::TasksBackward),
    ("app.message.followUp", Action::FollowUp),
    ("app.message.dequeue", Action::Dequeue),
    (
        "app.session.changeDirectory",
        Action::ChangeDirectoryPrefill,
    ),
    ("app.session.fork", Action::ForkOpen),
    (
        "app.session.color.cycleForward",
        Action::SessionColorForward,
    ),
    (
        "app.session.color.cycleBackward",
        Action::SessionColorBackward,
    ),
    ("app.hotkeys.open", Action::HotkeysOpen),
    ("app.clipboard.copyMessage", Action::CopyMessage),
    ("app.view.cycleForward", Action::ViewForward),
    ("app.view.cycleBackward", Action::ViewBackward),
    ("app.clear", Action::Clear),
    ("app.suspend", Action::Suspend),
    ("app.tools.expand", Action::ToolsExpand),
    ("app.chrome.cycleForward", Action::ChromeForward),
    ("app.chrome.cycleBackward", Action::ChromeBackward),
    ("app.thinking.cycleForward", Action::ThinkingForward),
    ("app.model.cycleForward", Action::ModelForward),
    ("app.model.cycleBackward", Action::ModelBackward),
    ("app.model.select", Action::ModelSelect),
    ("app.settings.open", Action::SettingsOpen),
    ("app.session.resume", Action::ResumeSession),
];

/// The prompt editor with the app's key dispatch in front of it
/// (`CustomEditor`).
struct CustomEditor {
    editor: Editor,
    actions: Rc<RefCell<Vec<Action>>>,
}

fn is_plain_text(data: &str) -> bool {
    !data.is_empty() && data.chars().all(|c| (c as u32) >= 32 && c as u32 != 127)
}

impl Component for CustomEditor {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.editor.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        if is_plain_text(data) {
            self.editor.handle_input(data);
            return;
        }
        let kb = get_keybindings();
        if kb.matches(data, "app.clipboard.pasteImage") {
            self.actions.borrow_mut().push(Action::PasteImage);
            return;
        }
        if kb.matches(data, "app.interrupt") {
            if !self.editor.is_showing_autocomplete() {
                self.actions.borrow_mut().push(Action::Interrupt);
                return;
            }
            self.editor.handle_input(data);
            return;
        }
        if kb.matches(data, "app.exit") && self.editor.get_text().is_empty() {
            self.actions.borrow_mut().push(Action::Exit);
            return;
        }
        let extra = [("app.thinking.cycleBackward", Action::ThinkingBackward)];
        for (id, action) in EDITOR_ACTIONS.iter().chain(extra.iter()) {
            if kb.matches(data, id) {
                self.actions.borrow_mut().push(action.clone());
                return;
            }
        }
        self.editor.handle_input(data);
    }

    fn invalidate(&mut self) {
        self.editor.invalidate();
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.editor.set_focused(focused);
    }
}

/// Events from outside the UI thread.
enum AppEvent {
    /// A session event; `agent_start` carries the usage totals sampled when
    /// it fired (the turn-cost anchor), since the UI thread sees it later.
    Session(
        Box<AgentSessionEvent>,
        Option<(AssistantUsageTotals, Instant)>,
    ),
    PromptDone(Result<(), String>),
    Rerender,
    ThemeChanged,
    /// A question or notice from off the UI thread (the permission gate).
    Dialog(DialogRequest),
    /// A step of a running OAuth login.
    Login(LoginUpdate),
    /// Output from the running `!` command.
    BashChunk(String),
    /// The `!` command ended.
    BashDone(Result<BashResult, String>),
    /// A `/copy` write ended: what was copied, and the flavour that landed.
    CopyDone(String, Result<CopyFlavour, String>),
    /// The clipboard image read ended.
    PastedImage(Option<ClipboardImage>),
    /// A `/subagent` run ended: its mode, and its summary or error.
    SubagentDone(String, Result<Option<String>, String>),
    /// Queued messages sent after a compaction failed; they go back.
    CompactionQueueFailed(Vec<(String, StreamingBehavior)>, String),
    /// A message typed while streaming could not be queued.
    QueueError(String),
}

/// A pane's answers, collected by its `on_done`.
type Outcomes = Rc<RefCell<Vec<SelectorOutcome>>>;

/// Where a `/login` or `/logout` is (`LoginController`).
enum LoginStep {
    /// "authentication method".
    AuthType(Rc<RefCell<ExtensionSelectorComponent>>, Outcomes),
    /// A provider pane.
    Provider {
        selector: Rc<RefCell<OAuthSelectorComponent>>,
        mode: LoginMode,
        options: Vec<AuthSelectorProvider>,
    },
    /// The API-key dialog.
    ApiKey {
        dialog: Rc<RefCell<LoginDialogComponent>>,
        provider: AuthSelectorProvider,
        had_model: bool,
    },
    /// An OAuth login running on the async runtime.
    OAuth(Box<OAuthLogin>),
}

/// The dialog of a running OAuth login and the answers it owes.
struct OAuthLogin {
    dialog: Rc<RefCell<LoginDialogComponent>>,
    provider: AuthSelectorProvider,
    had_model: bool,
    /// `onPrompt`'s answer.
    prompt: Option<tokio::sync::oneshot::Sender<String>>,
    /// A pasted redirect URL (`onManualCodeInput`).
    manual: Option<tokio::sync::oneshot::Sender<String>>,
    /// An `onSelect` pane over the dialog.
    select: Option<OAuthSelect>,
}

struct OAuthSelect {
    outcomes: Outcomes,
    options: Vec<cortexcode_ai_oauth::OAuthSelectOption>,
    reply: tokio::sync::oneshot::Sender<Option<String>>,
}

/// A `{ truncated: true }` result for a `!` row: the row only reads the flag.
fn truncated_marker(content: &str) -> TruncationResult {
    TruncationResult {
        truncated: true,
        ..truncate_tail(content, TruncationOptions::default())
    }
}

/// The native clipboard write (`clipboard-native.ts`), only where a display
/// is available. `copy_to_clipboard` never uses it on Linux, where the
/// platform tools keep selection ownership and the native library does not.
fn native_clipboard_writer() -> Option<NativeWriter> {
    SystemClipboardHost::has_native_display().then(|| {
        Box::new(|text: &str| {
            arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(text.to_string()))
                .map_err(|e| e.to_string())
        }) as NativeWriter
    })
}

/// The native clipboard's image as PNG (`clipboard.getImageBinary`).
fn native_clipboard_image_reader() -> Option<Box<dyn Fn() -> Option<Vec<u8>>>> {
    SystemClipboardHost::has_native_display().then(|| {
        Box::new(|| {
            let image = arboard::Clipboard::new().ok()?.get_image().ok()?;
            rgba_to_png(
                image.width as u32,
                image.height as u32,
                image.bytes.into_owned(),
            )
        }) as Box<dyn Fn() -> Option<Vec<u8>>>
    })
}

/// `/model <prefix>` completions: the scoped models, else the available
/// ones, fuzzy-matched on id and provider (so "opus anthropic" matches).
fn model_argument_completions(
    session: &AgentSession,
    prefix: &str,
) -> Option<Vec<AutocompleteItem>> {
    let scoped = session.scoped_models();
    let models: Vec<Model> = if scoped.is_empty() {
        session.get_available_models()
    } else {
        scoped.into_iter().map(|s| s.model).collect()
    };
    if models.is_empty() {
        return None;
    }
    let filtered =
        cortexcode_tui_fuzzy::fuzzy_filter(&models, prefix, |m| format!("{} {}", m.id, m.provider));
    if filtered.is_empty() {
        return None;
    }
    Some(
        filtered
            .into_iter()
            .map(|m| AutocompleteItem {
                value: format!("{}/{}", m.provider, m.id),
                label: m.id,
                description: Some(m.provider),
            })
            .collect(),
    )
}

/// `getPathArgument`: the first argument after `command`, quoted or not.
pub fn command_path_argument(text: &str, command: &str) -> Option<String> {
    let args = text.strip_prefix(command)?.strip_prefix(' ')?.trim_start();
    let first = args.chars().next()?;
    if first == '"' || first == '\'' {
        let rest = &args[1..];
        return rest.find(first).map(|end| rest[..end].to_string());
    }
    Some(
        args.split(char::is_whitespace)
            .next()
            .unwrap_or(args)
            .to_string(),
    )
}

/// `os.homedir()`.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `path.resolve(base, path)`: absolute and normalized, without touching
/// the filesystem.
fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    let joined = base.join(path);
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `getChangeDirectoryCompletions`: directories for `/cd <prefix>`, plus
/// `-` and `~` when nothing is typed yet. At most 50.
fn change_directory_completions(
    cwd: &Path,
    previous_cwd: Option<&Path>,
    home: &Path,
    prefix: &str,
) -> Vec<AutocompleteItem> {
    let expanded = match prefix.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.to_string_lossy()),
        None => prefix.to_string(),
    };
    let ends_with_sep = expanded.ends_with('/') || expanded.ends_with(std::path::MAIN_SEPARATOR);
    let (base, partial) = if ends_with_sep {
        (expanded.clone(), String::new())
    } else {
        let path = Path::new(&expanded);
        let base = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_string_lossy().into_owned(),
            _ if expanded.starts_with('/') => "/".to_string(),
            _ => ".".to_string(),
        };
        let partial = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (base, partial)
    };
    let search_dir = if Path::new(&base).is_absolute() {
        PathBuf::from(&base)
    } else {
        resolve_path(cwd, Path::new(if base.is_empty() { "." } else { &base }))
    };

    let mut completions = Vec::new();
    if prefix.is_empty() {
        if let Some(previous) = previous_cwd {
            completions.push(AutocompleteItem {
                value: "-".into(),
                label: format!("- ({})", previous.display()),
                description: None,
            });
        }
        completions.push(AutocompleteItem {
            value: "~".into(),
            label: format!("~ ({})", home.display()),
            description: None,
        });
    }
    let Ok(entries) = std::fs::read_dir(&search_dir) else {
        return completions;
    };
    // Node's readdirSync (libuv scandir) returns names sorted.
    let mut dirs: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    dirs.sort();
    for name in dirs {
        if !partial.is_empty() && !name.starts_with(&partial) {
            continue;
        }
        if partial.is_empty() && name.starts_with('.') {
            continue;
        }
        let joined = if !ends_with_sep {
            let dir = Path::new(prefix)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            match dir.as_str() {
                "" | "." => name.clone(),
                "/" => format!("/{name}"),
                _ => format!("{dir}/{name}"),
            }
        } else {
            format!("{prefix}{name}")
        };
        completions.push(AutocompleteItem {
            value: format!("{joined}/"),
            label: format!("{name}/"),
            description: None,
        });
    }
    completions.truncate(50);
    completions
}

/// The first version header in a changelog excerpt.
static CHANGELOG_VERSION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"##\s+\[?(\d+\.\d+\.\d+)\]?").expect("static pattern")
});

/// `new Date().toISOString()`.
fn iso_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn handle<C: Component + 'static>(c: C) -> Rc<RefCell<C>> {
    Rc::new(RefCell::new(c))
}

fn as_component<C: Component + 'static>(c: &Rc<RefCell<C>>) -> ComponentHandle {
    c.clone()
}

fn thinking_border_level(level: &str) -> ThinkingBorderLevel {
    match level {
        "minimal" => ThinkingBorderLevel::Minimal,
        "low" => ThinkingBorderLevel::Low,
        "medium" => ThinkingBorderLevel::Medium,
        "high" => ThinkingBorderLevel::High,
        "xhigh" => ThinkingBorderLevel::Xhigh,
        _ => ThinkingBorderLevel::Off,
    }
}

/// The banner's logo (the compact wordmark, or the name on a narrow screen).
fn logo(columns: u16, version: &str, cwd: &str) -> String {
    let t = theme();
    if columns < 40 {
        return t.bold(&t.fg("accent", APP_NAME)) + &t.fg("dim", &format!(" v{version}"));
    }
    let accent = |s: &str| theme().fg("accent", s);
    let glyph = |s: &str| theme().fg("text", s);
    let dim = |s: &str| theme().fg("dim", s);
    let muted = |s: &str| theme().fg("muted", s);
    let cursor = |s: &str| {
        let t = theme();
        t.blink(&t.fg("accent", s))
    };
    let note = || theme().fg("dim", &format!("  {} more", key_text("app.tools.expand")));
    build_compact_wordmark(&CompactWordmarkOptions {
        app_name: APP_NAME,
        version,
        cwd,
        tagline: None,
        accent: &accent,
        glyph: Some(&glyph),
        dim: &dim,
        muted: &muted,
        cursor: Some(&cursor),
        note: Some(&note),
    })
}

/// The expanded banner: the keys, grouped as the map is.
fn expanded_instructions() -> String {
    let hint = |id: &str, description: &str| key_hint(id, description);
    let dial = |forward: &str, backward: &str, subject: &str| {
        raw_key_hint(
            &format!("{}/{}", app_key_label(forward), app_key_label(backward)),
            &format!("to step {subject}"),
        )
    };
    let group = |title: &str| theme().fg("dim", &format!("\n{title}"));
    [
        group("Compose — the message in your hands"),
        hint("app.editor.external", "for external editor"),
        hint("app.input.voiceTranscribe", "to speak instead of type"),
        hint("app.clipboard.pasteImage", "to paste image"),
        hint("app.message.followUp", "to queue follow-up"),
        hint("app.message.dequeue", "to edit all queued messages"),
        raw_key_hint("drop files", "to attach"),
        raw_key_hint("/", "for commands"),
        raw_key_hint("!", "to run bash"),
        raw_key_hint("!!", "to run bash (no context)"),
        group("Steer — what the agent is before it runs"),
        dial(
            "app.mode.cycleForward",
            "app.mode.cycleBackward",
            "agent mode (ask/plan/build/debug)",
        ),
        dial(
            "app.model.cycleForward",
            "app.model.cycleBackward",
            "model — /model to pick one",
        ),
        dial(
            "app.thinking.cycleForward",
            "app.thinking.cycleBackward",
            "thinking level",
        ),
        group("Read — what you see of what it did"),
        dial(
            "app.view.cycleForward",
            "app.view.cycleBackward",
            "tool output (radar/peek/full)",
        ),
        dial(
            "app.tasks.cycleForward",
            "app.tasks.cycleBackward",
            "task panel view",
        ),
        hint("app.tools.expand", "to jump to full output and back"),
        hint("app.thinking.toggle", "to show or hide thinking"),
        group("Go — sessions and places"),
        hint("app.session.resume", "to resume a session"),
        hint("app.session.changeDirectory", "to change working directory"),
        dial(
            "app.session.color.cycleForward",
            "app.session.color.cycleBackward",
            "session colour",
        ),
        hint("app.settings.open", "for settings"),
        hint("app.hotkeys.open", "for all shortcuts"),
        group("Flow — getting out, getting back"),
        hint("app.interrupt", "to interrupt"),
        hint("app.clear", "to clear"),
        raw_key_hint(&format!("{} twice", key_text("app.clear")), "to exit"),
        hint("app.exit", "to exit (empty)"),
        hint("app.suspend", "to suspend"),
        key_hint("tui.editor.deleteToLineEnd", "to delete to end"),
    ]
    .join("\n")
}

struct Mode {
    session: AgentSession,
    runtime: tokio::runtime::Handle,
    tui: Tui,
    verbose: bool,
    size: Rc<Cell<(u16, u16)>>,
    dirty: Rc<Cell<bool>>,
    header: Rc<RefCell<ExpandableText>>,
    chat: Rc<RefCell<Container>>,
    status: Rc<RefCell<Container>>,
    loader: Option<Rc<RefCell<Loader>>>,
    streaming: Option<Rc<RefCell<AssistantMessageComponent>>>,
    streaming_message: Option<AssistantMessage>,
    /// Throttle for re-rendering the in-flight message: when the last run was,
    /// and whether an update is waiting for the window to pass.
    stream_render_at: Option<Instant>,
    stream_render_pending: bool,
    turn_cost_anchor: Option<(AssistantUsageTotals, Instant)>,
    turn_stop_reason: Option<StopReason>,
    tool_output_view: ToolOutputView,
    /// Where `app.tools.expand` jumped from, so the same key goes back there.
    view_before_jump: Option<ToolOutputView>,
    hide_thinking_block: bool,
    /// Tool blocks by call id, until their execution ends.
    pending_tools: HashMap<String, Rc<RefCell<ToolExecutionComponent>>>,
    /// The chain collecting tool calls, if the agent is mid-run.
    open_chain: Option<Rc<RefCell<ToolChainComponent>>>,
    /// Every chain and assistant message in the transcript, in order.
    chains: Vec<Rc<RefCell<ToolChainComponent>>>,
    assistant_components: Vec<Rc<RefCell<AssistantMessageComponent>>>,
    latest_block: Option<Rc<RefCell<ToolExecutionComponent>>>,
    latest_chain: Option<Rc<RefCell<ToolChainComponent>>>,
    chain_closed_for_current_message: bool,
    dial_reverse_taught: HashSet<&'static str>,
    /// The last status line, updated in place when nothing followed it.
    last_status: Option<(ComponentHandle, Rc<RefCell<Text>>)>,
    /// When running tool blocks that tick (bash's `Elapsed`) last re-rendered.
    last_tool_tick: Instant,
    show_images: bool,
    image_width_cells: u32,
    code_block_indent: String,
    editor: Rc<RefCell<CustomEditor>>,
    editor_container: Rc<RefCell<Container>>,
    /// The open extension selector and where its answer goes.
    selector: Option<OpenSelector>,
    actions: Rc<RefCell<Vec<Action>>>,
    notifications: Rc<RefCell<NotificationPanel>>,
    /// Tips on the notification band (`tips.rs`).
    tips: TipsController,
    footer: Rc<RefCell<FooterComponent>>,
    footer_data: FooterDataProvider,
    chrome: ChromeLayoutController,
    expanded: bool,
    last_sigint: Option<Instant>,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
    exit_requested: bool,
    listing: Box<dyn Fn(&AgentSession) -> ResourceListing>,
    session_runtime: Option<AgentSessionRuntime>,
    subscription: Option<cortexcode_code_agent_session::SessionSubscription>,
    is_oauth: Rc<dyn Fn(&str) -> bool>,
    /// The credential store `/login` and `/logout` write.
    auth_storage: Arc<AuthStorage>,
    /// A `/login` or `/logout` in progress.
    login: Option<LoginStep>,
    /// The open session selector (alt+h).
    session_selector: Option<Rc<RefCell<SessionSelectorComponent>>>,
    /// A resume waiting on the missing-cwd confirm: the session file, the
    /// cwd to fall back to, and where the answer arrives.
    pending_cwd_prompt: Option<(PathBuf, String, mpsc::Receiver<Option<String>>)>,
    /// The open options pane (`ask_options`) and where its answers go.
    ask_options: Option<OpenAskOptions>,
    /// The open session tree, with the leaf it was opened on.
    tree_selector: Option<(Rc<RefCell<TreeSelectorComponent>>, Option<String>)>,
    /// The open `/model` picker.
    model_selector: Option<Rc<RefCell<ModelSelectorComponent>>>,
    /// The open `/scoped-models` picker, with how many models it lists.
    scoped_models_selector: Option<(Rc<RefCell<ScopedModelsSelectorComponent>>, usize)>,
    /// The Anthropic extra-usage notice has been shown this session.
    anthropic_warning_shown: bool,
    /// The open `/settings` pane, and the changes it has asked for.
    settings_selector: Option<Rc<RefCell<SettingsSelectorComponent>>>,
    settings_changes: Rc<RefCell<Vec<SettingsChange>>>,
    /// When escape last hit an empty, idle prompt (`lastEscapeTime`).
    last_escape: Option<Instant>,
    /// A tree selection waiting on "Summarize branch?".
    pending_tree_summary: Option<PendingTreeAnswer>,
    /// A tree selection waiting on custom summarization instructions.
    pending_tree_instructions: Option<PendingTreeAnswer>,
    /// A navigation that summarizes, running off the input loop.
    tree_navigation: Option<TreeNavigation>,
    /// The open multi-line editor dialog (`showEditor`).
    editor_dialog: Option<OpenEditorDialog>,
    /// Rows started while the agent streams, until the next prompt.
    pending_messages: Rc<RefCell<Container>>,
    /// The prompt starts with `!` (`isBashMode`).
    is_bash_mode: bool,
    /// The running `!` command's row (`bashComponent`).
    bash_component: Option<Rc<RefCell<BashExecutionComponent>>>,
    /// `!` rows parked in the pending area (`pendingBashComponents`).
    pending_bash_components: Vec<Rc<RefCell<BashExecutionComponent>>>,
    /// Every `!` row in the transcript, for the expand sweep.
    bash_components: Vec<Rc<RefCell<BashExecutionComponent>>>,
    /// Every branch summary in the transcript, for the expand sweep.
    branch_summaries: Vec<Rc<RefCell<BranchSummaryMessageComponent>>>,
    /// Every compaction summary in the transcript, for the expand sweep.
    compaction_summaries: Vec<Rc<RefCell<CompactionSummaryMessageComponent>>>,
    /// The compaction spinner (`autoCompactionLoader`).
    compaction_loader: Option<Rc<RefCell<Loader>>>,
    /// Messages typed while a compaction runs (`compactionQueuedMessages`).
    compaction_queue: Vec<(String, StreamingBehavior)>,
    /// The task ledger above the prompt.
    task_panel: Rc<RefCell<TaskPanelComponent>>,
    /// Re-renders on task-store changes.
    task_store_subscription: Option<cortexcode_code_task_store::Subscription<'static>>,
    /// The open `/fork` message picker.
    fork_selector: Option<Rc<RefCell<UserMessageSelectorComponent>>>,
    /// Where `/cd -` returns to (`previousCwd`), shared with the `/cd`
    /// completions.
    previous_cwd: Rc<RefCell<Option<PathBuf>>>,
    /// An `/import` waiting on a confirm: the input path, the fallback cwd
    /// once the stored one turned out missing, and where the answer arrives.
    pending_import: Option<(String, Option<String>, mpsc::Receiver<Option<String>>)>,
}

impl Mode {
    fn new(options: InteractiveOptions) -> Self {
        let session = options.session;
        let settings = session.settings();
        let (
            show_hardware_cursor,
            clear_on_shrink,
            theme_name,
            editor_border,
            editor_padding_x,
            autocomplete_max_visible,
            compaction_enabled,
            tool_output_view,
            chrome_density,
            hide_thinking_block,
            code_block_indent,
            show_images,
            image_width_cells,
        ) = (
            settings.show_hardware_cursor(),
            settings.clear_on_shrink(),
            settings.theme(),
            settings.editor_border(),
            settings.editor_padding_x() as usize,
            settings.autocomplete_max_visible() as usize,
            settings.compaction_enabled(),
            settings.tool_output_view(),
            settings.chrome_density(),
            settings.hide_thinking_block(),
            settings.code_block_indent(),
            settings.show_images(),
            settings.image_width_cells() as u32,
        );
        drop(settings);
        let terminal = options
            .terminal
            .unwrap_or_else(|| Box::new(cortexcode_tui_terminal::ProcessTerminal::new()));
        let size = Rc::new(Cell::new((terminal.columns(), terminal.rows())));
        let mut tui = Tui::new(terminal, Some(show_hardware_cursor));
        tui.set_clear_on_shrink(clear_on_shrink);

        let keybindings = AppKeybindingsManager::create(None);
        keybindings.install();

        // Themes: the settings' theme (retired names resolve), watched.
        set_registered_themes(Vec::new());
        init_theme(theme_name.as_deref(), true);

        let dirty = Rc::new(Cell::new(true));
        let actions = Rc::new(RefCell::new(Vec::new()));
        let border = match editor_border {
            EditorBorder::Box => FrameBorderStyle::Box,
            EditorBorder::Rule => FrameBorderStyle::Rule,
        };
        set_input_frame_border(border);
        let rows = size.clone();
        let render_flag = dirty.clone();
        let mut editor = Editor::new(
            EditorHost {
                rows: Box::new(move || rows.get().1),
                request_render: Box::new(move || render_flag.set(true)),
            },
            get_editor_theme(),
            EditorOptions {
                padding_x: Some(editor_padding_x),
                autocomplete_max_visible: Some(autocomplete_max_visible),
                border: Some(border),
            },
        );
        editor.prompt_prefix = "❯".into();
        let sink = actions.clone();
        editor.on_submit = Some(Box::new(move |text: &str| {
            sink.borrow_mut().push(Action::Submit(text.to_string()))
        }));
        let sink = actions.clone();
        editor.on_change = Some(Box::new(move |text: &str| {
            sink.borrow_mut()
                .push(Action::EditorChanged(text.to_string()))
        }));
        let sink = actions.clone();
        editor.on_autocomplete_visibility_change = Some(Box::new(move |visible| {
            sink.borrow_mut()
                .push(Action::AutocompleteVisibility(visible))
        }));
        let editor = handle(CustomEditor {
            editor,
            actions: actions.clone(),
        });

        let footer_data = FooterDataProvider::new(session.cwd());
        footer_data
            .set_subagent_enabled(session.get_active_tool_names().iter().any(|t| t == "Task"));
        let is_oauth: Rc<dyn Fn(&str) -> bool> = Rc::from(options.is_oauth);
        let mut footer = FooterComponent::new(
            Box::new(SessionFooter {
                session: session.clone(),
                is_oauth: is_oauth.clone(),
            }),
            footer_data.clone(),
        );
        footer.set_auto_compact_enabled(compaction_enabled);
        footer.set_tool_output_view(tool_output_view);
        let footer = handle(footer);
        let footer_slot = handle(Slot::new(as_component(&footer)));
        let task_panel = handle(TaskPanelComponent::new());
        let tasks_slot = handle(Slot::new(as_component(&task_panel)));
        let panel_for_density = task_panel.clone();
        let footer_for_density = footer.clone();
        let density = chrome_density.unwrap_or(if size.get().1 < SMALL_TERMINAL_ROWS {
            ChromeDensity::Compact
        } else {
            ChromeDensity::Full
        });
        let mut chrome = ChromeLayoutController::new(
            ChromeSurfaces {
                footer_slot: footer_slot.clone(),
                tasks_slot: tasks_slot.clone(),
                set_footer_density: Box::new(move |d| {
                    footer_for_density
                        .borrow_mut()
                        .set_density(if d == FooterLayout::Line {
                            FooterDensity::Line
                        } else {
                            FooterDensity::Full
                        })
                }),
                set_tasks_density: Box::new(move |d| {
                    panel_for_density
                        .borrow_mut()
                        .set_density(if d == TasksLayout::Summary {
                            TaskPanelDensity::Summary
                        } else {
                            TaskPanelDensity::Full
                        })
                }),
            },
            density,
        );
        chrome.apply();

        let render_flag = dirty.clone();
        let budget = size.clone();
        let notifications = handle(NotificationPanel::new(
            move || render_flag.set(true),
            Some(Box::new(move || (budget.get().1 / 3) as usize)),
        ));

        // Tips: one small thing about cortex, on the band, at a moment that was
        // being spent anyway. `band_is_free` keeps it the lowest-priority thing
        // on screen: a tip is dropped rather than queued behind anything.
        let tips = {
            let (s1, s2, s3, s4, s5) = (
                session.clone(),
                session.clone(),
                session.clone(),
                session.clone(),
                session.clone(),
            );
            let (band, show) = (notifications.clone(), notifications.clone());
            TipsController::new(TipsControllerOptions::new(
                Box::new(move || s1.settings().tips_enabled()),
                Box::new(move || band.borrow().showing().is_none()),
                Box::new(move |tip| {
                    let rendered = render_tip(tip);
                    let body: Vec<&str> = rendered.body.iter().map(String::as_str).collect();
                    show.borrow_mut().notify(
                        NotificationKind::Info,
                        &rendered.title,
                        &body,
                        rendered.note.as_deref(),
                        Some(tip_ttl(body.len())),
                        Some("tip"),
                    );
                }),
                TipRotation::new(
                    TipRotationOptions {
                        seen: Box::new(move || s2.settings().seen_tips()),
                        mark_seen: Box::new(move |id| s3.settings().mark_tip_seen(id)),
                        star_nudge_count: Box::new(move || s4.settings().star_nudge_count()),
                        mark_star_nudge: Box::new(move || s5.settings().record_star_nudge()),
                    },
                    None,
                ),
            ))
        };

        let expanded = options.verbose || tool_output_view == MAX_TOOL_OUTPUT_VIEW;
        let (version, cwd) = (
            options.version.clone(),
            format_display_path(&session.cwd().to_string_lossy()),
        );
        let columns = size.get().0;
        let (v1, c1, v2, c2) = (version.clone(), cwd.clone(), version.clone(), cwd.clone());
        let header = handle(ExpandableText::new(
            move || logo(columns, &v1, &c1),
            move || {
                let onboarding = theme().fg(
                    "dim",
                    &format!(
                        "{APP_NAME} can explain its own features and look up its docs. Ask it how to use or extend {APP_NAME}."
                    ),
                );
                format!(
                    "{}\n{}\n\n{onboarding}",
                    logo(columns, &v2, &c2),
                    expanded_instructions()
                )
            },
            expanded,
            0,
            0,
        ));

        let chat = handle(Container::new());
        let header_container = handle(Container::new());
        header_container
            .borrow_mut()
            .add_child(as_component(&header));
        let screen_fill = handle(FlexSpacer::new());
        let widget_above = handle(Container::new());
        widget_above
            .borrow_mut()
            .add_child(as_component(&handle(Spacer::new(1))));
        let editor_container = handle(Container::new());
        editor_container
            .borrow_mut()
            .add_child(as_component(&editor));

        tui.add_child(as_component(&header_container));
        tui.add_child(as_component(&screen_fill));
        tui.set_flex_spacer(Some(screen_fill.clone()));
        tui.add_child(as_component(&chat));
        let pending_messages = handle(Container::new());
        tui.add_child(as_component(&pending_messages));
        let status = handle(Container::new());
        tui.add_child(as_component(&status));
        tui.add_child(as_component(&widget_above));
        tui.add_child(tasks_slot.clone());
        tui.add_child(as_component(&notifications));
        tui.add_child(as_component(&editor_container));
        tui.add_child(as_component(&handle(Container::new()))); // widgets below
        tui.add_child(footer_slot.clone());
        tui.set_focus(Some(as_component(&editor)));

        let (tx, rx) = mpsc::channel();
        Self {
            session,
            runtime: options.runtime,
            tui,
            verbose: options.verbose,
            size,
            dirty,
            header,
            chat,
            status,
            loader: None,
            streaming: None,
            streaming_message: None,
            stream_render_at: None,
            stream_render_pending: false,
            turn_cost_anchor: None,
            turn_stop_reason: None,
            tool_output_view,
            view_before_jump: None,
            hide_thinking_block,
            pending_tools: HashMap::new(),
            open_chain: None,
            chains: Vec::new(),
            assistant_components: Vec::new(),
            latest_block: None,
            latest_chain: None,
            chain_closed_for_current_message: false,
            dial_reverse_taught: HashSet::new(),
            last_status: None,
            last_tool_tick: Instant::now(),
            show_images,
            image_width_cells,
            code_block_indent,
            editor,
            editor_container,
            selector: None,
            actions,
            notifications,
            tips,
            footer,
            footer_data,
            chrome,
            expanded,
            last_sigint: None,
            tx,
            rx,
            exit_requested: false,
            listing: options.listing,
            session_runtime: options.session_runtime,
            subscription: None,
            is_oauth,
            auth_storage: options.auth_storage,
            login: None,
            session_selector: None,
            pending_cwd_prompt: None,
            ask_options: None,
            tree_selector: None,
            model_selector: None,
            scoped_models_selector: None,
            anthropic_warning_shown: false,
            settings_selector: None,
            settings_changes: Rc::default(),
            last_escape: None,
            pending_tree_summary: None,
            pending_tree_instructions: None,
            tree_navigation: None,
            editor_dialog: None,
            pending_messages,
            is_bash_mode: false,
            bash_component: None,
            pending_bash_components: Vec::new(),
            bash_components: Vec::new(),
            branch_summaries: Vec::new(),
            compaction_summaries: Vec::new(),
            compaction_loader: None,
            compaction_queue: Vec::new(),
            task_panel,
            task_store_subscription: None,
            fork_selector: None,
            previous_cwd: Rc::new(RefCell::new(None)),
            pending_import: None,
        }
    }

    fn update_editor_border_color(&mut self) {
        self.editor.borrow_mut().editor.border_color = if self.is_bash_mode {
            Box::new(|s: &str| theme().bash_mode_border(s))
        } else {
            let level = thinking_border_level(self.session.thinking_level().as_str());
            Box::new(move |s: &str| theme().thinking_border(level, s))
        };
        self.dirty.set(true);
    }

    /// `updateEditorPromptPrefix`: `!` in the bash-mode colour, else `❯`.
    fn update_editor_prompt_prefix(&mut self) {
        let mut editor = self.editor.borrow_mut();
        if self.is_bash_mode {
            editor.editor.prompt_prefix = "!".into();
            editor.editor.prompt_color = Box::new(|s: &str| theme().bash_mode_border(s));
        } else {
            editor.editor.prompt_prefix = "❯".into();
            editor.editor.prompt_color = Box::new(|s: &str| s.to_string());
        }
        self.dirty.set(true);
    }

    /// Out of bash mode after the prompt was cleared. The pin's `setText("")`
    /// already ran `onChange` synchronously; here that change is still queued
    /// and would see no transition, so the prefix is reset too.
    fn leave_bash_mode(&mut self) {
        self.is_bash_mode = false;
        self.update_editor_border_color();
        self.update_editor_prompt_prefix();
    }

    /// The editor's `onChange`: entering or leaving bash mode.
    fn on_editor_change(&mut self, text: &str) {
        let was_bash_mode = self.is_bash_mode;
        self.is_bash_mode = text.trim_start().starts_with('!');
        if was_bash_mode != self.is_bash_mode {
            self.update_editor_border_color();
            self.update_editor_prompt_prefix();
        }
    }

    /// `BashExecutionController.handleBashCommand`: run a `!` command
    /// through the session off the UI thread, streaming into its row. While
    /// the agent streams the row waits in the pending area. The `user_bash`
    /// extension hook waits on the extension runner (12.3).
    fn handle_bash_command(&mut self, command: String, exclude_from_context: bool) {
        let component = handle(BashExecutionComponent::new(&command, exclude_from_context));
        if self.session.is_streaming() {
            self.pending_messages
                .borrow_mut()
                .add_child(as_component(&component));
            self.pending_bash_components.push(component.clone());
        } else {
            self.add_to_chat(as_component(&component));
        }
        self.bash_components.push(component.clone());
        self.bash_component = Some(component);
        self.dirty.set(true);

        let session = self.session.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let chunks = tx.clone();
            let mut on_chunk = move |chunk: &str| {
                let _ = chunks.send(AppEvent::BashChunk(chunk.to_string()));
            };
            let result = session
                .execute_bash(&command, Some(&mut on_chunk), exclude_from_context, None)
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::BashDone(result));
        });
    }

    /// The `!` command ended: settle its row.
    fn finish_bash_command(&mut self, result: Result<BashResult, String>) {
        let component = self.bash_component.take();
        match result {
            Ok(result) => {
                if let Some(component) = component {
                    component.borrow_mut().set_complete(
                        result.exit_code,
                        result.cancelled,
                        result.truncated.then(|| truncated_marker(&result.output)),
                        result.full_output_path,
                    );
                }
            }
            Err(error) => {
                if let Some(component) = component {
                    component.borrow_mut().set_complete(None, false, None, None);
                }
                self.show_error(&format!("Bash command failed: {error}"));
            }
        }
        self.dirty.set(true);
    }

    /// `flushPendingBashComponents`: move parked `!` rows into the chat.
    fn flush_pending_bash_components(&mut self) {
        for component in std::mem::take(&mut self.pending_bash_components) {
            let component = as_component(&component);
            self.pending_messages.borrow_mut().remove_child(&component);
            self.add_to_chat(component);
        }
    }

    fn update_session_chip(&mut self) {
        let chip = render_session_chip(
            &self.session.display_name(),
            self.session.session_color_slot() as i64,
        );
        let shown = chip.is_some();
        self.editor.borrow_mut().editor.top_border_label = chip;
        self.footer.borrow_mut().set_session_chip_shown(shown);
        self.dirty.set(true);
    }

    fn update_terminal_title(&mut self) {
        let cwd = self.session.cwd().to_path_buf();
        let base = cwd.file_name().map_or_else(
            || cwd.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let title = match self.session.session_name() {
            Some(name) => format!("{APP_TITLE} - {name} - {base}"),
            None => format!("{APP_TITLE} - {base}"),
        };
        self.tui.terminal.set_title(&title);
    }

    fn add_to_chat(&mut self, component: ComponentHandle) {
        self.chat.borrow_mut().add_child(component);
        self.dirty.set(true);
    }

    /// `showError`: a filled error block in the chat, headline over detail.
    fn show_error(&mut self, message: &str) {
        let mut lines = message.split('\n');
        let title = format!("Error: {}", lines.next().unwrap_or(""));
        let mut body: Vec<&str> = lines.collect();
        // A wait measured in days is a dead end: say what to do about it.
        if is_long_retry_delay_error(Some(message)) {
            body.push(
                "Switch to another model with /model, or wait for the provider's quota to reset.",
            );
        }
        self.show_block(BlockFill::ToolErrorBg, "error", &title, &body);
    }

    /// The startup/reload listing and the session's own state.
    fn render_resources(&mut self) {
        let mut listing = (self.listing)(&self.session);
        listing.columns = Some(self.size.get().0 as usize);
        listing.verbose = self.verbose;
        listing.expanded = self.expanded;
        for component in show_loaded_resources(&listing, false, true) {
            self.add_to_chat(component);
        }
    }

    fn subscribe(&mut self) -> cortexcode_code_agent_session::SessionSubscription {
        let tx = self.tx.clone();
        let session = self.session.clone();
        self.session.subscribe(move |event| {
            let anchor =
                matches!(event, AgentSessionEvent::Agent(AgentEvent::AgentStart)).then(|| {
                    let totals = sum_assistant_usage(session.session_manager().entries());
                    (totals, Instant::now())
                });
            let _ = tx.send(AppEvent::Session(Box::new(event.clone()), anchor));
        })
    }

    fn prompt(&mut self, text: String) {
        self.prompt_with_images(text, Vec::new());
    }

    fn prompt_with_images(&mut self, text: String, images: Vec<ImageContent>) {
        startup_progress::clear();
        let session = self.session.clone();
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = session
                .prompt(
                    &text,
                    PromptOptions {
                        expand_prompt_templates: true,
                        images,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::PromptDone(result));
        });
    }

    fn submit(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        // Built-in slash commands; `with_args` ones also take "/name <args>".
        let (name, has_args) = match text.find(' ') {
            Some(i) => (&text[..i], true),
            None => (text.as_str(), false),
        };
        if let Some(command) = BuiltinCommand::lookup(name) {
            if !has_args || command.with_args() {
                self.run_builtin_command(command, &text);
                return;
            }
        }
        // `!` runs bash; `!!` keeps it out of the model's context.
        if let Some(rest) = text.strip_prefix('!') {
            let exclude_from_context = rest.starts_with('!');
            let command = if exclude_from_context {
                &rest[1..]
            } else {
                rest
            }
            .trim();
            if !command.is_empty() {
                if self.session.is_bash_running() {
                    self.show_warning(
                        "A bash command is already running. Press Esc to cancel it first.",
                    );
                    self.editor.borrow_mut().editor.set_text(&text);
                    return;
                }
                self.editor.borrow_mut().editor.add_to_history(&text);
                self.handle_bash_command(command.to_string(), exclude_from_context);
                self.leave_bash_mode();
                return;
            }
        }
        // Typed during a compaction: held until it ends.
        if self.session.is_compacting() {
            self.queue_compaction_message(text, StreamingBehavior::Steer);
            return;
        }
        // Typed while the agent works: steers the current run.
        if self.session.is_streaming() {
            self.editor.borrow_mut().editor.add_to_history(&text);
            self.queue_prompt(text, StreamingBehavior::Steer);
            return;
        }
        self.flush_pending_bash_components();
        self.editor.borrow_mut().editor.add_to_history(&text);
        self.prompt(text);
    }

    /// `getMarkdownThemeWithSettings`.
    fn markdown_theme(&self) -> Rc<dyn Fn() -> MarkdownTheme> {
        let indent = self.code_block_indent.clone();
        Rc::new(move || MarkdownTheme {
            code_block_indent: Some(indent.clone()),
            ..get_markdown_theme()
        })
    }

    /// `thinkingDisplayForView`: radar drops traces outright.
    fn thinking_display(&self) -> ThinkingDisplay {
        if self.tool_output_view == ToolOutputView::Radar {
            ThinkingDisplay::Omit
        } else if self.hide_thinking_block {
            ThinkingDisplay::Label
        } else {
            ThinkingDisplay::Full
        }
    }

    fn create_working_loader(&self) -> Rc<RefCell<Loader>> {
        let mut loader = Loader::new(
            Box::new(|s: &str| theme().fg("accent", s)),
            Box::new(|s: &str| theme().fg("muted", s)),
            DEFAULT_WORKING_MESSAGE,
            None,
        );
        loader.start();
        handle(loader)
    }

    fn stop_working_loader(&mut self) {
        if let Some(loader) = self.loader.take() {
            loader.borrow_mut().stop();
        }
        self.status.borrow_mut().clear();
    }

    /// `addMessageToChat` for the roles this transcript draws so far.
    fn add_message_to_chat(&mut self, message: &AgentMessage, populate_history: bool) {
        match message {
            AgentMessage::User(user) => {
                let text: String = user
                    .content
                    .blocks()
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect();
                if text.is_empty() {
                    return;
                }
                if !self.chat.borrow().children.is_empty() {
                    self.add_to_chat(as_component(&handle(Spacer::new(1))));
                }
                let component = UserMessageComponent::with_theme(&text, (self.markdown_theme())());
                self.add_to_chat(as_component(&handle(component)));
                if populate_history {
                    self.editor.borrow_mut().editor.add_to_history(&text);
                }
            }
            AgentMessage::Assistant(assistant) => {
                let component = handle(AssistantMessageComponent::with_theme(
                    Some(assistant),
                    self.thinking_display(),
                    self.markdown_theme(),
                    DEFAULT_HIDDEN_THINKING_LABEL,
                ));
                self.assistant_components.push(component.clone());
                self.add_to_chat(as_component(&component));
            }
            AgentMessage::Custom(custom) => {
                if custom.display {
                    let mut component =
                        CustomMessageComponent::new(custom.clone(), self.markdown_theme());
                    component.set_expanded(self.expanded);
                    self.add_to_chat(as_component(&handle(component)));
                }
            }
            AgentMessage::CompactionSummary(summary) => {
                self.add_to_chat(as_component(&handle(Spacer::new(1))));
                let component = handle(CompactionSummaryMessageComponent::new(
                    summary.clone(),
                    self.markdown_theme(),
                ));
                component.borrow_mut().set_expanded(self.expanded);
                self.compaction_summaries.push(component.clone());
                self.add_to_chat(as_component(&component));
            }
            AgentMessage::BranchSummary(summary) => {
                self.add_to_chat(as_component(&handle(Spacer::new(1))));
                let component = handle(BranchSummaryMessageComponent::new(
                    summary.clone(),
                    self.markdown_theme(),
                ));
                component.borrow_mut().set_expanded(self.expanded);
                self.branch_summaries.push(component.clone());
                self.add_to_chat(as_component(&component));
            }
            AgentMessage::BashExecution(bash) => {
                let exclude = bash.exclude_from_context.unwrap_or(false);
                let mut component = BashExecutionComponent::new(&bash.command, exclude);
                if !bash.output.is_empty() {
                    component.append_output(&bash.output);
                }
                component.set_complete(
                    bash.exit_code.map(|c| c as i32),
                    bash.cancelled,
                    bash.truncated.then(|| truncated_marker("")),
                    bash.full_output_path.clone(),
                );
                let component = handle(component);
                self.bash_components.push(component.clone());
                self.add_to_chat(as_component(&component));
            }
            _ => {}
        }
    }

    /// `renderSessionContext`: the transcript of messages already in the
    /// session, with tool calls drawn as the live path draws them.
    fn render_session_context(&mut self, messages: &[AgentMessage], populate_history: bool) {
        self.pending_tools.clear();
        let mut rendered_pending: Vec<(String, Rc<RefCell<ToolExecutionComponent>>)> = Vec::new();
        let mut last_stop_reason = None;
        for message in messages {
            match message {
                AgentMessage::Assistant(assistant) => {
                    // The live path's chain boundary, so a rebuilt history shows
                    // the chains it lived through.
                    if self.opens_new_chain(assistant) {
                        self.close_open_chain(ChainState::Done);
                    }
                    last_stop_reason = Some(assistant.stop_reason);
                    self.add_message_to_chat(message, populate_history);
                    for content in &assistant.content {
                        let Content::ToolCall(call) = content else {
                            continue;
                        };
                        let block =
                            self.new_tool_block(&call.name, &call.id, call.arguments.clone());
                        self.attach_tool_block(block.clone());
                        if matches!(
                            assistant.stop_reason,
                            StopReason::Aborted | StopReason::Error
                        ) {
                            let error = if assistant.stop_reason == StopReason::Aborted {
                                let attempt = self.session.retry_attempt();
                                if attempt > 0 {
                                    format!(
                                        "Aborted after {attempt} retry attempt{}",
                                        if attempt > 1 { "s" } else { "" }
                                    )
                                } else {
                                    "Operation aborted".to_string()
                                }
                            } else {
                                assistant
                                    .error_message
                                    .clone()
                                    .filter(|m| !m.is_empty())
                                    .unwrap_or_else(|| "Error".into())
                            };
                            block.borrow_mut().update_result(
                                ToolResult {
                                    content: vec![Content::text(error)],
                                    details: serde_json::Value::Null,
                                    is_error: true,
                                },
                                false,
                            );
                        } else {
                            block.borrow_mut().set_args_complete();
                            rendered_pending.push((call.id.clone(), block));
                        }
                    }
                }
                AgentMessage::ToolResult(result) => {
                    if let Some(i) = rendered_pending
                        .iter()
                        .position(|(id, _)| *id == result.tool_call_id)
                    {
                        let (_, block) = rendered_pending.remove(i);
                        block.borrow_mut().update_result(
                            ToolResult {
                                content: result.content.clone(),
                                details: result.details.clone().unwrap_or(serde_json::Value::Null),
                                is_error: result.is_error,
                            },
                            false,
                        );
                    }
                }
                _ => self.add_message_to_chat(message, populate_history),
            }
        }
        // History has no live state: a chain still open is finished, unless
        // calls still wait for results (a resumed run in flight).
        if rendered_pending.is_empty() {
            self.close_open_chain(if last_stop_reason == Some(StopReason::Stop) {
                ChainState::Done
            } else {
                ChainState::Interrupted
            });
        }
        self.pending_tools.extend(rendered_pending);
        self.dirty.set(true);
    }

    /// `renderInitialMessages`: the loaded session's transcript, and how often
    /// it was compacted.
    fn render_initial_messages(&mut self) {
        let messages = self.session.messages();
        self.update_editor_border_color();
        self.render_session_context(&messages, true);
        let compactions = self
            .session
            .session_manager()
            .entries()
            .iter()
            .filter(|e| matches!(e, cortexcode_code_session::FileEntry::Compaction { .. }))
            .count();
        if compactions > 0 {
            let times = if compactions == 1 {
                "1 time".to_string()
            } else {
                format!("{compactions} times")
            };
            self.show_status(&format!("Session compacted {times}"));
        }
    }

    /// Re-render the in-flight message now, or once the throttle window passes.
    fn schedule_streaming_render(&mut self) {
        let due = self
            .stream_render_at
            .is_none_or(|at| at.elapsed() >= STREAM_RENDER_THROTTLE);
        if due {
            self.run_streaming_render();
        } else {
            self.stream_render_pending = true;
        }
    }

    fn run_streaming_render(&mut self) {
        self.stream_render_pending = false;
        self.stream_render_at = Some(Instant::now());
        if let (Some(component), Some(message)) = (&self.streaming, &self.streaming_message) {
            component.borrow_mut().update_content(message, true);
            self.dirty.set(true);
        }
    }

    fn new_tool_block(
        &self,
        name: &str,
        id: &str,
        args: serde_json::Value,
    ) -> Rc<RefCell<ToolExecutionComponent>> {
        // A registered tool always has a definition, renderers or not; only an
        // unknown name gets the bare text rendering.
        let definition = self
            .session
            .get_tool_definition(name)
            .map(|_| registered_tool_definition(name));
        handle(ToolExecutionComponent::new(
            name,
            id,
            args,
            ToolExecutionOptions {
                show_images: self.show_images,
                image_width_cells: self.image_width_cells,
                view: self.tool_output_view,
            },
            definition,
            &self.session.cwd().to_string_lossy(),
        ))
    }

    /// `attachToolBlock`: into the open chain (a new one when none is open),
    /// marking the newest call and run.
    fn attach_tool_block(&mut self, block: Rc<RefCell<ToolExecutionComponent>>) {
        let chain = match &self.open_chain {
            Some(chain) if chain.borrow().is_open() => chain.clone(),
            _ => {
                let chain = handle(ToolChainComponent::new(self.tool_output_view));
                self.add_to_chat(as_component(&chain));
                self.chains.push(chain.clone());
                self.open_chain = Some(chain.clone());
                chain
            }
        };
        chain.borrow_mut().add(block.clone());
        if let Some(previous) = self.latest_block.replace(block.clone()) {
            previous.borrow_mut().set_latest(false);
        }
        block.borrow_mut().set_latest(true);
        let same = self
            .latest_chain
            .as_ref()
            .is_some_and(|c| Rc::ptr_eq(c, &chain));
        if !same {
            if let Some(previous) = self.latest_chain.replace(chain.clone()) {
                previous.borrow_mut().set_latest(false);
            }
            chain.borrow_mut().set_latest(true);
        }
        self.dirty.set(true);
    }

    /// `closeOpenChain`: settle the chain collecting calls.
    fn close_open_chain(&mut self, outcome: ChainState) {
        let Some(chain) = self.open_chain.take() else {
            return;
        };
        if chain.borrow().is_empty() {
            let h = as_component(&chain);
            self.chat.borrow_mut().remove_child(&h);
            self.chains.retain(|c| !Rc::ptr_eq(c, &chain));
        } else {
            chain.borrow_mut().close(outcome);
        }
        self.dirty.set(true);
    }

    /// `opensNewChain`: speaking ends the run, and so does a drawn trace.
    fn opens_new_chain(&self, message: &AssistantMessage) -> bool {
        let draws_thinking = self.thinking_display() != ThinkingDisplay::Omit;
        message.content.iter().any(|c| match c {
            Content::Text(t) => !t.text.trim().is_empty(),
            Content::Thinking(t) => draws_thinking && !t.thinking.trim().is_empty(),
            _ => false,
        })
    }

    /// `trimTranscriptMemory`: freeze all but the newest live tool blocks.
    fn trim_transcript_memory(&mut self) {
        let freezable: Vec<_> = self
            .chains
            .iter()
            .flat_map(|c| c.borrow().tool_blocks().to_vec())
            .filter(|b| b.borrow().is_freezable())
            .collect();
        let excess = freezable.len().saturating_sub(LIVE_TOOL_WINDOW);
        for block in &freezable[..excess] {
            block.borrow_mut().freeze();
        }
    }

    /// `showSelector`: swap the selector into the editor's slot.
    fn show_selector(
        &mut self,
        title: &str,
        options: Vec<String>,
        reply: mpsc::Sender<Option<String>>,
    ) {
        if let Some((previous, previous_reply)) = self.selector.take() {
            previous.borrow_mut().dispose();
            let _ = previous_reply.send(None);
        }
        let sink = self.actions.clone();
        let selector = handle(ExtensionSelectorComponent::new(
            title,
            options,
            None,
            Box::new(move |outcome| sink.borrow_mut().push(Action::SelectorDone(outcome))),
        ));
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&selector));
        }
        self.tui.set_focus(Some(as_component(&selector)));
        self.selector = Some((selector, reply));
        self.dirty.set(true);
    }

    /// `hideSelector` + `restoreEditor`, answering the asker.
    fn close_selector(&mut self, outcome: SelectorOutcome) {
        let Some((selector, reply)) = self.selector.take() else {
            return;
        };
        selector.borrow_mut().dispose();
        let _ = reply.send(match outcome {
            SelectorOutcome::Selected(option) => Some(option),
            SelectorOutcome::Cancelled => None,
        });
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&self.editor));
        }
        self.tui.set_focus(Some(as_component(&self.editor)));
        self.dirty.set(true);
    }

    /// `showAskOptions`: the options pane in the editor's slot.
    fn show_ask_options(
        &mut self,
        questions: Vec<AskQuestion>,
        reply: mpsc::Sender<Option<Vec<String>>>,
    ) {
        if questions.is_empty() {
            let _ = reply.send(None);
            return;
        }
        // A pane still up belongs to an asker that is gone: it reads as skipped.
        self.hide_ask_options(None);
        let on_submit = self.actions.clone();
        let on_cancel = self.actions.clone();
        let pane = handle(AskOptionsComponent::new(
            questions,
            Box::new(move |answers| {
                on_submit
                    .borrow_mut()
                    .push(Action::AskOptionsDone(Some(answers)))
            }),
            Box::new(move || on_cancel.borrow_mut().push(Action::AskOptionsDone(None))),
            AskOptionsOptions::default(),
        ));
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&pane));
        }
        self.tui.set_focus(Some(as_component(&pane)));
        self.ask_options = Some((pane, reply));
        self.dirty.set(true);
    }

    /// `hideAskOptions`: answer the asker and put the prompt back.
    fn hide_ask_options(&mut self, answers: Option<Vec<String>>) {
        let Some((_, reply)) = self.ask_options.take() else {
            return;
        };
        let _ = reply.send(answers);
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&self.editor));
        }
        self.tui.set_focus(Some(as_component(&self.editor)));
        self.dirty.set(true);
    }

    /// The editor's slot back to the prompt, focused.
    fn restore_editor(&mut self) {
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&self.editor));
        }
        self.tui.set_focus(Some(as_component(&self.editor)));
        self.dirty.set(true);
    }

    /// `showEditor`: the multi-line editor dialog in the editor's slot.
    fn show_editor_dialog(&mut self, title: &str, reply: mpsc::Sender<Option<String>>) {
        if let Some((_, previous)) = self.editor_dialog.take() {
            let _ = previous.send(None);
        }
        let rows = self.size.clone();
        let flag = self.dirty.clone();
        let sink = self.actions.clone();
        let dialog = handle(ExtensionEditorComponent::new(
            EditorHost {
                rows: Box::new(move || rows.get().1),
                request_render: Box::new(move || flag.set(true)),
            },
            title,
            None,
            Box::new(move |outcome| sink.borrow_mut().push(Action::EditorDialogDone(outcome))),
        ));
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&dialog));
        }
        self.tui.set_focus(Some(as_component(&dialog)));
        self.editor_dialog = Some((dialog, reply));
        self.dirty.set(true);
    }

    /// `hideEditor`, answering the asker.
    fn close_editor_dialog(&mut self, outcome: EditorOutcome) {
        let Some((_, reply)) = self.editor_dialog.take() else {
            return;
        };
        let _ = reply.send(match outcome {
            EditorOutcome::Submitted(text) => Some(text),
            EditorOutcome::Cancelled => None,
        });
        self.restore_editor();
    }

    /// Escape twice within 500ms on an empty prompt: the tree, the fork
    /// picker or nothing, as `doubleEscapeAction` says.
    fn handle_double_escape(&mut self) {
        let action = self.session.settings().double_escape_action();
        if action == DoubleEscapeAction::None {
            return;
        }
        let now = Instant::now();
        if self
            .last_escape
            .is_some_and(|at| now.duration_since(at) < Duration::from_millis(500))
        {
            self.last_escape = None;
            // `fork` opens the fork picker, which is wired with /fork (11.4).
            if action == DoubleEscapeAction::Tree {
                self.show_tree_selector(None);
            }
        } else {
            self.last_escape = Some(now);
        }
    }

    /// `showTreeSelector`: the session tree in the editor's slot.
    fn show_tree_selector(&mut self, initial_selected_id: Option<String>) {
        let (tree, leaf) = {
            let manager = self.session.session_manager();
            (manager.tree(), manager.leaf_id().map(str::to_string))
        };
        let filter = self.session.settings().tree_filter_mode();
        if tree.is_empty() {
            self.show_status("No entries in session");
            return;
        }
        let selector = handle(TreeSelectorComponent::new(
            &tree,
            leaf.as_deref(),
            self.size.get().1 as usize,
            initial_selected_id.as_deref(),
            Some(filter),
        ));
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&selector));
        }
        self.tui.set_focus(Some(as_component(&selector)));
        self.tree_selector = Some((selector, leaf));
        self.dirty.set(true);
    }

    fn poll_tree_selector(&mut self) {
        let Some((selector, leaf)) = &self.tree_selector else {
            return;
        };
        let events = selector.borrow_mut().poll(Instant::now());
        let leaf = leaf.clone();
        for event in events {
            match event {
                TreeEvent::LabelChange(id, label) => {
                    let _ = self
                        .session
                        .session_manager()
                        .append_label_change(id, label);
                    self.dirty.set(true);
                }
                TreeEvent::Cancel => {
                    self.tree_selector = None;
                    self.restore_editor();
                    return;
                }
                TreeEvent::Select(id) => {
                    self.tree_selector = None;
                    self.restore_editor();
                    if leaf.as_deref() == Some(id.as_str()) {
                        // Selecting the current leaf is a no-op.
                        self.show_status("Already at this point");
                    } else if self.session.settings().branch_summary_skip_prompt() {
                        self.navigate_tree(id, false, None);
                    } else {
                        self.ask_tree_summary(id);
                    }
                    return;
                }
            }
        }
    }

    /// `showNotice`: a filled warning block in the chat, for warnings that
    /// cost money if ignored (`showBlock`).
    fn show_notice(&mut self, title: &str, body: &[&str]) {
        self.show_block(BlockFill::WarningBg, "warning", title, body);
    }

    /// `showBlock`: a filled block in the chat, bold title over muted lines.
    fn show_block(&mut self, fill: BlockFill, fg: &str, title: &str, body: &[&str]) {
        let t = theme();
        let mut block = BoxComponent::new(1, 1, None);
        apply_block_fill(&mut block, fill);
        block.add_child(as_component(&handle(Text::new(
            t.bold(&t.fg(fg, title)),
            0,
            0,
        ))));
        for line in body {
            block.add_child(as_component(&handle(Text::new(t.fg("muted", line), 0, 0))));
        }
        self.add_to_chat(as_component(&handle(Spacer::new(1))));
        self.add_to_chat(as_component(&handle(block)));
    }

    /// `getModelCandidates`: the model scope when set, else every model with
    /// configured auth.
    fn model_candidates(&self) -> Vec<Model> {
        let scoped = self.session.scoped_models();
        if scoped.is_empty() {
            self.session.get_available_models()
        } else {
            scoped.into_iter().map(|s| s.model).collect()
        }
    }

    /// `updateAvailableProviderCount`: the footer names the provider once
    /// there is more than one.
    fn update_available_provider_count(&mut self) {
        let providers: HashSet<String> = self
            .model_candidates()
            .into_iter()
            .map(|m| m.provider)
            .collect();
        self.footer_data
            .set_available_provider_count(providers.len());
        self.dirty.set(true);
    }

    /// `maybeWarnAboutAnthropicSubscriptionAuth`: once per session.
    fn maybe_warn_about_anthropic_subscription_auth(&mut self, model: Option<Model>) {
        if self.anthropic_warning_shown {
            return;
        }
        let Some(model) = model.or_else(|| self.session.model()) else {
            return;
        };
        if !self.session.uses_anthropic_subscription_auth(&model) {
            return;
        }
        self.anthropic_warning_shown = true;
        self.show_notice(
            "Anthropic subscription",
            &[
                "Billed per token as extra usage, not against plan limits.",
                "Turn off in /settings → Anthropic extra usage.",
            ],
        );
    }

    /// `cycleModel`.
    fn cycle_model(&mut self, forward: bool) {
        let direction = if forward {
            cortexcode_code_agent_session::CycleDirection::Forward
        } else {
            cortexcode_code_agent_session::CycleDirection::Backward
        };
        match self.session.cycle_model(direction) {
            None => {
                let message = if self.session.scoped_models().is_empty() {
                    "Only one model available"
                } else {
                    "Only one model in scope"
                };
                self.show_status(message);
            }
            Some(result) => {
                self.footer.borrow_mut().invalidate();
                self.update_editor_border_color();
                let thinking =
                    if result.model.reasoning && result.thinking_level != ThinkingLevel::Off {
                        format!(" (thinking: {})", result.thinking_level.as_str())
                    } else {
                        String::new()
                    };
                let name = if result.model.name.is_empty() {
                    &result.model.id
                } else {
                    &result.model.name
                };
                self.show_dial_step(
                    if forward {
                        "app.model.cycleBackward"
                    } else {
                        "app.model.cycleForward"
                    },
                    &format!("Switched to {name}{thinking}"),
                );
                self.maybe_warn_about_anthropic_subscription_auth(Some(result.model));
            }
        }
    }

    /// `handleModel`: `/model` opens the picker; `/model <ref>` switches on
    /// an exact match, else opens the picker searching for it.
    fn handle_model_command(&mut self, search: Option<String>) {
        let Some(search) = search else {
            self.show_model_selector(None);
            return;
        };
        let candidates = self.model_candidates();
        let Some(model) = find_exact_model_reference_match(&search, &candidates).cloned() else {
            self.show_model_selector(Some(&search));
            return;
        };
        self.switch_model(model);
    }

    /// `session.setModel` and what the chrome shows about it.
    fn switch_model(&mut self, model: Model) {
        match self.session.set_model(model.clone()) {
            Ok(()) => {
                self.footer.borrow_mut().invalidate();
                self.update_editor_border_color();
                self.show_status(&format!("Model: {}", model.id));
                self.maybe_warn_about_anthropic_subscription_auth(Some(model));
            }
            Err(error) => self.show_error(&error.to_string()),
        }
    }

    /// `showModelSelector`.
    fn show_model_selector(&mut self, initial_search: Option<&str>) {
        let load_error = self.session.model_registry().error().map(String::from);
        let selector = handle(ModelSelectorComponent::new(
            self.session.model(),
            Ok(self.session.get_available_models()),
            load_error,
            self.session
                .scoped_models()
                .into_iter()
                .map(|s| s.model)
                .collect(),
            initial_search,
        ));
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&selector));
        }
        self.tui.set_focus(Some(as_component(&selector)));
        self.model_selector = Some(selector);
        self.dirty.set(true);
    }

    /// `showModelsSelector`: the enable set model cycling steps through.
    fn show_models_selector(&mut self) {
        let all = self.session.get_available_models();
        if all.is_empty() {
            self.show_status("No models available");
            return;
        }
        let full_id = |m: &Model| format!("{}/{}", m.provider, m.id);
        let scoped = self.session.scoped_models();
        let enabled = if !scoped.is_empty() {
            Some(scoped.iter().map(|s| full_id(&s.model)).collect())
        } else {
            let patterns = self.session.settings().enabled_models();
            patterns.filter(|p| !p.is_empty()).map(|patterns| {
                resolve_model_scope(&patterns, &all)
                    .models
                    .iter()
                    .map(|s| full_id(&s.model))
                    .collect()
            })
        };
        let total = all.len();
        let selector = handle(ScopedModelsSelectorComponent::new(all, enabled));
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&selector));
        }
        self.tui.set_focus(Some(as_component(&selector)));
        self.scoped_models_selector = Some((selector, total));
        self.dirty.set(true);
    }

    /// The model pickers' answers.
    fn poll_model_selectors(&mut self) {
        let event = self
            .model_selector
            .as_ref()
            .and_then(|s| s.borrow_mut().take_events().into_iter().next());
        if let Some(event) = event {
            self.model_selector = None;
            self.restore_editor();
            if let ModelSelectorEvent::Select(model) = event {
                self.switch_model(*model);
            }
        }
        if let Some((selector, total)) = &self.scoped_models_selector {
            let total = *total;
            let events = selector.borrow_mut().take_events();
            for event in events {
                match event {
                    ScopedModelsEvent::Change(enabled) => {
                        self.set_session_model_scope(enabled, total)
                    }
                    ScopedModelsEvent::Persist(enabled) => {
                        // Every model enabled clears the filter.
                        let patterns = enabled.filter(|ids| ids.len() != total);
                        self.session
                            .settings()
                            .set_enabled_models(patterns.as_deref());
                        self.show_status("Model selection saved to settings");
                    }
                    ScopedModelsEvent::Cancel => {
                        self.scoped_models_selector = None;
                        self.restore_editor();
                        return;
                    }
                }
            }
        }
    }

    /// The session's model scope from the picker (session-only): all or
    /// none enabled means no filter.
    fn set_session_model_scope(&mut self, enabled: Option<Vec<String>>, total: usize) {
        match enabled.filter(|ids| !ids.is_empty() && ids.len() < total) {
            Some(ids) => {
                let available = self.session.get_available_models();
                let scope = resolve_model_scope(&ids, &available);
                self.session.set_scoped_models(scope.models);
            }
            None => self.session.set_scoped_models(Vec::new()),
        }
        self.update_available_provider_count();
    }

    /// Put `component` in the editor's slot, focused.
    fn show_in_editor_slot(&mut self, component: ComponentHandle) {
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(component.clone());
        }
        self.tui.set_focus(Some(component));
        self.dirty.set(true);
    }

    /// `showOAuthSelector`: `/login` asks how to sign in; `/logout` lists
    /// the stored credentials.
    fn show_oauth_selector(&mut self, mode: LoginMode) {
        if mode == LoginMode::Login {
            self.show_login_auth_type_selector();
            return;
        }
        let options = logout_provider_options(&self.auth_storage);
        if options.is_empty() {
            self.show_status(NOTHING_TO_LOG_OUT);
            return;
        }
        let selector = handle(OAuthSelectorComponent::new(
            mode,
            self.auth_storage.clone(),
            options.clone(),
            None,
        ));
        self.show_in_editor_slot(as_component(&selector));
        self.login = Some(LoginStep::Provider {
            selector,
            mode,
            options,
        });
    }

    /// `showLoginAuthTypeSelector`.
    fn show_login_auth_type_selector(&mut self) {
        let outcomes: Outcomes = Rc::default();
        let sink = outcomes.clone();
        let selector = handle(ExtensionSelectorComponent::new(
            // Names the pane in its top border.
            "authentication method",
            vec![SUBSCRIPTION_LABEL.to_string(), API_KEY_LABEL.to_string()],
            None,
            Box::new(move |outcome| sink.borrow_mut().push(outcome)),
        ));
        self.show_in_editor_slot(as_component(&selector));
        self.login = Some(LoginStep::AuthType(selector, outcomes));
    }

    /// `showLoginProviderSelector`.
    fn show_login_provider_selector(&mut self, auth_type: AuthType) {
        let registry = self.session.model_registry().clone();
        let options = login_provider_options(&self.auth_storage, &registry, Some(auth_type));
        if options.is_empty() {
            self.show_status(no_providers_message(auth_type));
            return;
        }
        let auth = self.auth_storage.clone();
        let selector = handle(OAuthSelectorComponent::new(
            LoginMode::Login,
            self.auth_storage.clone(),
            options.clone(),
            Some(Box::new(move |id: &str| {
                provider_auth_status(&auth, &registry, id)
            })),
        ));
        self.show_in_editor_slot(as_component(&selector));
        self.login = Some(LoginStep::Provider {
            selector,
            mode: LoginMode::Login,
            options,
        });
    }

    /// The login panes' answers.
    fn poll_login(&mut self) {
        let Some(step) = self.login.take() else {
            return;
        };
        match step {
            LoginStep::AuthType(selector, outcomes) => {
                let outcome = outcomes.borrow_mut().drain(..).next();
                match outcome {
                    None => self.login = Some(LoginStep::AuthType(selector, outcomes)),
                    Some(outcome) => {
                        self.restore_editor();
                        if let SelectorOutcome::Selected(option) = outcome {
                            self.show_login_provider_selector(if option == SUBSCRIPTION_LABEL {
                                AuthType::OAuth
                            } else {
                                AuthType::ApiKey
                            });
                        }
                    }
                }
            }
            LoginStep::Provider {
                selector,
                mode,
                options,
            } => {
                let event = selector.borrow_mut().take_events().into_iter().next();
                match event {
                    None => {
                        self.login = Some(LoginStep::Provider {
                            selector,
                            mode,
                            options,
                        })
                    }
                    Some(OAuthSelectorEvent::Cancel) => {
                        self.restore_editor();
                        if mode == LoginMode::Login {
                            self.show_login_auth_type_selector();
                        }
                    }
                    Some(OAuthSelectorEvent::Select(id)) => {
                        self.restore_editor();
                        let Some(provider) = options.into_iter().find(|p| p.id == id) else {
                            return;
                        };
                        match (mode, provider.auth_type) {
                            (LoginMode::Logout, _) => self.logout(&provider),
                            (LoginMode::Login, AuthType::OAuth) => self.show_login_dialog(provider),
                            (LoginMode::Login, AuthType::ApiKey) => {
                                self.show_api_key_login_dialog(provider)
                            }
                        }
                    }
                }
            }
            LoginStep::ApiKey {
                dialog,
                provider,
                had_model,
            } => {
                let event = dialog.borrow_mut().take_events().into_iter().next();
                match event {
                    None => {
                        self.login = Some(LoginStep::ApiKey {
                            dialog,
                            provider,
                            had_model,
                        })
                    }
                    Some(LoginDialogEvent::Cancelled) => self.restore_editor(),
                    Some(LoginDialogEvent::Submitted(value)) => {
                        self.restore_editor();
                        let key = value.trim();
                        if key.is_empty() {
                            self.show_error(&format!(
                                "Failed to save API key for {}: API key cannot be empty.",
                                provider.name
                            ));
                            return;
                        }
                        self.auth_storage.set(
                            &provider.id,
                            AuthCredential::ApiKey {
                                key: key.to_string(),
                            },
                        );
                        self.complete_provider_authentication(&provider, had_model);
                    }
                }
            }
            LoginStep::OAuth(mut login) => {
                if let Some(select) = &login.select {
                    let outcome = select.outcomes.borrow_mut().drain(..).next();
                    if let Some(outcome) = outcome {
                        let select = login.select.take().expect("checked above");
                        let id = match outcome {
                            SelectorOutcome::Selected(label) => select
                                .options
                                .iter()
                                .find(|o| o.label == label)
                                .map(|o| o.id.clone()),
                            SelectorOutcome::Cancelled => None,
                        };
                        let _ = select.reply.send(id);
                        self.show_in_editor_slot(as_component(&login.dialog));
                    }
                    self.login = Some(LoginStep::OAuth(login));
                    return;
                }
                let events = login.dialog.borrow_mut().take_events();
                for event in events {
                    match event {
                        LoginDialogEvent::Cancelled => {
                            // Dropping the owed answers fails the flow as
                            // cancelled; its end is not waited for.
                            self.restore_editor();
                            return;
                        }
                        LoginDialogEvent::Submitted(value) => {
                            if let Some(reply) = login.prompt.take() {
                                let _ = reply.send(value);
                            } else if !value.is_empty() {
                                if let Some(reply) = login.manual.take() {
                                    let _ = reply.send(value);
                                }
                            }
                        }
                    }
                }
                self.login = Some(LoginStep::OAuth(login));
            }
        }
        self.dirty.set(true);
    }

    /// `/logout`'s pick: forget the credential.
    fn logout(&mut self, provider: &AuthSelectorProvider) {
        self.auth_storage.logout(&provider.id);
        if let Some(error) = self.auth_storage.drain_errors().into_iter().next() {
            self.show_error(&format!("Logout failed: {error}"));
            return;
        }
        self.update_available_provider_count();
        self.show_status(&logged_out_message(provider));
    }

    /// `showApiKeyLoginDialog`.
    fn show_api_key_login_dialog(&mut self, provider: AuthSelectorProvider) {
        let mut dialog = LoginDialogComponent::new(&provider.name, None);
        dialog.show_prompt("Enter API key:", None);
        let dialog = handle(dialog);
        self.show_in_editor_slot(as_component(&dialog));
        self.login = Some(LoginStep::ApiKey {
            dialog,
            provider,
            had_model: self.session.model().is_some(),
        });
    }

    /// `showLoginDialog`: the provider's OAuth flow, on the runtime.
    fn show_login_dialog(&mut self, provider: AuthSelectorProvider) {
        let uses_callback_server = self
            .auth_storage
            .get_oauth_providers()
            .iter()
            .find(|p| p.id() == provider.id)
            .is_some_and(|p| p.uses_callback_server());
        let dialog = handle(LoginDialogComponent::new(&provider.name, None));
        self.show_in_editor_slot(as_component(&dialog));
        let tx = self.tx.clone();
        let send = Mutex::new(tx.clone());
        let bridge = OAuthBridge::new(
            Box::new(move |update| {
                let _ = send
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(AppEvent::Login(update));
            }),
            dialog.borrow().signal(),
            uses_callback_server,
        );
        let auth = self.auth_storage.clone();
        let id = provider.id.clone();
        self.runtime.spawn(async move {
            let result = run_oauth_login(auth, id, bridge).await;
            let _ = tx.send(AppEvent::Login(LoginUpdate::Done(result)));
        });
        self.login = Some(LoginStep::OAuth(Box::new(OAuthLogin {
            dialog,
            provider,
            had_model: self.session.model().is_some(),
            prompt: None,
            manual: None,
            select: None,
        })));
    }

    /// A step of the running OAuth login. Steps of a login the user
    /// cancelled are dropped, which fails what they wait on.
    fn handle_login_update(&mut self, update: LoginUpdate) {
        let Some(LoginStep::OAuth(login)) = &mut self.login else {
            return;
        };
        self.dirty.set(true);
        match update {
            LoginUpdate::Auth { info, manual } => {
                let mut dialog = login.dialog.borrow_mut();
                dialog.show_auth(&info.url, info.instructions.as_deref());
                open_url(&info.url);
                if let Some(manual) = manual {
                    dialog.show_manual_input(
                        "Paste redirect URL below, or complete login in browser:",
                    );
                    login.manual = Some(manual);
                } else if login.provider.id == "github-copilot" {
                    // Copilot polls after onAuth.
                    dialog.show_waiting("Waiting for browser authentication...");
                }
            }
            LoginUpdate::Prompt { prompt, reply } => {
                login
                    .dialog
                    .borrow_mut()
                    .show_prompt(&prompt.message, prompt.placeholder.as_deref());
                login.prompt = Some(reply);
            }
            LoginUpdate::Progress(message) => login.dialog.borrow_mut().show_progress(&message),
            LoginUpdate::Select { prompt, reply } => {
                let outcomes: Outcomes = Rc::default();
                let sink = outcomes.clone();
                let selector = handle(ExtensionSelectorComponent::new(
                    &prompt.message,
                    prompt.options.iter().map(|o| o.label.clone()).collect(),
                    None,
                    Box::new(move |outcome| sink.borrow_mut().push(outcome)),
                ));
                login.select = Some(OAuthSelect {
                    outcomes,
                    options: prompt.options,
                    reply,
                });
                self.show_in_editor_slot(as_component(&selector));
            }
            LoginUpdate::Done(result) => {
                let Some(LoginStep::OAuth(login)) = self.login.take() else {
                    return;
                };
                self.restore_editor();
                match result {
                    Ok(()) => {
                        self.complete_provider_authentication(&login.provider, login.had_model)
                    }
                    Err(error) if error == LOGIN_CANCELLED => {}
                    Err(error) => self.show_error(&format!(
                        "Failed to login to {}: {error}",
                        login.provider.name
                    )),
                }
            }
        }
    }

    /// `completeProviderAuthentication`: with no model yet, select the
    /// provider's default; record where the credential went.
    fn complete_provider_authentication(
        &mut self,
        provider: &AuthSelectorProvider,
        had_model: bool,
    ) {
        let label = action_label(&provider.name, provider.auth_type);
        let available = self.session.get_available_models();
        let (selected, error) = match post_login_model(had_model, &provider.id, &label, &available)
        {
            PostLoginModel::Keep => (None, None),
            PostLoginModel::Error(error) => (None, Some(error)),
            PostLoginModel::Select(model) => match self.session.set_model((*model).clone()) {
                Ok(()) => (Some(*model), None),
                Err(error) => (
                    None,
                    Some(format!(
                        "{label}, but selecting its default model failed: {error}. Use /model to select a model."
                    )),
                ),
            },
        };
        self.update_available_provider_count();
        self.footer.borrow_mut().invalidate();
        self.update_editor_border_color();
        let path = cortexcode_code_paths::auth_path();
        match selected {
            Some(model) => {
                self.show_record(&format!(
                    "{label}. Selected {}. Credentials saved to {}",
                    model.id,
                    path.display()
                ));
                self.maybe_warn_about_anthropic_subscription_auth(Some(model));
            }
            None => {
                self.show_record(&format!("{label}. Credentials saved to {}", path.display()));
                match error {
                    Some(error) => self.show_error(&error),
                    None => self.maybe_warn_about_anthropic_subscription_auth(None),
                }
            }
        }
    }

    /// `showSettingsSelector`: the `/settings` pane in the editor's slot,
    /// built from the live session.
    fn show_settings_selector(&mut self) {
        let session = self.session.clone();
        let config = {
            let settings = self.session.settings();
            let disabled: HashSet<String> = settings.disabled_tools().into_iter().collect();
            let all_tools = self.session.get_all_tools();
            // Union so tools disabled at startup (absent from the live
            // registry) still appear and can be re-enabled for next session.
            let mut names: Vec<String> = all_tools
                .iter()
                .filter(|t| t.source == ToolSource::Builtin)
                .map(|t| t.name.clone())
                .chain(disabled.iter().cloned())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            names.sort();
            let tokens: HashMap<&str, usize> = all_tools
                .iter()
                .map(|t| {
                    (
                        t.name.as_str(),
                        measure_tool_schema_tokens(&t.name, &t.description, &t.parameters),
                    )
                })
                .collect();
            let tools = names
                .into_iter()
                .map(|name| ToolToggleInfo {
                    enabled: !disabled.contains(&name),
                    tokens: tokens.get(name.as_str()).copied(),
                    name,
                })
                .collect();
            let learn = settings.learn_settings();
            SettingsConfig {
                auto_compact: settings.compaction_enabled(),
                tools,
                tool_groups: vec![
                    ToolGroupInfo {
                        id: "web".into(),
                        label: "Web tools".into(),
                        description: "webfetch + websearch (network access).".into(),
                        enabled: settings.enable_web_tools(),
                    },
                    ToolGroupInfo {
                        id: "embsearch".into(),
                        label: "Semantic search".into(),
                        description: "Semantic index layer fused into the always-on search tool."
                            .into(),
                        enabled: settings.enable_semantic_index(),
                    },
                ],
                // Resolved fresh on every open, never downloaded.
                external_tools: describe_external_tools(),
                // Extension flags arrive with the extension runner (12.3).
                flags: Vec::new(),
                tool_output_view: self.tool_output_view,
                tool_output_max_bytes: settings.tool_output_max_bytes(),
                tool_output_max_lines: settings.tool_output_max_lines(),
                context_gc: settings.context_gc_enabled(),
                show_images: settings.show_images(),
                image_width_cells: settings.image_width_cells(),
                auto_resize_images: settings.image_auto_resize(),
                block_images: settings.block_images(),
                enable_skill_commands: settings.enable_skill_commands(),
                light: settings.light(),
                plugin_install_scope: settings.plugin_install_scope(),
                enable_plugin_tools: settings.enable_plugin_tools(),
                project_pinned_settings: settings.project_settings().keys().cloned().collect(),
                platform: get_workspace_platforms().unwrap_or_default(),
                steering_mode: self.session.steering_mode(),
                follow_up_mode: self.session.follow_up_mode(),
                transport: transport_name(settings.transport()),
                thinking_level: self.session.thinking_level().as_str().to_string(),
                available_thinking_levels: self
                    .session
                    .get_available_thinking_levels()
                    .iter()
                    .map(|l| l.as_str().to_string())
                    .collect(),
                current_theme: settings.theme().unwrap_or_else(|| "dark".into()),
                available_themes: get_available_themes(),
                hide_thinking_block: self.hide_thinking_block,
                collapse_changelog: settings.collapse_changelog(),
                enable_install_telemetry: settings.enable_install_telemetry(),
                double_escape_action: settings.double_escape_action(),
                tree_filter_mode: settings.tree_filter_mode(),
                show_hardware_cursor: settings.show_hardware_cursor(),
                editor_border: settings.editor_border(),
                editor_padding_x: settings.editor_padding_x(),
                autocomplete_max_visible: settings.autocomplete_max_visible(),
                quiet_startup: settings.quiet_startup(),
                tips_enabled: settings.tips_enabled(),
                clear_on_shrink: settings.clear_on_shrink(),
                show_terminal_progress: settings.show_terminal_progress(),
                warnings: settings.warnings(),
                voice_silence_ms: resolve_voice_silence_ms(settings.voice_silence_ms()),
                webtools_timeout_secs: settings.webtools_timeout_secs(),
                learn,
                // Re-measured on every change so the pane can price a toggle.
                measure_token_surface: Some(Rc::new(move || {
                    let tools = session.agent().with_state(|s| s.tools.clone());
                    measure_prompt_surface(&session.system_prompt(), &tools.0)
                })),
                supports_images: get_capabilities().images.is_some(),
            }
        };
        self.settings_changes.borrow_mut().clear();
        let sink = self.settings_changes.clone();
        let selector = handle(SettingsSelectorComponent::new(config, move |change| {
            sink.borrow_mut().push(change)
        }));
        let list = selector.borrow().settings_list();
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&selector));
        }
        self.tui.set_focus(Some(list));
        self.settings_selector = Some(selector);
        self.dirty.set(true);
    }

    /// Apply what the settings pane asked for.
    fn poll_settings_selector(&mut self) {
        if self.settings_selector.is_none() {
            return;
        }
        let changes = std::mem::take(&mut *self.settings_changes.borrow_mut());
        if changes.is_empty() {
            return;
        }
        for change in changes {
            self.apply_settings_change(change);
        }
        // The pane priced the change before it was applied.
        if let Some(selector) = &self.settings_selector {
            selector.borrow().refresh_token_surface();
        }
    }

    /// `SettingsCallbacks`, one change at a time.
    fn apply_settings_change(&mut self, change: SettingsChange) {
        self.dirty.set(true);
        match change {
            SettingsChange::Cancel => {
                self.settings_selector = None;
                self.restore_editor();
            }
            SettingsChange::AutoCompact(enabled) => {
                self.session.set_auto_compaction_enabled(enabled);
                self.footer.borrow_mut().set_auto_compact_enabled(enabled);
            }
            SettingsChange::ToolEnabled { name, enabled } => {
                // Persisted for future sessions (the startup denylist).
                let mut disabled: Vec<String> = self.session.settings().disabled_tools();
                disabled.retain(|n| *n != name);
                if !enabled {
                    disabled.push(name.clone());
                }
                self.session.settings().set_disabled_tools(&disabled);
                // Live for this session; a tool removed before launch comes
                // back next session.
                let mut active = self.session.get_active_tool_names();
                active.retain(|n| *n != name);
                if enabled && self.session.get_tool_definition(&name).is_some() {
                    active.push(name);
                }
                self.session.set_active_tools_by_name(&active);
                self.footer_data.set_subagent_enabled(
                    self.session
                        .get_active_tool_names()
                        .iter()
                        .any(|t| t == "Task"),
                );
            }
            SettingsChange::ToolGroup { id, enabled } => {
                // These gate tool creation: next session.
                let mut settings = self.session.settings();
                match id.as_str() {
                    "web" => settings.set_enable_web_tools(enabled),
                    "embsearch" => settings.set_enable_semantic_index(enabled),
                    _ => {}
                }
            }
            SettingsChange::ToolOutputView(view) => self.apply_tool_output_view(view, true),
            SettingsChange::ToolOutputMaxBytes(bytes) => self
                .session
                .settings()
                .set_tool_output_max_bytes(bytes as i64),
            SettingsChange::ToolOutputMaxLines(lines) => self
                .session
                .settings()
                .set_tool_output_max_lines(lines as i64),
            SettingsChange::ContextGc(enabled) => {
                self.session.settings().set_context_gc_enabled(enabled)
            }
            SettingsChange::Flag { name, value } => {
                // Applied live with the extension runner (12.3).
                self.session.settings().set_flag_override(&name, value);
            }
            SettingsChange::ShowImages(enabled) => {
                self.session.settings().set_show_images(enabled);
                for block in self.tool_blocks() {
                    block.borrow_mut().set_show_images(enabled);
                }
            }
            SettingsChange::ImageWidthCells(width) => {
                self.session.settings().set_image_width_cells(width as i64);
                for block in self.tool_blocks() {
                    block.borrow_mut().set_image_width_cells(width as u32);
                }
            }
            SettingsChange::AutoResizeImages(enabled) => {
                self.session.settings().set_image_auto_resize(enabled)
            }
            SettingsChange::BlockImages(blocked) => {
                self.session.settings().set_block_images(blocked)
            }
            SettingsChange::EnableSkillCommands(enabled) => {
                self.session.settings().set_enable_skill_commands(enabled);
                self.setup_autocomplete_provider();
            }
            SettingsChange::Light(enabled) => self.session.settings().set_light(enabled),
            SettingsChange::PluginInstallScope(scope) => {
                self.session.settings().set_plugin_install_scope(scope)
            }
            SettingsChange::EnablePluginTools(enabled) => {
                self.session.settings().set_enable_plugin_tools(enabled)
            }
            SettingsChange::Platform(platforms) => {
                let tokens: Vec<String> =
                    platforms.iter().map(|p| p.as_str().to_string()).collect();
                self.session.settings().set_platform(Some(&tokens));
                set_platforms(Some(&platforms));
            }
            SettingsChange::SteeringMode(mode) => self.session.set_steering_mode(mode),
            SettingsChange::FollowUpMode(mode) => self.session.set_follow_up_mode(mode),
            SettingsChange::Transport(name) => {
                if let Ok(transport) =
                    serde_json::from_value::<Transport>(serde_json::Value::String(name))
                {
                    self.session.settings().set_transport(transport);
                    self.session.agent().set_transport(Some(transport));
                }
            }
            SettingsChange::ThinkingLevel(level) => {
                if let Some(level) = parse_thinking_level(&level) {
                    self.session.set_thinking_level(level);
                    self.footer.borrow_mut().invalidate();
                    self.update_editor_border_color();
                }
            }
            SettingsChange::Theme(name) => {
                let result = set_theme(&name, true);
                self.session.settings().set_theme(&name);
                self.tui.invalidate();
                if let Err(error) = result {
                    self.show_error(&format!(
                        "Failed to load theme \"{name}\": {error}\nFell back to dark theme."
                    ));
                }
            }
            SettingsChange::ThemePreview(name) => {
                if set_theme(&name, true).is_ok() {
                    self.tui.invalidate();
                }
            }
            SettingsChange::HideThinkingBlock(hidden) => {
                self.hide_thinking_block = hidden;
                self.session.settings().set_hide_thinking_block(hidden);
                // `rebuildChatFromMessages`: the transcript again, with the
                // new thinking display.
                self.reset_transcript_view();
                let messages = self.session.messages();
                self.render_session_context(&messages, false);
            }
            SettingsChange::CollapseChangelog(collapsed) => {
                self.session.settings().set_collapse_changelog(collapsed)
            }
            SettingsChange::EnableInstallTelemetry(enabled) => self
                .session
                .settings()
                .set_enable_install_telemetry(enabled),
            SettingsChange::QuietStartup(enabled) => {
                self.session.settings().set_quiet_startup(enabled)
            }
            SettingsChange::TipsEnabled(enabled) => {
                self.session.settings().set_tips_enabled(enabled)
            }
            SettingsChange::DoubleEscapeAction(action) => {
                self.session.settings().set_double_escape_action(action)
            }
            SettingsChange::TreeFilterMode(mode) => {
                self.session.settings().set_tree_filter_mode(mode)
            }
            SettingsChange::ShowHardwareCursor(enabled) => {
                self.session.settings().set_show_hardware_cursor(enabled);
                self.tui.set_show_hardware_cursor(enabled);
            }
            SettingsChange::EditorBorder(border) => {
                self.session.settings().set_editor_border(border);
                let style = match border {
                    EditorBorder::Box => FrameBorderStyle::Box,
                    EditorBorder::Rule => FrameBorderStyle::Rule,
                };
                set_input_frame_border(style);
                self.editor.borrow_mut().editor.set_border(style);
            }
            SettingsChange::EditorPaddingX(padding) => {
                self.session.settings().set_editor_padding_x(padding as i64);
                self.editor
                    .borrow_mut()
                    .editor
                    .set_padding_x(padding as usize);
            }
            SettingsChange::AutocompleteMaxVisible(max_visible) => {
                self.session
                    .settings()
                    .set_autocomplete_max_visible(max_visible as i64);
                self.editor
                    .borrow_mut()
                    .editor
                    .set_autocomplete_max_visible(max_visible as usize);
            }
            SettingsChange::ClearOnShrink(enabled) => {
                self.session.settings().set_clear_on_shrink(enabled);
                self.tui.set_clear_on_shrink(enabled);
            }
            SettingsChange::ShowTerminalProgress(enabled) => {
                self.session.settings().set_show_terminal_progress(enabled)
            }
            SettingsChange::Warnings(warnings) => self.session.settings().set_warnings(warnings),
            SettingsChange::VoiceSilenceMs(ms) => {
                // Voice input is not ported; the value is kept for it.
                self.session.settings().set_voice_silence_ms(ms as i64)
            }
            SettingsChange::WebtoolsTimeoutSecs(secs) => self
                .session
                .settings()
                .set_webtools_timeout_secs(secs as i64),
            SettingsChange::LearnSetting(key, value) => {
                self.session.settings().set_learn_setting(key, value as i64)
            }
        }
    }

    /// Every tool block in the transcript.
    fn tool_blocks(&self) -> Vec<Rc<RefCell<ToolExecutionComponent>>> {
        let mut blocks: Vec<_> = self
            .chains
            .iter()
            .flat_map(|c| c.borrow().tool_blocks().to_vec())
            .collect();
        for block in self.pending_tools.values() {
            if !blocks.iter().any(|b| Rc::ptr_eq(b, block)) {
                blocks.push(block.clone());
            }
        }
        blocks
    }

    /// "Summarize branch?" for a tree selection.
    fn ask_tree_summary(&mut self, entry_id: String) {
        let (reply, answer) = mpsc::channel();
        self.show_selector(
            "Summarize branch?",
            vec![
                "No summary".into(),
                "Summarize".into(),
                "Summarize with custom prompt".into(),
            ],
            reply,
        );
        self.pending_tree_summary = Some((entry_id, answer));
    }

    fn poll_tree_summary(&mut self) {
        let Some((_, answer)) = &self.pending_tree_summary else {
            return;
        };
        let choice = match answer.try_recv() {
            Ok(choice) => choice,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        let Some((entry_id, _)) = self.pending_tree_summary.take() else {
            return;
        };
        match choice.as_deref() {
            // Escape: back to the tree, on the same entry.
            None => self.show_tree_selector(Some(entry_id)),
            Some("Summarize with custom prompt") => {
                let (reply, answer) = mpsc::channel();
                self.show_editor_dialog("Custom summarization instructions", reply);
                self.pending_tree_instructions = Some((entry_id, answer));
            }
            Some(choice) => self.navigate_tree(entry_id, choice != "No summary", None),
        }
    }

    fn poll_tree_instructions(&mut self) {
        let Some((_, answer)) = &self.pending_tree_instructions else {
            return;
        };
        let text = match answer.try_recv() {
            Ok(text) => text,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        let Some((entry_id, _)) = self.pending_tree_instructions.take() else {
            return;
        };
        match text {
            // Cancelled: back to the summary question.
            None => self.ask_tree_summary(entry_id),
            Some(text) => self.navigate_tree(entry_id, true, Some(text)),
        }
    }

    /// `session.navigateTree`: at once without a summary; with one, off the
    /// input loop behind a loader, so escape can cancel it.
    fn navigate_tree(&mut self, entry_id: String, summarize: bool, instructions: Option<String>) {
        let options = NavigateTreeOptions {
            summarize,
            custom_instructions: instructions,
            ..Default::default()
        };
        let session = self.session.clone();
        let target = entry_id.clone();
        if !summarize {
            let result = self
                .runtime
                .block_on(async move { session.navigate_tree(&target, options).await })
                .map_err(|e| e.to_string());
            self.finish_tree_navigation(entry_id, result);
            return;
        }
        self.add_to_chat(as_component(&handle(Spacer::new(1))));
        self.stop_working_loader();
        let mut loader = Loader::new(
            Box::new(|s: &str| theme().fg("accent", s)),
            Box::new(|s: &str| theme().fg("muted", s)),
            format!(
                "Summarizing branch... ({} to cancel)",
                key_text("app.interrupt")
            ),
            None,
        );
        loader.start();
        let loader = handle(loader);
        self.status.borrow_mut().add_child(as_component(&loader));
        self.loader = Some(loader);
        let (done, result) = mpsc::channel();
        let wake = self.tx.clone();
        self.runtime.spawn(async move {
            let outcome = session
                .navigate_tree(&target, options)
                .await
                .map_err(|e| e.to_string());
            let _ = done.send(outcome);
            let _ = wake.send(AppEvent::Rerender);
        });
        self.tree_navigation = Some((entry_id, result));
        self.dirty.set(true);
    }

    fn poll_tree_navigation(&mut self) {
        let Some((_, result)) = &self.tree_navigation else {
            return;
        };
        let outcome = match result.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("Branch summarization failed".into()),
        };
        let Some((entry_id, _)) = self.tree_navigation.take() else {
            return;
        };
        self.stop_working_loader();
        self.finish_tree_navigation(entry_id, outcome);
    }

    /// After `navigateTree`: redraw the transcript for the new position.
    fn finish_tree_navigation(
        &mut self,
        entry_id: String,
        outcome: Result<NavigateTreeResult, String>,
    ) {
        match outcome {
            Ok(result) if result.aborted => {
                self.show_status("Branch summarization cancelled");
                self.show_tree_selector(Some(entry_id));
            }
            Ok(result) if result.cancelled => self.show_status("Navigation cancelled"),
            Ok(result) => {
                self.reset_transcript_view();
                self.render_initial_messages();
                if let Some(text) = result.editor_text {
                    let mut editor = self.editor.borrow_mut();
                    if editor.editor.get_text().trim().is_empty() {
                        editor.editor.set_text(&text);
                    }
                }
                self.show_status("Navigated to selected point");
            }
            Err(error) => self.show_error(&error),
        }
        self.dirty.set(true);
    }

    /// `createBaseAutocompleteProvider` + `setupAutocompleteProvider`: the
    /// built-in commands, then prompt templates, then skill commands. `@`
    /// file completion walks with `fd` (`ensureTool("fd")` at init): an
    /// override, the managed copy or `fd`/`fdfind` on PATH. Downloading a
    /// missing `fd` is not ported, so without one `@` completion stays off,
    /// as in hoocode when the download fails.
    fn setup_autocomplete_provider(&mut self) {
        let mut commands: Vec<CommandEntry> = BUILTIN_SLASH_COMMANDS
            .iter()
            .map(|c| {
                CommandEntry::Slash(SlashCommand {
                    name: c.name.to_string(),
                    description: Some(c.description.to_string()),
                    argument_hint: (c.name == "cd").then(|| "<path>".to_string()),
                    get_argument_completions: if c.name == "model" {
                        let session = self.session.clone();
                        Some(Box::new(move |prefix: &str| {
                            model_argument_completions(&session, prefix)
                        }) as ArgumentCompletionsFn)
                    } else {
                        (c.name == "cd").then(|| {
                            let session = self.session.clone();
                            let previous = self.previous_cwd.clone();
                            Box::new(move |prefix: &str| {
                                let completions = change_directory_completions(
                                    session.cwd(),
                                    previous.borrow().as_deref(),
                                    &home_dir(),
                                    prefix,
                                );
                                (!completions.is_empty()).then_some(completions)
                            }) as ArgumentCompletionsFn
                        })
                    },
                })
            })
            .collect();
        let skill_commands = self.session.settings().enable_skill_commands();
        for info in self.session.resource_loader().slash_commands() {
            if info.source == "skill" && !skill_commands {
                continue;
            }
            commands.push(CommandEntry::Slash(SlashCommand {
                description: prefix_autocomplete_description(info.description, &info.source_info),
                name: info.name,
                argument_hint: None,
                get_argument_completions: None,
            }));
        }
        let fd_path = get_tool_path("fd").map(std::path::PathBuf::from);
        let provider =
            CombinedAutocompleteProvider::new(commands, self.session.cwd().to_path_buf(), fd_path);
        self.editor
            .borrow_mut()
            .editor
            .set_autocomplete_provider(Box::new(provider));
    }

    /// `createBuiltInSlashCommands`: run one.
    fn run_builtin_command(&mut self, command: BuiltinCommand, text: &str) {
        self.editor.borrow_mut().editor.set_text("");
        match command {
            BuiltinCommand::Quit => self.exit_requested = true,
            BuiltinCommand::Resume => self.show_session_selector(),
            BuiltinCommand::Tree => self.show_tree_selector(None),
            BuiltinCommand::Name => self.handle_name_command(text),
            BuiltinCommand::Session => self.handle_session_command(),
            BuiltinCommand::New => self.handle_new_command(),
            BuiltinCommand::Compact => {
                let instructions = text
                    .strip_prefix("/compact ")
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(String::from);
                self.handle_compact_command(instructions);
            }
            BuiltinCommand::Model => {
                let search = text
                    .strip_prefix("/model ")
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(String::from);
                self.handle_model_command(search);
            }
            BuiltinCommand::ScopedModels => self.show_models_selector(),
            BuiltinCommand::Settings => self.show_settings_selector(),
            BuiltinCommand::Login => self.show_oauth_selector(LoginMode::Login),
            BuiltinCommand::Logout => self.show_oauth_selector(LoginMode::Logout),
            BuiltinCommand::Copy => self.handle_copy_command(text),
            BuiltinCommand::Hotkeys => self.handle_hotkeys_command(),
            BuiltinCommand::Changelog => self.handle_changelog_command(),
            BuiltinCommand::Debug => self.handle_debug_command(),
            BuiltinCommand::Color => {
                // An argument sets the slot outright; bare `/color` opens the
                // swatches.
                if !self.handle_color_command(text) {
                    self.show_session_color_selector();
                }
            }
            BuiltinCommand::Chrome => self.handle_chrome_command(text),
            BuiltinCommand::Fork => self.show_user_message_selector(),
            BuiltinCommand::Clone => self.handle_clone_command(),
            BuiltinCommand::Cd => self.handle_change_directory(text),
            BuiltinCommand::Reload => self.handle_reload_command(),
            BuiltinCommand::Export => self.handle_export_command(text),
            BuiltinCommand::Import => self.handle_import_command(text),
            BuiltinCommand::Subagent => self.handle_subagent_command(text),
            BuiltinCommand::Pending(name) => {
                // Wired by its own ledger task (see 11.4e).
                self.show_status(&format!("/{name} is not available yet"));
            }
        }
        self.dirty.set(true);
    }

    /// The session chip, or the plain display name without a theme slot.
    fn current_chip(&self) -> String {
        render_session_chip(
            &self.session.display_name(),
            self.session.session_color_slot() as i64,
        )
        .map(|chip| chip.styled)
        .unwrap_or_else(|| self.session.display_name())
    }

    /// `handleCopy`: the last agent message, the last n turns, or the whole
    /// session, as markdown plus (where the platform can carry it) HTML. The
    /// write runs off the UI thread; [`Self::finish_copy`] reports it.
    fn handle_copy_command(&mut self, text: &str) {
        let argument = text
            .strip_prefix("/copy")
            .unwrap_or(text)
            .trim()
            .to_lowercase();
        // `/copy 0` is a typo, not a request for nothing.
        let turns = (!argument.is_empty() && argument.bytes().all(|b| b.is_ascii_digit()))
            .then(|| argument.parse::<usize>().unwrap_or(usize::MAX).max(1));
        let whole = argument == "all" || argument == "session";
        if !argument.is_empty() && !whole && turns.is_none() {
            self.show_warning("Usage: /copy [all|<number of turns>]");
            return;
        }

        let markdown = if whole || turns.is_some() {
            Some(self.session.get_transcript_markdown(&TranscriptSelection {
                turns,
                user_label: None,
                agent_label: Some(APP_TITLE.to_string()),
            }))
        } else {
            self.session.get_last_assistant_text()
        };
        let Some(markdown) = markdown.filter(|m| !m.is_empty()) else {
            self.show_error(if whole || turns.is_some() {
                "Nothing in this session to copy yet."
            } else {
                "No agent messages to copy yet."
            });
            return;
        };

        let subject = if whole {
            "session transcript".to_string()
        } else if let Some(turns) = turns {
            format!("last {turns} turn{}", if turns > 1 { "s" } else { "" })
        } else {
            "last agent message".to_string()
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let host = SystemClipboardHost {
                native: native_clipboard_writer(),
            };
            let payload = RichPayload {
                html: markdown_to_html(&markdown),
                text: markdown,
            };
            let result = copy_rich_to_clipboard(&host, &payload);
            let _ = tx.send(AppEvent::CopyDone(subject, result));
        });
    }

    /// The end of a `/copy`: name the flavour that landed, since "copied"
    /// meaning markdown on one machine and formatted text on another is how
    /// this gets reported as broken.
    fn finish_copy(&mut self, subject: &str, result: Result<CopyFlavour, String>) {
        match result {
            Ok(flavour) => {
                let as_ = match flavour {
                    CopyFlavour::Rich => "markdown + formatted text",
                    CopyFlavour::Text => "markdown",
                };
                self.show_status(&format!("Copied {subject} as {as_}"));
            }
            Err(reason) => self.show_error(&format!(
                "Could not copy the {subject}: {reason}. /export writes it to a file instead."
            )),
        }
    }

    /// `handleClipboardImagePaste`: read the clipboard's image off the UI
    /// thread; [`Self::insert_pasted_image`] puts its path in the prompt.
    fn handle_clipboard_image_paste(&mut self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let host = SystemClipboardImageHost {
                native: native_clipboard_image_reader(),
            };
            let _ = tx.send(AppEvent::PastedImage(read_clipboard_image(&host)));
        });
    }

    /// Write the pasted image to a temp file and insert its path at the
    /// cursor. Failures are silent (no clipboard access, no image).
    fn insert_pasted_image(&mut self, image: Option<ClipboardImage>) {
        let Some(image) = image else {
            return;
        };
        let ext = extension_for_image_mime_type(&image.mime_type).unwrap_or("png");
        let file_name = format!("{APP_NAME}-clipboard-{}.{ext}", uuid::Uuid::new_v4());
        let path = std::env::temp_dir().join(file_name);
        if std::fs::write(&path, &image.bytes).is_err() {
            return;
        }
        self.editor
            .borrow_mut()
            .editor
            .insert_text_at_cursor(&path.to_string_lossy());
        self.dirty.set(true);
    }

    /// A bordered page in the chat: accent title over markdown
    /// (`handleHotkeys`, `handleChangelog`, the startup "What's New").
    fn show_bordered_markdown(&mut self, title: &str, markdown: &str) {
        let t = theme();
        self.add_to_chat(as_component(&handle(Spacer::new(1))));
        self.add_to_chat(as_component(&handle(DynamicBorder::new(None))));
        self.add_to_chat(as_component(&handle(Text::new(
            t.bold(&t.fg("accent", title)),
            1,
            0,
        ))));
        self.add_to_chat(as_component(&handle(Spacer::new(1))));
        self.add_to_chat(as_component(&handle(Markdown::new(
            markdown,
            1,
            0,
            (self.markdown_theme())(),
            None,
        ))));
        self.add_to_chat(as_component(&handle(DynamicBorder::new(None))));
    }

    /// `handleHotkeys`.
    fn handle_hotkeys_command(&mut self) {
        self.show_bordered_markdown("Keyboard Shortcuts", &hotkeys_markdown());
    }

    /// `handleChangelog`: every entry, oldest first.
    fn handle_changelog_command(&mut self) {
        let entries = parse_changelog(&changelog_path());
        if entries.is_empty() {
            self.add_to_chat(as_component(&handle(Spacer::new(1))));
            self.add_to_chat(as_component(&handle(Text::new(
                theme().fg("dim", "No changelog entries found."),
                1,
                0,
            ))));
            return;
        }
        let markdown = entries
            .iter()
            .rev()
            .map(|e| e.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.show_bordered_markdown("What's New", &markdown);
    }

    /// `showStartupNoticesIfNeeded`: the entries new since the last run,
    /// in full or as one line when the changelog is collapsed.
    fn show_startup_notices(&mut self, changelog: Option<String>) {
        let Some(markdown) = changelog.filter(|m| !m.trim().is_empty()) else {
            return;
        };
        let has_rows = !self.chat.borrow().children.is_empty();
        if has_rows {
            self.add_to_chat(as_component(&handle(Spacer::new(1))));
        }
        self.add_to_chat(as_component(&handle(DynamicBorder::new(None))));
        let t = theme();
        let collapse = self.session.settings().collapse_changelog();
        if collapse {
            let latest = CHANGELOG_VERSION
                .captures(&markdown)
                .map_or_else(|| VERSION.to_string(), |c| c[1].to_string());
            self.add_to_chat(as_component(&handle(Text::new(
                format!(
                    "Updated to v{latest}. Use {} to view full changelog.",
                    t.bold("/changelog")
                ),
                1,
                0,
            ))));
        } else {
            self.add_to_chat(as_component(&handle(Text::new(
                t.bold(&t.fg("accent", "What's New")),
                1,
                0,
            ))));
            self.add_to_chat(as_component(&handle(Spacer::new(1))));
            self.add_to_chat(as_component(&handle(Markdown::new(
                markdown.trim(),
                1,
                0,
                (self.markdown_theme())(),
                None,
            ))));
        }
        self.add_to_chat(as_component(&handle(DynamicBorder::new(None))));
    }

    /// `handleDebug`: every rendered line with its width, and the messages
    /// as JSON lines, written to the debug log.
    fn handle_debug_command(&mut self) {
        let (width, height) = self.size.get();
        let lines = self.tui.render(width);
        let mut data = vec![
            format!("Debug output at {}", iso_now()),
            format!("Terminal: {width}x{height}"),
            format!("Total lines: {}", lines.len()),
            String::new(),
            "=== All rendered lines with visible widths ===".to_string(),
        ];
        for (idx, line) in lines.iter().enumerate() {
            let escaped = serde_json::to_string(line).unwrap_or_default();
            data.push(format!("[{idx}] (w={}) {escaped}", visible_width(line)));
        }
        data.push(String::new());
        data.push("=== Agent messages (JSONL) ===".to_string());
        for message in self.session.messages() {
            data.push(serde_json::to_string(&message).unwrap_or_default());
        }
        data.push(String::new());

        let path = cortexcode_code_paths::debug_log_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, data.join("\n")) {
            self.show_error(&format!("Failed to write debug log: {error}"));
            return;
        }
        let t = theme();
        self.add_to_chat(as_component(&handle(Spacer::new(1))));
        self.add_to_chat(as_component(&handle(Text::new(
            format!(
                "{}\n{}",
                t.fg("accent", "✓ Debug log written"),
                t.fg("muted", &path.to_string_lossy())
            ),
            1,
            0,
        ))));
    }

    /// `handleColor`: `/color <slot|name>`; false for a bare `/color`.
    fn handle_color_command(&mut self, text: &str) -> bool {
        let arg = text.strip_prefix("/color").unwrap_or(text).trim();
        if arg.is_empty() {
            return false;
        }
        let Some(slot) = parse_session_color_slot(arg) else {
            self.show_warning(&format!(
                "Usage: /color <1-{SESSION_COLOR_SLOTS}> or /color <{}> (first letter works too), or /color on its own to pick one",
                session_color_name_list().join("|")
            ));
            return true;
        };
        self.session.set_session_color(slot);
        let t = theme();
        let name = session_color_name(slot)
            .map(|name| format!("  {}", t.fg("dim", name)))
            .unwrap_or_default();
        let line = format!(
            "{} {}{name}",
            t.fg("dim", "Session color set:"),
            self.current_chip()
        );
        self.add_to_chat(as_component(&handle(Spacer::new(1))));
        self.add_to_chat(as_component(&handle(Text::new(line, 1, 0))));
        true
    }

    /// `cycleSessionColor`: one step on the colour dial.
    fn cycle_session_color(&mut self, forward: bool) {
        let direction = if forward {
            SessionCycleDirection::Forward
        } else {
            SessionCycleDirection::Backward
        };
        let slot =
            cycle_session_color_slot(f64::from(self.session.session_color_slot()), direction);
        self.session.set_session_color(slot);
        let name = session_color_name(slot).map_or_else(|| slot.to_string(), str::to_string);
        self.show_dial_step(
            "app.session.color.cycleBackward",
            &format!("Session color: {name}"),
        );
    }

    /// `showSessionColorSelector`: swatches in the prompt's slot; moving
    /// through them repaints the live chip.
    fn show_session_color_selector(&mut self) {
        let original = self.session.session_color_slot();
        let selector = session_color_selector(&self.session.display_name(), original);
        {
            let list = selector.select_list();
            let mut list = list.borrow_mut();
            let slot_of = |item: &SelectItem| item.value.parse::<u8>().ok();
            let sink = self.actions.clone();
            list.on_selection_change = Some(Box::new(move |item| {
                if let Some(slot) = slot_of(item) {
                    sink.borrow_mut().push(Action::SessionColorPreview(slot));
                }
            }));
            let sink = self.actions.clone();
            list.on_select = Some(Box::new(move |item| {
                sink.borrow_mut()
                    .push(Action::SessionColorDone(slot_of(item)));
            }));
            let sink = self.actions.clone();
            list.on_cancel = Some(Box::new(move || {
                sink.borrow_mut().push(Action::SessionColorDone(None));
            }));
        }
        self.show_in_editor_slot(as_component(&handle(selector)));
    }

    /// Paint the prompt's chip in `slot` without saving it.
    fn preview_session_color(&mut self, slot: u8) {
        let chip = render_session_chip(&self.session.display_name(), i64::from(slot));
        self.editor.borrow_mut().editor.top_border_label = chip;
        self.dirty.set(true);
    }

    /// The colour picker closed: save the choice, or put back the colour
    /// the session actually has.
    fn close_session_color_selector(&mut self, slot: Option<u8>) {
        self.restore_editor();
        match slot {
            Some(slot) => {
                self.session.set_session_color(slot);
                let name =
                    session_color_name(slot).map_or_else(|| slot.to_string(), str::to_string);
                self.show_status(&format!("Session color: {name}"));
            }
            None => self.update_session_chip(),
        }
    }

    /// `handleChromeCommand`: `/chrome` names the stop, `/chrome <stop>`
    /// sets it (the dial's only way in where alt never arrives).
    fn handle_chrome_command(&mut self, text: &str) {
        let argument = text
            .strip_prefix("/chrome")
            .unwrap_or(text)
            .trim()
            .to_lowercase();
        let all: Vec<&str> = ChromeDensity::ALL.iter().map(|d| d.as_str()).collect();
        if argument.is_empty() {
            self.show_status(&format!(
                "Chrome: {} — {}",
                self.chrome.density(),
                all.join(" · ")
            ));
            return;
        }
        let Some(density) = ChromeDensity::parse(&argument) else {
            self.show_error(&format!(
                "Unknown chrome density \"{argument}\". Try: {}",
                all.join(", ")
            ));
            return;
        };
        self.chrome.set_density(density);
        self.session.settings().set_chrome_density(density);
        self.dirty.set(true);
        self.show_status(&format!("Chrome: {density}"));
    }

    /// `showUserMessageSelector`: `/fork` picks a user message; the branch
    /// before it becomes a new session and its text returns to the prompt.
    fn show_user_message_selector(&mut self) {
        let messages = self.session.get_user_messages_for_forking();
        let Some(last) = messages.last() else {
            self.show_status("No messages to fork from");
            return;
        };
        let initial = last.entry_id.clone();
        let items = messages
            .into_iter()
            .map(|m| UserMessageItem {
                id: m.entry_id,
                text: m.text,
                timestamp: None,
            })
            .collect();
        let selector = handle(UserMessageSelectorComponent::new(items, Some(&initial)));
        self.show_in_editor_slot(as_component(&selector));
        self.fork_selector = Some(selector);
    }

    fn poll_fork_selector(&mut self) {
        let Some(selector) = &self.fork_selector else {
            return;
        };
        let Some(event) = selector
            .borrow_mut()
            .poll(Instant::now())
            .into_iter()
            .next()
        else {
            return;
        };
        self.fork_selector = None;
        self.restore_editor();
        if let UserMessageEvent::Select(entry_id) = event {
            self.fork_session(&entry_id, ForkPosition::Before);
        }
    }

    /// `runtimeHost.fork`, then the new session's transcript. `Before` puts
    /// the forked message's text back in the prompt (`/fork`); `At` copies
    /// the branch whole (`/clone`).
    fn fork_session(&mut self, entry_id: &str, position: ForkPosition) {
        self.stop_working_loader();
        let handle_rt = self.runtime.clone();
        let Some(runtime) = self.session_runtime.as_mut() else {
            return;
        };
        match handle_rt.block_on(runtime.fork(entry_id, position)) {
            Ok(result) if result.cancelled => self.dirty.set(true),
            Ok(result) => {
                self.rebind_current_session();
                self.render_current_session_state();
                self.editor
                    .borrow_mut()
                    .editor
                    .set_text(result.selected_text.as_deref().unwrap_or(""));
                self.show_status(if position == ForkPosition::At {
                    "Cloned to new session"
                } else {
                    "Forked to new session"
                });
            }
            Err(error) => self.show_error(&error.to_string()),
        }
    }

    /// `handleClone`: the whole current branch into a new session.
    fn handle_clone_command(&mut self) {
        let leaf = self.session.session_manager().leaf_id().map(str::to_string);
        match leaf {
            Some(leaf) => self.fork_session(&leaf, ForkPosition::At),
            None => self.show_status("Nothing to clone yet"),
        }
    }

    /// `handleChangeDirectory`: `/cd [path|~|-]` moves the runtime to a new
    /// session in the target directory.
    fn handle_change_directory(&mut self, text: &str) {
        let raw = text.strip_prefix("/cd").unwrap_or(text).trim();
        let previous_cwd = self.session.cwd().to_path_buf();
        let home = home_dir();
        let target = if raw.is_empty() || raw == "~" {
            home.clone()
        } else if raw == "-" {
            let Some(previous) = self.previous_cwd.borrow().clone() else {
                self.show_warning("No previous directory to return to");
                return;
            };
            previous
        } else {
            let expanded = match raw.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(raw),
            };
            resolve_path(&previous_cwd, &expanded)
        };
        if resolve_path(Path::new("/"), &target) == resolve_path(Path::new("/"), &previous_cwd) {
            self.show_status(&format!("Already in {}", target.display()));
            return;
        }

        self.stop_working_loader();
        let handle_rt = self.runtime.clone();
        let Some(runtime) = self.session_runtime.as_mut() else {
            return;
        };
        match handle_rt.block_on(runtime.change_directory(&target)) {
            Ok(result) if result.cancelled => {}
            Ok(result) => {
                *self.previous_cwd.borrow_mut() = Some(previous_cwd.clone());
                self.rebind_current_session();
                self.render_current_session_state();
                let t = theme();
                let line = format!(
                    "{} {}\n{}",
                    t.fg("accent", "✓ Working directory"),
                    t.fg("muted", &result.cwd.to_string_lossy()),
                    t.fg(
                        "dim",
                        &format!(
                            "New session started here. {} reopens the session you left in {}.",
                            key_display_text("app.session.resume"),
                            previous_cwd.display()
                        )
                    )
                );
                self.add_to_chat(as_component(&handle(Spacer::new(1))));
                self.add_to_chat(as_component(&handle(Text::new(line, 1, 0))));
            }
            Err(RuntimeError::ChangeDirectory { message, .. }) => self.show_error(&message),
            Err(error) => {
                // `handleFatalRuntimeError`.
                self.show_error(&format!(
                    "Failed to change directory to {}: {error}",
                    target.display()
                ));
                self.exit_requested = true;
            }
        }
        self.dirty.set(true);
    }

    /// `handleReloadCommand`: re-read keybindings, settings, resources and
    /// themes, then replay the transcript with the listing below it.
    fn handle_reload_command(&mut self) {
        if self.session.is_streaming() {
            self.show_warning("Wait for the current response to finish before reloading.");
            return;
        }
        if self.session.is_compacting() {
            self.show_warning("Wait for compaction to finish before reloading.");
            return;
        }
        let t = theme();
        let mut reload_box = Container::new();
        reload_box.add_child(as_component(&handle(DynamicBorder::new(Some(Box::new(
            |s: &str| theme().fg("border", s),
        ))))));
        reload_box.add_child(as_component(&handle(Text::new(
            t.fg(
                "muted",
                "Reloading keybindings, extensions, skills, prompts, themes...",
            ),
            1,
            0,
        ))));
        reload_box.add_child(as_component(&handle(DynamicBorder::new(Some(Box::new(
            |s: &str| theme().fg("border", s),
        ))))));
        self.show_in_editor_slot(as_component(&handle(reload_box)));
        self.tui.request_render(true);

        let session = self.session.clone();
        self.runtime.block_on(async move { session.reload().await });
        AppKeybindingsManager::create(None).install();
        let theme_name = self.session.settings().theme();
        if let Some(name) = theme_name {
            if let Err(error) = set_theme(&name, true) {
                self.show_error(&format!("Failed to load theme \"{name}\": {error}"));
            }
        }
        self.update_editor_border_color();
        self.setup_autocomplete_provider();
        self.reset_transcript_view();
        // From the session file, as the pin replays it: messages recorded
        // only there (a `/subagent` answer) show too.
        let messages = self.session.session_manager().build_context().messages;
        self.render_session_context(&messages, false);
        self.restore_editor();
        // Below the replayed transcript: /reload is asked for to see what
        // just loaded.
        self.render_resources();
        self.update_available_provider_count();
        if let Some(error) = self.session.model_registry().error() {
            self.show_error(&format!("models.json error: {error}"));
        }
        self.show_status("Reloaded keybindings, extensions, skills, prompts, themes");
    }

    /// `handleExport`: `/export <path.jsonl>` writes the branch as JSONL.
    /// The HTML export (every other path) is export-html, owned by 12.7.
    fn handle_export_command(&mut self, text: &str) {
        let output = command_path_argument(text, "/export");
        match output.as_deref() {
            Some(path) if path.ends_with(".jsonl") => {
                match self.session.export_to_jsonl(Some(Path::new(path))) {
                    Ok(file) => {
                        self.show_record(&format!("Session exported to: {}", file.display()))
                    }
                    Err(error) => {
                        self.show_error(&format!("Failed to export session: {error}"))
                    }
                }
            }
            _ => self.show_error(
                "Failed to export session: HTML export is not available yet; use /export <file.jsonl>",
            ),
        }
    }

    /// `handleImport`: `/import <path.jsonl>` replaces the session, after a
    /// confirm.
    fn handle_import_command(&mut self, text: &str) {
        let Some(input) = command_path_argument(text, "/import") else {
            self.show_error("Usage: /import <path.jsonl>");
            return;
        };
        let (reply, answer) = mpsc::channel();
        self.show_selector(
            &format!("Import session\nReplace current session with {input}?"),
            vec!["Yes".into(), "No".into()],
            reply,
        );
        self.pending_import = Some((input, None, answer));
    }

    /// An `/import` confirm answered.
    fn poll_import_confirm(&mut self) {
        let Some((_, _, answer)) = &self.pending_import else {
            return;
        };
        let confirmed = match answer.try_recv() {
            Ok(choice) => choice.as_deref() == Some("Yes"),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => false,
        };
        let Some((input, fallback, _)) = self.pending_import.take() else {
            return;
        };
        if confirmed {
            self.import_session(input, fallback);
        } else {
            self.show_status("Import cancelled");
        }
    }

    /// `runtimeHost.importFromJsonl`; a stored cwd that is gone asks first.
    fn import_session(&mut self, input: String, cwd_override: Option<String>) {
        self.stop_working_loader();
        let overridden = cwd_override.is_some();
        let handle_rt = self.runtime.clone();
        let Some(runtime) = self.session_runtime.as_mut() else {
            return;
        };
        match handle_rt.block_on(runtime.import_from_jsonl(Path::new(&input), cwd_override)) {
            Ok(result) if result.cancelled => self.show_status("Import cancelled"),
            Ok(_) => {
                self.rebind_current_session();
                self.render_current_session_state();
                self.show_record(&format!("Session imported from: {input}"));
            }
            Err(RuntimeError::MissingSessionCwd(issue)) if !overridden => {
                let (reply, answer) = mpsc::channel();
                let title = format!(
                    "Session cwd not found\n{}",
                    format_missing_session_cwd_prompt(&issue)
                );
                self.show_selector(&title, vec!["Yes".into(), "No".into()], reply);
                self.pending_import = Some((input, Some(issue.fallback_cwd), answer));
            }
            Err(error @ RuntimeError::ImportFileNotFound(_)) => {
                self.show_error(&format!("Failed to import session: {error}"))
            }
            Err(error) => {
                // `handleFatalRuntimeError`.
                self.show_error(&format!("Failed to import session: {error}"));
                self.exit_requested = true;
            }
        }
        self.dirty.set(true);
    }

    /// `handleSubagent`: `/subagent <mode> <task>` runs one subagent of that
    /// type off the UI thread; [`Self::finish_subagent`] reports it.
    fn handle_subagent_command(&mut self, text: &str) {
        const USAGE: &str = "Usage: /subagent <mode> <task>";
        let args = text.strip_prefix("/subagent ").map_or("", str::trim);
        let Some((mode, task)) = args.split_once(' ') else {
            self.show_status(USAGE);
            return;
        };
        let (mode, task) = (mode.trim().to_string(), task.trim().to_string());
        if task.is_empty() {
            self.show_status(USAGE);
            return;
        }
        let cwd = self.session.cwd().to_path_buf();
        let registry = load_agent_registry(&LoadAgentRegistryOptions::new(
            cwd.to_string_lossy().into_owned(),
        ));
        let valid: Vec<String> = registry.list().iter().map(|a| a.name.clone()).collect();
        if !valid.contains(&mode) {
            self.show_status(&format!(
                "Unknown subagent_type: {mode}. Available: {}",
                valid.join(", ")
            ));
            return;
        }

        self.show_status(&format!("Spawning {mode} subagent..."));
        let available = self.session.get_available_models();
        let model = self.session.model();
        let options = DispatchOptions {
            force_agent: Some(mode.clone()),
            model: model.as_ref().map(|m| m.id.clone()),
            provider: model.as_ref().map(|m| m.provider.clone()),
            ..Default::default()
        };
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            // The pool's lifeguard runs on this runtime, so it is made here.
            let pool = get_subagent_pool(&cwd, &available);
            let outcome = match pool.dispatch(&task, options).await {
                Ok(dispatched) => match dispatched.result {
                    Some(result) if result.ok => Ok(result
                        .result_data
                        .as_ref()
                        .and_then(|data| data.get("summary"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)),
                    result => Err(format!(
                        "Subagent ({mode}) failed: {}",
                        result
                            .and_then(|r| r.error)
                            .unwrap_or_else(|| "unknown error".into())
                    )),
                },
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(AppEvent::SubagentDone(mode, outcome));
        });
    }

    /// A `/subagent` run ended. Its answer joins the session as a displayed
    /// custom message (seen when the transcript is next drawn).
    fn finish_subagent(&mut self, mode: &str, result: Result<Option<String>, String>) {
        match result {
            Ok(summary) => {
                self.show_status(&format!("{mode} subagent completed"));
                let summary = summary
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "(no output)".into());
                self.session
                    .session_manager()
                    .append_message(AgentMessage::Custom(CustomMessage {
                        custom_type: "subagent".into(),
                        content: UserContent::Text(summary),
                        display: true,
                        details: None,
                        timestamp: cortexcode_ai_types::now_ms(),
                    }));
            }
            Err(error) => self.show_error(&error),
        }
    }

    /// `handleName`.
    fn handle_name_command(&mut self, text: &str) {
        let name = text.strip_prefix("/name").unwrap_or("").trim();
        let t = theme();
        if name.is_empty() {
            let label = if self.session.session_name().is_some() {
                "Session name:"
            } else {
                "Session name (auto):"
            };
            let line = format!(
                "{} {}  {}",
                t.fg("dim", label),
                self.current_chip(),
                t.fg("dim", "/name <name> to change")
            );
            self.add_to_chat(as_component(&handle(Spacer::new(1))));
            self.add_to_chat(as_component(&handle(Text::new(line, 1, 0))));
            return;
        }
        self.session.set_session_name(name);
        let message = format!(
            "{} {}",
            t.fg("dim", "Session name set:"),
            self.current_chip()
        );
        self.show_status(&message);
    }

    /// `handleSession`: the session's facts in the transcript.
    fn handle_session_command(&mut self) {
        let stats = self.session.get_session_stats();
        let t = theme();
        let dim = |s: &str| t.fg("dim", s);
        let mut info = format!("{}\n\n", t.bold("Session Info"));
        let name_label = if self.session.session_name().is_some() {
            "Name:"
        } else {
            "Name (auto):"
        };
        info += &format!("{} {}\n", dim(name_label), self.current_chip());
        if let Some(branch) = self.session.session_manager().session_branch() {
            info += &format!("{} {branch}\n", dim("Branch:"));
        }
        info += &format!(
            "{} {}\n",
            dim("File:"),
            stats.session_file.as_deref().unwrap_or("In-memory")
        );
        info += &format!("{} {}\n\n", dim("ID:"), stats.session_id);
        info += &format!("{}\n", t.bold("Messages"));
        info += &format!("{} {}\n", dim("User:"), stats.user_messages);
        info += &format!("{} {}\n", dim("Assistant:"), stats.assistant_messages);
        info += &format!("{} {}\n", dim("Tool Calls:"), stats.tool_calls);
        info += &format!("{} {}\n", dim("Tool Results:"), stats.tool_results);
        info += &format!("{} {}\n\n", dim("Total:"), stats.total_messages);
        info += &format!("{}\n", t.bold("Tokens"));
        info += &format!("{} {}\n", dim("Input:"), group_digits(stats.tokens.input));
        info += &format!("{} {}\n", dim("Output:"), group_digits(stats.tokens.output));
        if stats.tokens.cache_read > 0 {
            info += &format!(
                "{} {}\n",
                dim("Cache Read:"),
                group_digits(stats.tokens.cache_read)
            );
        }
        if stats.tokens.cache_write > 0 {
            info += &format!(
                "{} {}\n",
                dim("Cache Write:"),
                group_digits(stats.tokens.cache_write)
            );
        }
        info += &format!("{} {}\n", dim("Total:"), group_digits(stats.tokens.total));
        if stats.cost > 0.0 {
            info += &format!("\n{}\n", t.bold("Cost"));
            info += &format!("{} {:.4}", dim("Total:"), stats.cost);
        }
        self.add_to_chat(as_component(&handle(Spacer::new(1))));
        self.add_to_chat(as_component(&handle(Text::new(info, 1, 0))));
    }

    /// `handleClear` (`/new`): a fresh session in the same directory.
    fn handle_new_command(&mut self) {
        self.stop_working_loader();
        let handle_rt = self.runtime.clone();
        let Some(runtime) = self.session_runtime.as_mut() else {
            return;
        };
        match handle_rt.block_on(runtime.new_session(NewSessionRequest::default())) {
            Ok(result) if result.cancelled => {}
            Ok(_) => {
                self.rebind_current_session();
                self.render_current_session_state();
                self.add_to_chat(as_component(&handle(Spacer::new(1))));
                let line = theme().fg("accent", "✓ New session started");
                self.add_to_chat(as_component(&handle(Text::new(line, 1, 0))));
            }
            Err(error) => {
                // `handleFatalRuntimeError`.
                self.show_error(&format!("Failed to create session: {error}"));
                self.exit_requested = true;
            }
        }
    }

    /// `handleCompactCommand`: the session reports the outcome through its
    /// compaction events.
    fn handle_compact_command(&mut self, instructions: Option<String>) {
        self.stop_working_loader();
        let session = self.session.clone();
        self.runtime.spawn(async move {
            let _ = session.compact(instructions.as_deref()).await;
        });
    }

    /// `showSessionSelector`: the session picker in the editor's slot.
    fn show_session_selector(&mut self) {
        if self.session_runtime.is_none() || self.session_selector.is_some() {
            return;
        }
        let (session_dir, current_file) = {
            let manager = self.session.session_manager();
            let dir = manager.session_dir().to_path_buf();
            let dir = if dir.as_os_str().is_empty() {
                cortexcode_code_session::default_session_dir(manager.cwd())
            } else {
                dir
            };
            (dir, manager.session_file().map(Path::to_path_buf))
        };
        let on_select = self.actions.clone();
        let on_cancel = self.actions.clone();
        let selector = handle(SessionSelectorComponent::new(
            session_picker::current_sessions_loader(session_dir),
            session_picker::all_sessions_loader(),
            Box::new(move |path| {
                on_select
                    .borrow_mut()
                    .push(Action::SessionSelectorDone(Some(path)))
            }),
            Box::new(move || {
                on_cancel
                    .borrow_mut()
                    .push(Action::SessionSelectorDone(None))
            }),
            SessionSelectorOptions {
                rename_session: Some(Box::new(|path: &Path, next: &str| {
                    let next = next.trim();
                    if next.is_empty() {
                        return Ok(());
                    }
                    let mut manager = SessionManager::open(path, None, None);
                    manager.append_session_info(Some(next), None);
                    Ok(())
                })),
                show_rename_hint: Some(true),
                keybindings: None,
            },
            current_file.as_deref(),
        ));
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&selector));
        }
        self.tui.set_focus(Some(as_component(&selector)));
        self.session_selector = Some(selector);
        self.dirty.set(true);
    }

    fn close_session_selector(&mut self) {
        if self.session_selector.take().is_none() {
            return;
        }
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(as_component(&self.editor));
        }
        self.tui.set_focus(Some(as_component(&self.editor)));
        self.dirty.set(true);
    }

    /// `handleResumeSession`: swap to `path`; a stored cwd that is gone asks
    /// first (`promptForMissingSessionCwd`).
    fn handle_resume_session(&mut self, path: PathBuf, cwd_override: Option<String>) {
        if self.session_runtime.is_none() {
            return;
        }
        self.stop_working_loader();
        let overridden = cwd_override.is_some();
        let handle = self.runtime.clone();
        let Some(runtime) = self.session_runtime.as_mut() else {
            return;
        };
        let result = handle.block_on(runtime.switch_session(&path, cwd_override));
        match result {
            Ok(result) if result.cancelled => {}
            Ok(_) => {
                self.rebind_current_session();
                self.render_current_session_state();
                self.show_status(if overridden {
                    "Resumed session in current cwd"
                } else {
                    "Resumed session"
                });
            }
            Err(RuntimeError::MissingSessionCwd(issue)) if !overridden => {
                let (reply, answer) = mpsc::channel();
                let title = format!(
                    "Session cwd not found\n{}",
                    format_missing_session_cwd_prompt(&issue)
                );
                self.show_selector(&title, vec!["Yes".into(), "No".into()], reply);
                self.pending_cwd_prompt = Some((path, issue.fallback_cwd, answer));
            }
            Err(error) => {
                // `handleFatalRuntimeError`.
                self.show_error(&format!("Failed to resume session: {error}"));
                self.exit_requested = true;
            }
        }
    }

    /// The missing-cwd confirm answered: resume in the fallback cwd, or not.
    fn poll_cwd_prompt(&mut self) {
        let Some((_, _, answer)) = &self.pending_cwd_prompt else {
            return;
        };
        let confirmed = match answer.try_recv() {
            Ok(choice) => choice.as_deref() == Some("Yes"),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => false,
        };
        let Some((path, fallback, _)) = self.pending_cwd_prompt.take() else {
            return;
        };
        if confirmed {
            self.handle_resume_session(path, Some(fallback));
        } else {
            self.show_status("Resume cancelled");
        }
    }

    /// `rebindCurrentSession`: point the mode at the runtime's session.
    fn rebind_current_session(&mut self) {
        let Some(runtime) = &self.session_runtime else {
            return;
        };
        self.subscription = None;
        self.session = runtime.session().clone();
        self.footer.borrow_mut().set_source(Box::new(SessionFooter {
            session: self.session.clone(),
            is_oauth: self.is_oauth.clone(),
        }));
        self.footer_data.set_cwd(self.session.cwd());
        self.subscription = Some(self.subscribe());
        self.update_editor_border_color();
        self.update_session_chip();
        self.update_terminal_title();
        self.setup_autocomplete_provider();
        self.update_available_provider_count();
    }

    /// `resetTranscriptView`: drop every view reference into the transcript.
    fn reset_transcript_view(&mut self) {
        self.chat.borrow_mut().clear();
        self.open_chain = None;
        self.latest_block = None;
        self.latest_chain = None;
        self.streaming = None;
        self.streaming_message = None;
        self.pending_tools.clear();
        self.chains.clear();
        self.assistant_components.clear();
        self.bash_components.clear();
        self.branch_summaries.clear();
        self.compaction_summaries.clear();
        self.last_status = None;
    }

    /// `renderCurrentSessionState`: the transcript of the session just
    /// swapped in, after its resource listing.
    fn render_current_session_state(&mut self) {
        self.reset_transcript_view();
        self.render_resources();
        self.render_initial_messages();
    }

    /// `showStatus`: a passing status in the notification band above the
    /// prompt (the first line is the title, the rest its body).
    fn show_status(&mut self, message: &str) {
        self.notify(NotificationKind::Info, message);
    }

    /// `showWarning`: on the notification band, not in the transcript.
    fn show_warning(&mut self, message: &str) {
        self.notify(NotificationKind::Warning, message);
    }

    fn notify(&mut self, kind: NotificationKind, message: &str) {
        let mut lines = message.split('\n');
        let title = lines.next().unwrap_or("");
        let body: Vec<&str> = lines.collect();
        self.notifications
            .borrow_mut()
            .notify(kind, title, &body, None, None, None);
        self.dirty.set(true);
    }

    /// `showRecord`: a dim line in the chat that stays; back-to-back records
    /// update the previous line instead of stacking.
    fn show_record(&mut self, message: &str) {
        let styled = if message.contains("\x1b[") {
            message.to_string()
        } else {
            theme().fg("dim", message)
        };
        if let Some((spacer, text)) = &self.last_status {
            let chat = self.chat.borrow();
            let n = chat.children.len();
            let text_handle = as_component(text);
            if n >= 2
                && Rc::ptr_eq(&chat.children[n - 1], &text_handle)
                && Rc::ptr_eq(&chat.children[n - 2], spacer)
            {
                text.borrow_mut().set_text(styled);
                drop(chat);
                self.dirty.set(true);
                return;
            }
        }
        let spacer = as_component(&handle(Spacer::new(1)));
        let text = handle(Text::new(styled, 1, 0));
        self.add_to_chat(spacer.clone());
        self.add_to_chat(as_component(&text));
        self.last_status = Some((spacer, text));
    }

    /// `showDialStep`: the stop a dial landed on, and (the first time) how to
    /// step back.
    fn show_dial_step(&mut self, backward: &'static str, message: &str) {
        let taught = !self.dial_reverse_taught.insert(backward);
        let topic = backward
            .strip_suffix(".cycleForward")
            .or_else(|| backward.strip_suffix(".cycleBackward"))
            .unwrap_or(backward);
        let note = (!taught).then(|| format!("{} steps back", key_text(backward)));
        self.notifications.borrow_mut().notify(
            NotificationKind::Info,
            message,
            &[],
            note.as_deref(),
            None,
            Some(topic),
        );
        self.dirty.set(true);
    }

    /// `applyToolOutputView`: move every block and chain to `view`.
    fn apply_tool_output_view(&mut self, view: ToolOutputView, persist: bool) {
        let previous_thinking = self.thinking_display();
        let was_expanded = self.tool_output_view == MAX_TOOL_OUTPUT_VIEW;
        self.tool_output_view = view;
        if persist {
            self.session.settings().set_tool_output_view(view);
            self.view_before_jump = None;
        }
        self.footer.borrow_mut().set_tool_output_view(view);
        let thinking = self.thinking_display();
        for chain in &self.chains {
            chain.borrow_mut().set_view(view);
        }
        if thinking != previous_thinking {
            for component in &self.assistant_components {
                component.borrow_mut().set_thinking_display(thinking);
            }
        }
        let expanded = view == MAX_TOOL_OUTPUT_VIEW;
        if expanded != was_expanded {
            // "full" holds nothing back: the header opens with it.
            self.expanded = expanded;
            self.header.borrow_mut().set_expanded(expanded);
            for component in &self.bash_components {
                component.borrow_mut().set_expanded(expanded);
            }
            for component in &self.branch_summaries {
                component.borrow_mut().set_expanded(expanded);
            }
            for component in &self.compaction_summaries {
                component.borrow_mut().set_expanded(expanded);
            }
        }
        self.dirty.set(true);
    }

    /// `jumpToFullView`: to `full`, or back to where the jump started.
    fn jump_to_full_view(&mut self) {
        if self.tool_output_view == MAX_TOOL_OUTPUT_VIEW {
            let back = self
                .view_before_jump
                .take()
                .unwrap_or(DEFAULT_TOOL_OUTPUT_VIEW);
            self.apply_tool_output_view(back, false);
            return;
        }
        self.view_before_jump = Some(self.tool_output_view);
        self.apply_tool_output_view(MAX_TOOL_OUTPUT_VIEW, false);
    }

    /// `cycleToolOutputView`: one stop on the dial, saved.
    fn cycle_tool_output_view(&mut self, forward: bool) {
        let next = cycle_tool_output_view(self.tool_output_view, forward);
        self.apply_tool_output_view(next, true);
        self.show_dial_step(
            if forward {
                "app.view.cycleBackward"
            } else {
                "app.view.cycleForward"
            },
            &format!("Tool output: {next}"),
        );
    }

    /// `showTurnCost`: this request's own tokens, time and cost.
    fn show_turn_cost(&mut self) {
        let Some((anchor, at)) = self.turn_cost_anchor.take() else {
            return;
        };
        let now = sum_assistant_usage(self.session.session_manager().entries());
        let input = now.input as i64 - anchor.input as i64;
        let output = now.output as i64 - anchor.output as i64;
        let cost = now.cost - anchor.cost;
        // Nothing was accounted: stay silent rather than print zeroes.
        if input <= 0 && output <= 0 {
            return;
        }
        let t = theme();
        let mut segs = vec![
            format!(
                "{}{}{}{}",
                t.fg("dim", "↑"),
                t.fg("muted", &format_tokens(input.max(0) as u64)),
                t.fg("dim", " ↓"),
                t.fg("muted", &format_tokens(output.max(0) as u64)),
            ),
            t.fg("muted", &format_duration_secs(at.elapsed().as_secs_f64())),
        ];
        if cost > 0.0 {
            segs.push(t.fg(
                "muted",
                &format!(
                    "${}",
                    cortexcode_code_agent_session::format::js_to_fixed(cost, 3)
                ),
            ));
        }
        let separator = t.fg("dim", " · ");
        self.add_to_chat(as_component(&handle(Spacer::new(1))));
        self.add_to_chat(as_component(&handle(Text::new(
            segs.join(&separator),
            1,
            0,
        ))));
    }

    fn handle_session_event(&mut self, event: AgentSessionEvent) {
        match event {
            AgentSessionEvent::Agent(event) => self.handle_agent_event(event),
            AgentSessionEvent::ThinkingLevelChanged { .. } => self.update_editor_border_color(),
            AgentSessionEvent::CompactionStart { reason } => self.on_compaction_start(reason),
            AgentSessionEvent::QueueUpdate { .. } => self.update_pending_messages_display(),
            AgentSessionEvent::CompactionEnd {
                reason,
                result,
                aborted,
                will_retry,
                error_message,
            } => {
                self.on_compaction_end(reason, result, aborted, error_message);
                self.flush_compaction_queue(will_retry);
            }
            AgentSessionEvent::SessionInfoChanged { .. } => {
                self.update_session_chip();
                self.update_terminal_title();
            }
            _ => {}
        }
        self.dirty.set(true);
    }

    /// `session.prompt(text, {streamingBehavior})` for a message typed while
    /// the agent works: it only queues, so it has no turn of its own to settle.
    fn queue_prompt(&mut self, text: String, behavior: StreamingBehavior) {
        let session = self.session.clone();
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = session
                .prompt(
                    &text,
                    PromptOptions {
                        expand_prompt_templates: true,
                        streaming_behavior: Some(behavior),
                        ..Default::default()
                    },
                )
                .await;
            if let Err(error) = result {
                let _ = tx.send(AppEvent::QueueError(error.to_string()));
            }
        });
        self.update_pending_messages_display();
    }

    /// `updatePendingMessagesDisplay`: the queued steering and follow-up
    /// messages above the prompt, with the dequeue hint.
    fn update_pending_messages_display(&mut self) {
        let mut steering = self.session.get_steering_messages();
        let mut follow_up = self.session.get_follow_up_messages();
        for (text, behavior) in &self.compaction_queue {
            match behavior {
                StreamingBehavior::Steer => steering.push(text.clone()),
                StreamingBehavior::FollowUp => follow_up.push(text.clone()),
            }
        }
        let mut pending = self.pending_messages.borrow_mut();
        pending.clear();
        if steering.is_empty() && follow_up.is_empty() {
            self.dirty.set(true);
            return;
        }
        let t = theme();
        pending.add_child(as_component(&handle(Spacer::new(1))));
        let rows = steering
            .iter()
            .map(|m| format!("Steering: {m}"))
            .chain(follow_up.iter().map(|m| format!("Follow-up: {m}")))
            .chain(std::iter::once(format!(
                "↳ {} to edit all queued messages",
                key_display_text("app.message.dequeue")
            )));
        for row in rows {
            pending.add_child(as_component(&handle(TruncatedText::new(
                t.fg("dim", &row),
                1,
                0,
            ))));
        }
        self.dirty.set(true);
    }

    /// `restoreQueuedMessagesToEditor`: every queued message, oldest first,
    /// back into the prompt ahead of what is typed; `abort` then stops the run.
    fn restore_queued_messages_to_editor(&mut self, abort: bool) -> usize {
        let (steering, follow_up) = self.session.clear_queue();
        let mut queued: Vec<String> = steering;
        let mut compaction_follow_up = Vec::new();
        for (text, behavior) in std::mem::take(&mut self.compaction_queue) {
            match behavior {
                StreamingBehavior::Steer => queued.push(text),
                StreamingBehavior::FollowUp => compaction_follow_up.push(text),
            }
        }
        queued.extend(follow_up);
        queued.extend(compaction_follow_up);
        let count = queued.len();
        if count > 0 {
            let current = self.editor.borrow().editor.get_text();
            let combined = [queued.join("\n\n"), current]
                .into_iter()
                .filter(|t| !t.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            self.editor.borrow_mut().editor.set_text(&combined);
        }
        self.update_pending_messages_display();
        if abort {
            self.session.agent().abort();
        }
        count
    }

    /// `queueCompactionMessage`.
    fn queue_compaction_message(&mut self, text: String, behavior: StreamingBehavior) {
        {
            let mut editor = self.editor.borrow_mut();
            editor.editor.add_to_history(&text);
            editor.editor.set_text("");
        }
        self.compaction_queue.push((text, behavior));
        self.update_pending_messages_display();
        self.show_status("Queued message for after compaction");
    }

    /// `flushCompactionQueue`: once a compaction ends, the first held message
    /// becomes the prompt and the rest queue behind it (all of them queue
    /// when a retry is pending).
    fn flush_compaction_queue(&mut self, will_retry: bool) {
        if self.compaction_queue.is_empty() {
            return;
        }
        let queued = std::mem::take(&mut self.compaction_queue);
        self.update_pending_messages_display();
        let queue_rest = |session: &AgentSession, rest: &[(String, StreamingBehavior)]| {
            for (text, behavior) in rest {
                let queued = match behavior {
                    StreamingBehavior::FollowUp => session.follow_up(text, &[]),
                    StreamingBehavior::Steer => session.steer(text, &[]),
                };
                queued.map_err(|e| e.to_string())?;
            }
            Ok::<(), String>(())
        };
        let failed = |error: String, count: usize| {
            format!(
                "Failed to send queued message{}: {error}",
                if count > 1 { "s" } else { "" }
            )
        };
        if will_retry {
            if let Err(error) = queue_rest(&self.session, &queued) {
                let count = queued.len();
                let _ = self.tx.send(AppEvent::CompactionQueueFailed(
                    queued,
                    failed(error, count),
                ));
            }
            self.update_pending_messages_display();
            return;
        }
        let (first, rest) = (queued[0].0.clone(), queued[1..].to_vec());
        self.prompt(first);
        if let Err(error) = queue_rest(&self.session, &rest) {
            let count = queued.len();
            let _ = self.tx.send(AppEvent::CompactionQueueFailed(
                queued,
                failed(error, count),
            ));
        }
        self.update_pending_messages_display();
    }

    /// `handleFollowUp`: alt+enter queues the prompt as a follow-up while the
    /// agent works (held during a compaction), else submits it.
    fn handle_follow_up(&mut self) {
        let text = self.editor.borrow().editor.get_text().trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.session.is_compacting() {
            self.queue_compaction_message(text, StreamingBehavior::FollowUp);
            return;
        }
        {
            let mut editor = self.editor.borrow_mut();
            editor.editor.set_text("");
        }
        if self.session.is_streaming() {
            self.editor.borrow_mut().editor.add_to_history(&text);
            self.queue_prompt(text, StreamingBehavior::FollowUp);
        } else {
            self.submit(text);
        }
    }

    /// `handleDequeue`.
    fn handle_dequeue(&mut self) {
        match self.restore_queued_messages_to_editor(false) {
            0 => self.show_status("No queued messages to restore"),
            1 => self.show_status("Restored 1 queued message to editor"),
            n => self.show_status(&format!("Restored {n} queued messages to editor")),
        }
    }

    /// `settleDanglingPlanItems`: plan rows the model left in progress settle
    /// once the request is over: done after a clean stop, else cancelled.
    /// Skipped while messages are queued (the request continues).
    fn settle_dangling_plan_items(&mut self) {
        let Some(outcome) =
            plan_settle_outcome(self.turn_stop_reason, self.session.pending_message_count())
        else {
            return;
        };
        if settle_dangling_main_tasks(task_store(), outcome) > 0 {
            self.dirty.set(true);
        }
    }

    /// `compaction_start`: the spinner with its cancel hint; Escape aborts
    /// the compaction meanwhile.
    fn on_compaction_start(&mut self, reason: CompactionReason) {
        if self.session.settings().show_terminal_progress() {
            self.tui.terminal.set_progress(true);
        }
        self.status.borrow_mut().clear();
        let cancel_hint = format!("({} to cancel)", key_text("app.interrupt"));
        let label = match reason {
            CompactionReason::Manual => format!("Compacting context... {cancel_hint}"),
            CompactionReason::Overflow => {
                format!("Context overflow detected, Auto-compacting... {cancel_hint}")
            }
            CompactionReason::Threshold => format!("Auto-compacting... {cancel_hint}"),
        };
        let mut loader = Loader::new(
            Box::new(|s: &str| theme().fg("accent", s)),
            Box::new(|s: &str| theme().fg("muted", s)),
            &label,
            None,
        );
        loader.start();
        let loader = handle(loader);
        self.status.borrow_mut().add_child(as_component(&loader));
        self.compaction_loader = Some(loader);
    }

    /// `compaction_end`: the chat rebuilt around the summary, or why not.
    fn on_compaction_end(
        &mut self,
        reason: CompactionReason,
        result: Option<CompactionResult>,
        aborted: bool,
        error_message: Option<String>,
    ) {
        if self.session.settings().show_terminal_progress() {
            self.tui.terminal.set_progress(false);
        }
        if let Some(loader) = self.compaction_loader.take() {
            loader.borrow_mut().stop();
            self.status.borrow_mut().clear();
        }
        let manual = reason == CompactionReason::Manual;
        if aborted {
            if manual {
                self.show_error("Compaction cancelled");
            } else {
                self.show_status("Auto-compaction cancelled");
            }
        } else if let Some(result) = result {
            self.reset_transcript_view();
            let messages = self.session.session_manager().build_context().messages;
            self.render_session_context(&messages, false);
            let summary = AgentMessage::CompactionSummary(CompactionSummaryMessage {
                summary: result.summary,
                tokens_before: result.tokens_before,
                tokens_after: result.tokens_after,
                timestamp: cortexcode_ai_types::now_ms(),
            });
            self.add_message_to_chat(&summary, false);
            self.footer.borrow_mut().invalidate();
        } else if let Some(error) = error_message {
            if manual {
                self.show_error(&error);
            } else {
                self.add_to_chat(as_component(&handle(Spacer::new(1))));
                self.add_to_chat(as_component(&handle(Text::new(
                    theme().fg("error", &error),
                    1,
                    0,
                ))));
            }
        }
    }

    fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::AgentStart => {
                self.tips.on_turn_start();
                self.stop_working_loader();
                let loader = self.create_working_loader();
                self.status.borrow_mut().add_child(as_component(&loader));
                self.loader = Some(loader);
            }
            AgentEvent::MessageStart { message } => match &message {
                AgentMessage::User(_) => {
                    // A new turn drops the finished tasks of the last one.
                    task_store().reset();
                    startup_progress::clear();
                    self.turn_stop_reason = None;
                    self.add_message_to_chat(&message, false);
                    self.update_pending_messages_display();
                }
                AgentMessage::Custom(_) => self.add_message_to_chat(&message, false),
                AgentMessage::Assistant(assistant) => {
                    let component = handle(AssistantMessageComponent::with_theme(
                        None,
                        self.thinking_display(),
                        self.markdown_theme(),
                        DEFAULT_HIDDEN_THINKING_LABEL,
                    ));
                    self.add_to_chat(as_component(&component));
                    component.borrow_mut().update_content(assistant, false);
                    self.assistant_components.push(component.clone());
                    self.chain_closed_for_current_message = false;
                    self.streaming = Some(component);
                    self.streaming_message = Some(assistant.clone());
                }
                _ => {}
            },
            AgentEvent::MessageUpdate { message, .. } => {
                if let (Some(_), AgentMessage::Assistant(assistant)) = (&self.streaming, message) {
                    self.schedule_streaming_render();
                    // Whatever this message first puts on screen ends the run
                    // its previous calls formed.
                    if !self.chain_closed_for_current_message && self.opens_new_chain(&assistant) {
                        self.chain_closed_for_current_message = true;
                        self.close_open_chain(ChainState::Done);
                    }
                    for content in &assistant.content {
                        let Content::ToolCall(call) = content else {
                            continue;
                        };
                        match self.pending_tools.get(&call.id) {
                            Some(block) => block.borrow_mut().update_args(call.arguments.clone()),
                            None => {
                                let block = self.new_tool_block(
                                    &call.name,
                                    &call.id,
                                    call.arguments.clone(),
                                );
                                self.attach_tool_block(block.clone());
                                self.pending_tools.insert(call.id.clone(), block);
                            }
                        }
                    }
                    self.streaming_message = Some(assistant);
                }
            }
            AgentEvent::MessageEnd { message } => {
                let AgentMessage::Assistant(mut assistant) = message else {
                    return;
                };
                self.turn_stop_reason = Some(assistant.stop_reason);
                if let Some(component) = self.streaming.take() {
                    if assistant.stop_reason == StopReason::Aborted {
                        let attempt = self.session.retry_attempt();
                        assistant.error_message = Some(if attempt > 0 {
                            format!(
                                "Aborted after {attempt} retry attempt{}",
                                if attempt > 1 { "s" } else { "" }
                            )
                        } else {
                            "Operation aborted".to_string()
                        });
                    }
                    component.borrow_mut().update_content(&assistant, false);
                    if matches!(
                        assistant.stop_reason,
                        StopReason::Aborted | StopReason::Error
                    ) {
                        let error = assistant
                            .error_message
                            .clone()
                            .filter(|m| !m.is_empty())
                            .unwrap_or_else(|| "Error".into());
                        for block in self.pending_tools.values() {
                            block.borrow_mut().update_result(
                                ToolResult {
                                    content: vec![Content::text(error.clone())],
                                    details: serde_json::Value::Null,
                                    is_error: true,
                                },
                                false,
                            );
                        }
                        self.pending_tools.clear();
                    } else {
                        // Args are complete: edit blocks compute their diffs.
                        for block in self.pending_tools.values() {
                            block.borrow_mut().set_args_complete();
                        }
                    }
                    self.streaming_message = None;
                    self.stream_render_pending = false;
                }
            }
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                let block = match self.pending_tools.get(&tool_call_id) {
                    Some(block) => block.clone(),
                    None => {
                        let block = self.new_tool_block(&tool_name, &tool_call_id, args);
                        self.attach_tool_block(block.clone());
                        self.pending_tools.insert(tool_call_id, block.clone());
                        block
                    }
                };
                block.borrow_mut().mark_execution_started();
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } => {
                if let Some(block) = self.pending_tools.get(&tool_call_id) {
                    block.borrow_mut().update_result(
                        ToolResult {
                            content: partial_result.content,
                            details: partial_result.details,
                            is_error: false,
                        },
                        true,
                    );
                }
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                result,
                is_error,
                ..
            } => {
                if let Some(block) = self.pending_tools.remove(&tool_call_id) {
                    block.borrow_mut().update_result(
                        ToolResult {
                            content: result.content,
                            details: result.details,
                            is_error,
                        },
                        false,
                    );
                    self.trim_transcript_memory();
                }
            }
            AgentEvent::AgentEnd { .. } => {
                self.pending_tools.clear();
                self.stop_working_loader();
                if let Some(component) = self.streaming.take() {
                    let handle = as_component(&component);
                    self.chat.borrow_mut().remove_child(&handle);
                    self.streaming_message = None;
                }
            }
            _ => {}
        }
    }

    fn handle_action(&mut self, action: Action) {
        match action {
            Action::Submit(text) => self.submit(text),
            Action::Interrupt => {
                if self.tree_navigation.is_some() {
                    // Escape cancels a branch summary while it runs.
                    self.session.abort_branch_summary();
                } else if self.compaction_loader.is_some() {
                    self.session.abort_compaction();
                } else if self.session.is_streaming() {
                    // Queued messages come back to the prompt, then the run stops.
                    self.restore_queued_messages_to_editor(true);
                } else if self.session.is_bash_running() {
                    self.session.abort_bash();
                } else if self.is_bash_mode {
                    self.editor.borrow_mut().editor.set_text("");
                    self.leave_bash_mode();
                } else if self.editor.borrow().editor.get_text().trim().is_empty() {
                    self.handle_double_escape();
                }
            }
            Action::Clear => {
                let now = Instant::now();
                if self
                    .last_sigint
                    .is_some_and(|at| now.duration_since(at) < Duration::from_millis(500))
                {
                    self.exit_requested = true;
                } else {
                    self.editor.borrow_mut().editor.set_text("");
                    self.last_sigint = Some(now);
                    self.dirty.set(true);
                }
            }
            Action::Exit => self.exit_requested = true,
            Action::Suspend => {}
            Action::ToolsExpand => self.jump_to_full_view(),
            Action::ViewForward | Action::ViewBackward => {
                self.cycle_tool_output_view(action == Action::ViewForward)
            }
            Action::ChromeForward | Action::ChromeBackward => {
                let density = self.chrome.cycle_density(action == Action::ChromeForward);
                let mut settings = self.session.settings();
                settings.set_chrome_density(density);
                drop(settings);
                self.show_dial_step("app.chrome.cycleBackward", &format!("Chrome: {density}"));
            }
            Action::ThinkingForward | Action::ThinkingBackward => {
                let direction = if action == Action::ThinkingForward {
                    cortexcode_code_agent_session::CycleDirection::Forward
                } else {
                    cortexcode_code_agent_session::CycleDirection::Backward
                };
                match self.session.cycle_thinking_level(direction) {
                    None => self.show_status("Current model does not support thinking"),
                    Some(level) => {
                        self.update_editor_border_color();
                        self.show_dial_step(
                            "app.thinking.cycleBackward",
                            &format!("Thinking level: {}", level.as_str()),
                        );
                    }
                }
            }
            Action::ModelForward | Action::ModelBackward => {
                self.cycle_model(action == Action::ModelForward)
            }
            Action::ModelSelect => self.show_model_selector(None),
            Action::SettingsOpen => self.show_settings_selector(),
            Action::SelectorDone(outcome) => self.close_selector(outcome),
            Action::ResumeSession => self.show_session_selector(),
            Action::EditorChanged(text) => self.on_editor_change(&text),
            Action::CopyMessage => self.handle_copy_command(""),
            Action::PasteImage => self.handle_clipboard_image_paste(),
            Action::HotkeysOpen => self.handle_hotkeys_command(),
            Action::SessionColorForward | Action::SessionColorBackward => {
                self.cycle_session_color(action == Action::SessionColorForward)
            }
            Action::SessionColorPreview(slot) => self.preview_session_color(slot),
            Action::SessionColorDone(slot) => self.close_session_color_selector(slot),
            Action::ChangeDirectoryPrefill => {
                // Prefill rather than act: the editor's path completion is the
                // way to name the target.
                self.editor.borrow_mut().editor.set_text("/cd ");
                self.dirty.set(true);
            }
            Action::ForkOpen => self.show_user_message_selector(),
            Action::FollowUp => self.handle_follow_up(),
            Action::Dequeue => self.handle_dequeue(),
            Action::TasksForward | Action::TasksBackward => {
                let forward = action == Action::TasksForward;
                let view = self.task_panel.borrow_mut().cycle_view(forward);
                let label = match view {
                    TaskPanelView::Flat => "tasks",
                    TaskPanelView::Subagents => "subagents",
                    TaskPanelView::Teams => "teams",
                };
                self.show_dial_step(
                    if forward {
                        "app.tasks.cycleBackward"
                    } else {
                        "app.tasks.cycleForward"
                    },
                    &format!("Task panel: {label}"),
                );
            }
            Action::AskOptionsDone(answers) => self.hide_ask_options(answers),
            Action::EditorDialogDone(outcome) => self.close_editor_dialog(outcome),
            Action::SessionSelectorDone(path) => {
                self.close_session_selector();
                if let Some(path) = path {
                    self.handle_resume_session(path, None);
                }
            }
            Action::AutocompleteVisibility(visible) => {
                if self.chrome.set_autocomplete_open(visible) {
                    self.dirty.set(true);
                }
            }
        }
    }

    /// The next time something is due without input.
    fn next_wakeup(&self) -> Duration {
        let now = Instant::now();
        let mut wait = Duration::from_millis(250);
        let deadlines = [
            self.notifications.borrow().deadline(),
            self.tips.deadline(),
            self.editor.borrow().editor.autocomplete_deadline(),
        ];
        for deadline in deadlines.into_iter().flatten() {
            wait = wait.min(deadline.saturating_duration_since(now));
        }
        if let Some(loader) = &self.loader {
            wait = wait.min(loader.borrow().interval());
        }
        if let Some(deadline) = self
            .selector
            .as_ref()
            .and_then(|(s, _)| s.borrow().deadline())
        {
            wait = wait.min(deadline.saturating_duration_since(now));
        }
        if let Some(deadline) = self
            .fork_selector
            .as_ref()
            .and_then(|s| s.borrow().deadline())
        {
            wait = wait.min(deadline.saturating_duration_since(now));
        }
        if let Some(deadline) = self
            .tree_selector
            .as_ref()
            .and_then(|(s, _)| s.borrow().deadline())
        {
            wait = wait.min(deadline.saturating_duration_since(now));
        }
        if self.stream_render_pending {
            if let Some(at) = self.stream_render_at {
                wait = wait.min((at + STREAM_RENDER_THROTTLE).saturating_duration_since(now));
            }
        }
        wait.max(Duration::from_millis(1))
    }

    fn run(
        mut self,
        options_initial: (
            Option<String>,
            Vec<ImageContent>,
            Vec<String>,
            Option<String>,
        ),
    ) -> Result<(), String> {
        let input = self.tui.start();
        // From here the TUI owns the terminal: the agent's operational log
        // lines (dispatch, warm fallback, lifeguard) must not write to it.
        set_terminal_owned_by_tui(true);
        let tx = self.tx.clone();
        self.task_store_subscription = Some(task_store().subscribe(move || {
            let _ = tx.send(AppEvent::Rerender);
        }));
        self.subscription = Some(self.subscribe());

        // Everything the chrome shows about the session.
        self.update_editor_border_color();
        self.update_session_chip();
        self.update_terminal_title();
        let theme_name = self.session.settings().theme();
        if let Some(name) = theme_name {
            if let Err(error) = set_theme(&name, true) {
                self.show_error(&format!(
                    "Failed to load theme \"{name}\": {error}\nFell back to dark theme."
                ));
            }
        }
        let changelog = {
            let has_messages = !self.session.messages().is_empty();
            let mut settings = self.session.settings();
            changelog_for_display(has_messages, &mut settings, VERSION)
        };
        self.render_resources();
        self.show_startup_notices(changelog);
        // Messages after the resource listing, as the pin orders them.
        self.render_initial_messages();
        self.setup_autocomplete_provider();
        self.update_available_provider_count();
        self.maybe_warn_about_anthropic_subscription_auth(None);

        let tx = Mutex::new(self.tx.clone());
        set_dialog_sink(Some(Box::new(move |request| {
            tx.lock()
                .unwrap_or_else(|e| e.into_inner())
                .send(AppEvent::Dialog(request))
                .is_ok()
        })));
        let tx = self.tx.clone();
        on_theme_change(move || {
            let _ = tx.send(AppEvent::ThemeChanged);
        });
        let tx = self.tx.clone();
        let progress = startup_progress::subscribe(move || {
            let _ = tx.send(AppEvent::Rerender);
        });
        let tx = self.tx.clone();
        let branch = self.footer_data.on_branch_change(move || {
            let _ = tx.send(AppEvent::Rerender);
        });

        let (initial_message, initial_images, initial_messages, fallback) = options_initial;
        if let Some(message) = fallback {
            self.show_error(&message);
        }
        if let Some(model_error) = self.session.model_registry().error() {
            self.show_error(&format!("models.json error: {model_error}"));
        }
        let has_initial_message = initial_message.is_some();
        let mut queued: Vec<String> = initial_message
            .into_iter()
            .chain(initial_messages)
            .collect();
        queued.reverse();
        if let Some(first) = queued.pop() {
            // The @file images go with the initial message only.
            let images = if has_initial_message {
                initial_images
            } else {
                Vec::new()
            };
            self.prompt_with_images(first, images);
        }

        self.tui.request_render(false);
        self.dirty.set(false);
        loop {
            match input.recv_timeout(self.next_wakeup()) {
                Ok(event) => {
                    // A keystroke that turns out to do nothing is still the
                    // user being present: it restarts the tips' idle clock.
                    if let TuiEvent::Input(_) = event {
                        self.tips.on_activity();
                    }
                    if let TuiEvent::Resize = event {
                        let terminal = &self.tui.terminal;
                        self.size.set((terminal.columns(), terminal.rows()));
                    }
                    self.tui.process_event(event);
                    while let Ok(event) = input.try_recv() {
                        if let TuiEvent::Input(_) = event {
                            self.tips.on_activity();
                        }
                        self.tui.process_event(event);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            loop {
                let pending: Vec<Action> = self.actions.borrow_mut().drain(..).collect();
                if pending.is_empty() {
                    break;
                }
                for action in pending {
                    self.handle_action(action);
                }
            }
            while let Ok(event) = self.rx.try_recv() {
                match event {
                    AppEvent::Session(event, anchor) => {
                        if self.turn_cost_anchor.is_none() {
                            self.turn_cost_anchor = anchor;
                        }
                        self.handle_session_event(*event)
                    }
                    AppEvent::PromptDone(result) => {
                        // The request has ended (retries and continuations
                        // included): settle it (`settleRequestOnIdle`). The
                        // last chain has nothing after it to close it.
                        let outcome = if self.turn_stop_reason == Some(StopReason::Stop) {
                            ChainState::Done
                        } else {
                            ChainState::Interrupted
                        };
                        self.close_open_chain(outcome);
                        self.show_turn_cost();
                        self.settle_dangling_plan_items();
                        self.tips.on_turn_end();
                        if let Err(error) = result {
                            self.show_error(&error);
                        }
                        if let Some(next) = queued.pop() {
                            self.prompt(next);
                        }
                    }
                    AppEvent::Rerender => self.dirty.set(true),
                    AppEvent::Dialog(DialogRequest::Select {
                        title,
                        options,
                        reply,
                    }) => self.show_selector(&title, options, reply),
                    AppEvent::Dialog(DialogRequest::Notify(message)) => self.show_record(&message),
                    AppEvent::Dialog(DialogRequest::AskOptions { questions, reply }) => {
                        self.show_ask_options(questions, reply)
                    }
                    AppEvent::Dialog(DialogRequest::HideAskOptions) => self.hide_ask_options(None),
                    AppEvent::Login(update) => self.handle_login_update(update),
                    AppEvent::BashChunk(chunk) => {
                        if let Some(component) = &self.bash_component {
                            component.borrow_mut().append_output(&chunk);
                            self.dirty.set(true);
                        }
                    }
                    AppEvent::BashDone(result) => self.finish_bash_command(result),
                    AppEvent::CopyDone(subject, result) => self.finish_copy(&subject, result),
                    AppEvent::PastedImage(image) => self.insert_pasted_image(image),
                    AppEvent::SubagentDone(mode, result) => self.finish_subagent(&mode, result),
                    AppEvent::CompactionQueueFailed(queued, error) => {
                        self.session.clear_queue();
                        self.compaction_queue = queued;
                        self.update_pending_messages_display();
                        self.show_error(&error);
                    }
                    AppEvent::QueueError(error) => self.show_error(&error),
                    AppEvent::ThemeChanged => {
                        self.tui.invalidate();
                        self.update_editor_border_color();
                        self.update_session_chip();
                    }
                }
            }
            if self.notifications.borrow_mut().poll() {
                self.dirty.set(true);
            }
            if self.tips.poll() {
                self.dirty.set(true);
            }
            if self
                .selector
                .as_ref()
                .is_some_and(|(s, _)| s.borrow_mut().poll())
            {
                self.dirty.set(true);
            }
            if self.last_tool_tick.elapsed() >= Duration::from_secs(1) {
                self.last_tool_tick = Instant::now();
                if self.task_panel.borrow().ticking() {
                    self.dirty.set(true);
                }
                for block in self.pending_tools.values() {
                    if block.borrow().is_ticking() {
                        block.borrow_mut().invalidate();
                        self.dirty.set(true);
                    }
                }
            }
            if self
                .session_selector
                .as_ref()
                .is_some_and(|s| s.borrow_mut().poll())
            {
                self.dirty.set(true);
            }
            self.poll_cwd_prompt();
            self.poll_import_confirm();
            self.poll_tree_selector();
            self.poll_fork_selector();
            self.poll_model_selectors();
            self.poll_login();
            self.poll_settings_selector();
            self.poll_tree_summary();
            self.poll_tree_instructions();
            self.poll_tree_navigation();
            if self.loader.as_ref().is_some_and(|l| l.borrow_mut().tick()) {
                self.dirty.set(true);
            }
            if self
                .compaction_loader
                .as_ref()
                .is_some_and(|l| l.borrow_mut().tick())
            {
                self.dirty.set(true);
            }
            if self
                .bash_component
                .as_ref()
                .is_some_and(|c| c.borrow_mut().tick())
            {
                self.dirty.set(true);
            }
            if self.stream_render_pending
                && self
                    .stream_render_at
                    .is_none_or(|at| at.elapsed() >= STREAM_RENDER_THROTTLE)
            {
                self.run_streaming_render();
            }
            if self.editor.borrow_mut().editor.poll_autocomplete() {
                self.dirty.set(true);
            }
            if self.exit_requested {
                break;
            }
            // Input already re-rendered; anything else that changed renders now.
            if self.dirty.replace(false) {
                self.tui.request_render(false);
            }
        }

        set_dialog_sink(None);
        if let Some((_, reply)) = self.selector.take() {
            let _ = reply.send(None);
        }
        if let Some((_, reply)) = self.ask_options.take() {
            let _ = reply.send(None);
        }
        if let Some((_, reply)) = self.editor_dialog.take() {
            let _ = reply.send(None);
        }
        progress.unsubscribe();
        self.footer_data.off_branch_change(branch);
        self.footer_data.dispose();
        cortexcode_code_tui_theme::stop_theme_watcher();
        self.tips.stop();
        self.notifications.borrow_mut().stop();
        if let Some(subscription) = self.task_store_subscription.take() {
            subscription.unsubscribe();
        }
        self.task_panel.borrow_mut().dispose();
        self.tui.stop();
        set_terminal_owned_by_tui(false);
        let session = self.session.clone();
        if session.is_streaming() {
            self.runtime.block_on(async move { session.abort().await });
        }
        self.session.dispose();
        Ok(())
    }
}

/// The built-in slash commands this mode dispatches itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltinCommand {
    Quit,
    Resume,
    Tree,
    Name,
    Session,
    New,
    Compact,
    Model,
    ScopedModels,
    Settings,
    Login,
    Logout,
    Copy,
    Hotkeys,
    Changelog,
    Debug,
    Color,
    Chrome,
    Fork,
    Clone,
    Cd,
    Reload,
    Export,
    Import,
    Subagent,
    /// A built-in whose handler lands with a later task.
    Pending(&'static str),
}

impl BuiltinCommand {
    fn lookup(text: &str) -> Option<Self> {
        let name = text.strip_prefix('/')?;
        Some(match name {
            "quit" => Self::Quit,
            "resume" => Self::Resume,
            "tree" => Self::Tree,
            "name" => Self::Name,
            "session" => Self::Session,
            "new" => Self::New,
            "compact" => Self::Compact,
            "model" => Self::Model,
            "scoped-models" => Self::ScopedModels,
            "settings" => Self::Settings,
            "login" => Self::Login,
            "logout" => Self::Logout,
            "copy" => Self::Copy,
            "hotkeys" => Self::Hotkeys,
            "changelog" => Self::Changelog,
            "debug" => Self::Debug,
            "color" => Self::Color,
            "chrome" => Self::Chrome,
            "fork" => Self::Fork,
            "clone" => Self::Clone,
            "cd" => Self::Cd,
            "reload" => Self::Reload,
            "export" => Self::Export,
            "import" => Self::Import,
            "subagent" => Self::Subagent,
            _ => {
                let builtin = BUILTIN_SLASH_COMMANDS.iter().find(|c| c.name == name)?;
                Self::Pending(builtin.name)
            }
        })
    }

    /// Commands that also match "/name <args>".
    fn with_args(self) -> bool {
        match self {
            Self::Name
            | Self::Compact
            | Self::Model
            | Self::Copy
            | Self::Color
            | Self::Chrome
            | Self::Cd
            | Self::Export
            | Self::Import
            | Self::Subagent => true,
            Self::Pending(_) => false,
            _ => false,
        }
    }
}

/// `resolveVoiceSilenceMs`: `VOICETOOLS_SILENCE_MS` (clamped to
/// 300-10000) wins over the setting.
fn resolve_voice_silence_ms(setting: u64) -> u64 {
    std::env::var("VOICETOOLS_SILENCE_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .map(|v| (v.floor() as u64).clamp(300, 10000))
        .unwrap_or(setting)
}

/// A transport's settings.json name.
fn transport_name(transport: Transport) -> String {
    serde_json::to_value(transport)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "auto".into())
}

/// `toLocaleString()` for a count: grouped with commas.
fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `getAutocompleteSourceTag`: u/p/t, plus the package for npm and git.
fn autocomplete_source_tag(source_info: &serde_json::Value) -> Option<String> {
    let scope = source_info.get("scope")?.as_str()?;
    let prefix = match scope {
        "user" => "u",
        "project" => "p",
        _ => "t",
    };
    let source = source_info
        .get("source")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim();
    if matches!(source, "auto" | "local" | "cli") {
        return Some(prefix.to_string());
    }
    if source.starts_with("npm:") {
        return Some(format!("{prefix}:{source}"));
    }
    if let Some(git) = cortexcode_code_paths::git::parse_git_url(source) {
        let git_ref = git.git_ref.map(|r| format!("@{r}")).unwrap_or_default();
        return Some(format!("{prefix}:git:{}/{}{git_ref}", git.host, git.path));
    }
    Some(prefix.to_string())
}

/// `prefixAutocompleteDescription`.
fn prefix_autocomplete_description(
    description: Option<String>,
    source_info: &serde_json::Value,
) -> Option<String> {
    let Some(tag) = autocomplete_source_tag(source_info) else {
        return description;
    };
    Some(match description {
        Some(d) if !d.is_empty() => format!("[{tag}] {d}"),
        _ => format!("[{tag}]"),
    })
}

/// What dangling plan rows settle to when a request ends: done after a clean
/// stop, cancelled after an abort, error or length stop; nothing while
/// messages are queued (the request continues).
pub fn plan_settle_outcome(
    stop_reason: Option<StopReason>,
    pending_messages: usize,
) -> Option<TaskStatus> {
    if pending_messages > 0 {
        return None;
    }
    Some(if stop_reason == Some(StopReason::Stop) {
        TaskStatus::Done
    } else {
        TaskStatus::Cancelled
    })
}

/// `InteractiveMode.run`: until the user exits.
pub fn run_interactive(options: InteractiveOptions) -> Result<(), String> {
    let initial = (
        options.initial_message.clone(),
        options.initial_images.clone(),
        options.initial_messages.clone(),
        options.model_fallback_message.clone(),
    );
    let mode = Mode::new(options);
    mode.run(initial)
}
