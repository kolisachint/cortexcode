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
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cortexcode_agent_types::{AgentEvent, AgentMessage};
use cortexcode_ai_types::{AssistantMessage, Content, StopReason};
use cortexcode_ai_types::{Model, ThinkingLevel};
use cortexcode_code_agent_session::format::{format_duration_secs, format_tokens};
use cortexcode_code_agent_session::runtime::{format_missing_session_cwd_prompt, RuntimeError};
use cortexcode_code_agent_session::stats::{sum_assistant_usage, AssistantUsageTotals};
use cortexcode_code_agent_session::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, NavigateTreeOptions, NavigateTreeResult,
    NewSessionRequest, PromptOptions,
};
use cortexcode_code_models::{find_exact_model_reference_match, resolve_model_scope};
use cortexcode_code_paths::{APP_NAME, APP_TITLE};
use cortexcode_code_resources::BUILTIN_SLASH_COMMANDS;
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::{ChromeDensity, DoubleEscapeAction, EditorBorder, ToolOutputView};
use cortexcode_code_tools_optin::AskQuestion;
use cortexcode_code_tui_keybindings::{
    app_key_label, key_hint, key_text, raw_key_hint, AppKeybindingsManager,
};
use cortexcode_code_tui_selectors::ask_options::{AskOptionsComponent, AskOptionsOptions};
use cortexcode_code_tui_selectors::model_selector::{ModelSelectorComponent, ModelSelectorEvent};
use cortexcode_code_tui_selectors::scoped_models_selector::{
    ScopedModelsEvent, ScopedModelsSelectorComponent,
};
use cortexcode_code_tui_selectors::session_selector::{
    SessionSelectorComponent, SessionSelectorOptions,
};
use cortexcode_code_tui_selectors::tree_selector::{TreeEvent, TreeSelectorComponent};
use cortexcode_code_tui_theme::{apply_block_fill, BlockFill};
use cortexcode_code_tui_theme::{
    get_editor_theme, get_markdown_theme, init_theme, on_theme_change, set_registered_themes,
    set_theme, theme, ThinkingBorderLevel,
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
use cortexcode_tui_components::{
    CombinedAutocompleteProvider, CommandEntry, Editor, EditorHost, EditorOptions,
    FrameBorderStyle, Loader, MarkdownTheme, SlashCommand, Spacer, Text,
};
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::{
    Component, ComponentHandle, Container, FlexSpacer, Slot, Tui, TuiEvent,
};
use cortexcode_tui_terminal::Terminal;

use crate::chrome_layout::{
    ChromeLayoutController, ChromeSurfaces, FooterLayout, SMALL_TERMINAL_ROWS,
};
use crate::dialog_bridge::{set_dialog_sink, DialogRequest};
use crate::expandable_text::{Expandable, ExpandableText};
use crate::extension_editor::{EditorOutcome, ExtensionEditorComponent};
use crate::extension_selector::{ExtensionSelectorComponent, SelectorOutcome};
use crate::footer::{FooterComponent, FooterDensity, FooterModel, FooterSource};
use crate::footer_data::FooterDataProvider;
use crate::input_frame::set_input_frame_border;
use crate::notification_panel::{NotificationKind, NotificationPanel};
use crate::resource_display::{format_display_path, show_loaded_resources, ResourceListing};
use crate::session_chip::render_session_chip;
use crate::session_picker;
use crate::startup_progress;
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
    pub version: String,
    /// Force the verbose startup banner.
    pub verbose: bool,
    pub initial_message: Option<String>,
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
const EDITOR_ACTIONS: [(&str, Action); 12] = [
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
        let tasks_slot = handle(Slot::new(as_component(&handle(Container::new()))));
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
                set_tasks_density: Box::new(|_| {}),
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
        tui.add_child(as_component(&handle(Container::new()))); // pending messages
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
            session_selector: None,
            pending_cwd_prompt: None,
            ask_options: None,
            tree_selector: None,
            model_selector: None,
            scoped_models_selector: None,
            anthropic_warning_shown: false,
            last_escape: None,
            pending_tree_summary: None,
            pending_tree_instructions: None,
            tree_navigation: None,
            editor_dialog: None,
        }
    }

    fn update_editor_border_color(&mut self) {
        let level = thinking_border_level(self.session.thinking_level().as_str());
        self.editor.borrow_mut().editor.border_color =
            Box::new(move |s: &str| theme().thinking_border(level, s));
        self.dirty.set(true);
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

    fn show_error(&mut self, message: &str) {
        self.add_to_chat(as_component(&handle(Spacer::new(1))));
        self.add_to_chat(as_component(&handle(Text::new(
            theme().fg("error", message),
            1,
            0,
        ))));
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
        startup_progress::clear();
        let session = self.session.clone();
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = session
                .prompt(
                    &text,
                    PromptOptions {
                        expand_prompt_templates: true,
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
        let t = theme();
        let mut block = BoxComponent::new(1, 1, None);
        apply_block_fill(&mut block, BlockFill::WarningBg);
        block.add_child(as_component(&handle(Text::new(
            t.bold(&t.fg("warning", title)),
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
    /// built-in commands, then prompt templates, then skill commands.
    fn setup_autocomplete_provider(&mut self) {
        let mut commands: Vec<CommandEntry> = BUILTIN_SLASH_COMMANDS
            .iter()
            .map(|c| {
                CommandEntry::Slash(SlashCommand {
                    name: c.name.to_string(),
                    description: Some(c.description.to_string()),
                    argument_hint: (c.name == "cd").then(|| "<path>".to_string()),
                    get_argument_completions: None,
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
        let provider =
            CombinedAutocompleteProvider::new(commands, self.session.cwd().to_path_buf(), None);
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
            BuiltinCommand::Pending(name) => {
                // Wired by its own ledger task (see 11.3b/11.3d2/11.3f2/11.4d/11.4e).
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
        let mut lines = message.split('\n');
        let title = lines.next().unwrap_or("");
        let body: Vec<&str> = lines.collect();
        self.notifications.borrow_mut().notify(
            NotificationKind::Info,
            title,
            &body,
            None,
            None,
            None,
        );
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
            AgentSessionEvent::SessionInfoChanged { .. } => {
                self.update_session_chip();
                self.update_terminal_title();
            }
            _ => {}
        }
        self.dirty.set(true);
    }

    fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::AgentStart => {
                self.stop_working_loader();
                let loader = self.create_working_loader();
                self.status.borrow_mut().add_child(as_component(&loader));
                self.loader = Some(loader);
            }
            AgentEvent::MessageStart { message } => match &message {
                AgentMessage::User(_) => {
                    startup_progress::clear();
                    self.turn_stop_reason = None;
                    self.add_message_to_chat(&message, false);
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
                } else if self.session.is_streaming() {
                    let session = self.session.clone();
                    self.runtime.spawn(async move { session.abort().await });
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
            Action::SelectorDone(outcome) => self.close_selector(outcome),
            Action::ResumeSession => self.show_session_selector(),
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
        options_initial: (Option<String>, Vec<String>, Option<String>),
    ) -> Result<(), String> {
        let input = self.tui.start();
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
        self.render_resources();
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

        let (initial_message, initial_messages, fallback) = options_initial;
        if let Some(message) = fallback {
            self.show_error(&message);
        }
        if let Some(model_error) = self.session.model_registry().error() {
            self.show_error(&format!("models.json error: {model_error}"));
        }
        let mut queued: Vec<String> = initial_message
            .into_iter()
            .chain(initial_messages)
            .collect();
        queued.reverse();
        if let Some(first) = queued.pop() {
            self.prompt(first);
        }

        self.tui.request_render(false);
        self.dirty.set(false);
        loop {
            match input.recv_timeout(self.next_wakeup()) {
                Ok(event) => {
                    if let TuiEvent::Resize = event {
                        let terminal = &self.tui.terminal;
                        self.size.set((terminal.columns(), terminal.rows()));
                    }
                    self.tui.process_event(event);
                    while let Ok(event) = input.try_recv() {
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
            if self
                .selector
                .as_ref()
                .is_some_and(|(s, _)| s.borrow_mut().poll())
            {
                self.dirty.set(true);
            }
            if self.last_tool_tick.elapsed() >= Duration::from_secs(1) {
                self.last_tool_tick = Instant::now();
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
            self.poll_tree_selector();
            self.poll_model_selectors();
            self.poll_tree_summary();
            self.poll_tree_instructions();
            self.poll_tree_navigation();
            if self.loader.as_ref().is_some_and(|l| l.borrow_mut().tick()) {
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
        self.notifications.borrow_mut().stop();
        self.tui.stop();
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
            _ => {
                let builtin = BUILTIN_SLASH_COMMANDS.iter().find(|c| c.name == name)?;
                Self::Pending(builtin.name)
            }
        })
    }

    /// Commands that also match "/name <args>".
    fn with_args(self) -> bool {
        match self {
            Self::Name | Self::Compact | Self::Model => true,
            Self::Pending(name) => matches!(
                name,
                "export" | "import" | "copy" | "color" | "chrome" | "cd" | "subagent"
            ),
            _ => false,
        }
    }
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

/// `InteractiveMode.run`: until the user exits.
pub fn run_interactive(options: InteractiveOptions) -> Result<(), String> {
    let initial = (
        options.initial_message.clone(),
        options.initial_messages.clone(),
        options.model_fallback_message.clone(),
    );
    let mode = Mode::new(options);
    mode.run(initial)
}
