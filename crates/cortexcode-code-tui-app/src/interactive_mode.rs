//! The interactive mode (`interactive-mode.ts`): builds the TUI tree —
//! banner, the rows nobody uses, transcript, notification band, prompt,
//! footer — wires the keys, and runs the submit loop against the session.
//!
//! What is here is the shell and its idle screen. The transcript widgets
//! (assistant markdown, tool blocks, the working loader) arrive in 11.2; until
//! then a turn is shown as the user's text and the agent's final text.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use cortexcode_agent_types::{AgentEvent, AgentMessage};
use cortexcode_ai_types::{AssistantMessage, Content, StopReason};
use cortexcode_code_agent_session::format::{format_duration_secs, format_tokens};
use cortexcode_code_agent_session::stats::{sum_assistant_usage, AssistantUsageTotals};
use cortexcode_code_agent_session::{AgentSession, AgentSessionEvent, PromptOptions};
use cortexcode_code_paths::{APP_NAME, APP_TITLE};
use cortexcode_code_settings::{ChromeDensity, EditorBorder, ToolOutputView};
use cortexcode_code_tui_keybindings::{
    app_key_label, key_hint, key_text, raw_key_hint, AppKeybindingsManager,
};
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
use cortexcode_tui_components::{
    Editor, EditorHost, EditorOptions, FrameBorderStyle, Loader, MarkdownTheme, Spacer, Text,
};
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::{
    Component, ComponentHandle, Container, FlexSpacer, Slot, Tui, TuiEvent,
};
use cortexcode_tui_terminal::Terminal;

use crate::chrome_layout::{
    ChromeLayoutController, ChromeSurfaces, FooterLayout, SMALL_TERMINAL_ROWS,
};
use crate::expandable_text::{Expandable, ExpandableText};
use crate::footer::{FooterComponent, FooterDensity, FooterModel, FooterSource};
use crate::footer_data::FooterDataProvider;
use crate::input_frame::set_input_frame_border;
use crate::notification_panel::{NotificationKind, NotificationPanel};
use crate::resource_display::{format_display_path, show_loaded_resources, ResourceListing};
use crate::session_chip::render_session_chip;
use crate::startup_progress;
use crate::wordmark::{build_compact_wordmark, CompactWordmarkOptions};

/// How the mode is started.
pub struct InteractiveOptions {
    pub session: AgentSession,
    /// Where agent turns run.
    pub runtime: tokio::runtime::Handle,
    /// The startup resource listing, read when it is drawn.
    pub listing: Box<dyn Fn() -> ResourceListing>,
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
    ViewForward,
    ViewBackward,
    /// The editor submitted this text (it has already cleared itself).
    Submit(String),
    AutocompleteVisibility(bool),
}

/// Bindings the prompt answers to, and their action (`CustomEditor.onAction`).
const EDITOR_ACTIONS: [(&str, Action); 8] = [
    ("app.view.cycleForward", Action::ViewForward),
    ("app.view.cycleBackward", Action::ViewBackward),
    ("app.clear", Action::Clear),
    ("app.suspend", Action::Suspend),
    ("app.tools.expand", Action::ToolsExpand),
    ("app.chrome.cycleForward", Action::ChromeForward),
    ("app.chrome.cycleBackward", Action::ChromeBackward),
    ("app.thinking.cycleForward", Action::ThinkingForward),
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
    show_images: bool,
    image_width_cells: u32,
    code_block_indent: String,
    editor: Rc<RefCell<CustomEditor>>,
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
    listing: Box<dyn Fn() -> ResourceListing>,
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
                is_oauth,
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
            show_images,
            image_width_cells,
            code_block_indent,
            editor,
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
        let mut listing = (self.listing)();
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
    fn add_message_to_chat(&mut self, message: &AgentMessage) {
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

    /// `showStatus`: a dim line in the chat; back-to-back statuses update the
    /// previous line instead of stacking.
    fn show_status(&mut self, message: &str) {
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
                    self.add_message_to_chat(&message);
                }
                AgentMessage::Custom(_) => self.add_message_to_chat(&message),
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
                if self.session.is_streaming() {
                    let session = self.session.clone();
                    self.runtime.spawn(async move { session.abort().await });
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
        let _subscription = self.subscribe();

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
