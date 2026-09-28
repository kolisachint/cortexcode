//! The `/settings` pane, hoocode `components/settings-selector.ts`.
//!
//! One [`SettingsList`] in the prompt's frame. The top level is short: the
//! tool rows, then categories whose submenus hold the leaf settings, then the
//! external binaries. Every change goes out as one [`SettingsChange`].
//!
//! Adaptations: TypeScript shares one mutable item object between the flat
//! leaf list and the category submenu that shows it, so a value cycled in a
//! submenu is still there when the submenu is reopened. Here the leaves live
//! in one shared table ([`Leaf`]) that each category rebuilds its rows from
//! and writes changes back to. The forty `on*Change` callbacks are one
//! [`SettingsChange`] enum through one callback.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use cortexcode_code_paths::{APP_NAME, CONFIG_DIR_NAME};
use cortexcode_code_settings::{
    DoubleEscapeAction, EditorBorder, FlagValue, LearnSettingKey, LearnSettings,
    MarketplacePlatform, PluginInstallScope, QueueMode, ThinkingLevelSetting, ToolOutputView,
    TreeFilterMode, WarningSettings,
};
use cortexcode_code_tools::external_tools::{
    build_row_gates, status_label, Acquisition, ExternalToolStatus, ManagedToolSource,
};
use cortexcode_code_tools::light::PromptSurface;
use cortexcode_code_tui_keybindings::key_display_text;
use cortexcode_code_tui_theme::{
    get_select_list_theme, get_settings_list_theme, get_theme_description, style_input, theme,
};
use cortexcode_code_tui_widgets::input_frame::{InputFrame, InputFrameOptions};
use cortexcode_code_tui_widgets::tool_output_view::{
    tool_output_view_description, TOOL_OUTPUT_VIEWS,
};
use cortexcode_tui_components::{
    Input, SelectItem, SelectList, SettingItem, SettingsList, SettingsListOptions, Spacer,
    SubmenuOutcome, Text,
};
use cortexcode_tui_render::{Component, ComponentHandle, Container};

use crate::framed_list::layout;
use crate::small_selectors::thinking_level_description;

/// Where a submenu reports how it closed (`done(value?)`).
pub type DoneSlot = Rc<RefCell<Option<SubmenuOutcome>>>;

fn done(slot: &DoneSlot, value: Option<String>) {
    *slot.borrow_mut() = Some(match value {
        Some(value) => SubmenuOutcome::Selected(value),
        None => SubmenuOutcome::Cancelled,
    });
}

/// `ToolToggleInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolToggleInfo {
    pub name: String,
    /// Not in the persisted disabled set.
    pub enabled: bool,
    /// What the tool's schema costs per request, on or off. `None` for a tool
    /// disabled before launch, whose schema this session never built.
    pub tokens: Option<usize>,
}

/// `FlagInfo`: an extension-registered flag; its type follows its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagInfo {
    pub name: String,
    pub description: Option<String>,
    pub value: FlagValue,
}

/// `ToolGroupInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGroupInfo {
    pub id: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
}

/// `SettingsConfig`: what the pane shows.
pub struct SettingsConfig {
    pub auto_compact: bool,
    pub tools: Vec<ToolToggleInfo>,
    pub tool_groups: Vec<ToolGroupInfo>,
    /// The external binaries and whether each is present.
    pub external_tools: Vec<ExternalToolStatus>,
    pub flags: Vec<FlagInfo>,
    pub tool_output_view: ToolOutputView,
    pub tool_output_max_bytes: u64,
    pub tool_output_max_lines: u64,
    pub context_gc: bool,
    pub show_images: bool,
    pub image_width_cells: u64,
    pub auto_resize_images: bool,
    pub block_images: bool,
    pub enable_skill_commands: bool,
    pub light: bool,
    pub plugin_install_scope: PluginInstallScope,
    pub enable_plugin_tools: bool,
    /// settings.json keys the project file sets (they win over a pane row).
    pub project_pinned_settings: Vec<String>,
    /// Platform layouts in force (empty = unset).
    pub platform: Vec<MarketplacePlatform>,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    pub transport: String,
    pub thinking_level: String,
    pub available_thinking_levels: Vec<String>,
    pub current_theme: String,
    pub available_themes: Vec<String>,
    pub hide_thinking_block: bool,
    pub collapse_changelog: bool,
    pub enable_install_telemetry: bool,
    pub double_escape_action: DoubleEscapeAction,
    pub tree_filter_mode: TreeFilterMode,
    pub show_hardware_cursor: bool,
    pub editor_border: EditorBorder,
    pub editor_padding_x: u64,
    pub autocomplete_max_visible: u64,
    pub quiet_startup: bool,
    pub tips_enabled: bool,
    pub clear_on_shrink: bool,
    pub show_terminal_progress: bool,
    pub warnings: WarningSettings,
    pub voice_silence_ms: u64,
    pub webtools_timeout_secs: u64,
    pub learn: LearnSettings,
    /// Re-measure the fixed per-turn surface; `None` without a session.
    pub measure_token_surface: Option<Rc<dyn Fn() -> PromptSurface>>,
    /// Whether the terminal renders inline images (`getCapabilities().images`).
    pub supports_images: bool,
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            auto_compact: true,
            tools: Vec::new(),
            tool_groups: Vec::new(),
            external_tools: Vec::new(),
            flags: Vec::new(),
            tool_output_view: ToolOutputView::Peek,
            tool_output_max_bytes: 8192,
            tool_output_max_lines: 200,
            context_gc: true,
            show_images: false,
            image_width_cells: 80,
            auto_resize_images: true,
            block_images: false,
            enable_skill_commands: true,
            light: false,
            plugin_install_scope: PluginInstallScope::User,
            enable_plugin_tools: false,
            project_pinned_settings: Vec::new(),
            platform: Vec::new(),
            steering_mode: QueueMode::All,
            follow_up_mode: QueueMode::All,
            transport: "auto".into(),
            thinking_level: "off".into(),
            available_thinking_levels: vec!["off".into()],
            current_theme: "dark".into(),
            available_themes: vec!["dark".into()],
            hide_thinking_block: false,
            collapse_changelog: false,
            enable_install_telemetry: false,
            double_escape_action: DoubleEscapeAction::Tree,
            tree_filter_mode: TreeFilterMode::Default,
            show_hardware_cursor: false,
            editor_border: EditorBorder::Box,
            editor_padding_x: 1,
            autocomplete_max_visible: 10,
            quiet_startup: false,
            tips_enabled: true,
            clear_on_shrink: false,
            show_terminal_progress: false,
            warnings: WarningSettings::default(),
            voice_silence_ms: 800,
            webtools_timeout_secs: 30,
            learn: LearnSettings {
                max_sessions: 20,
                max_age_days: 30,
                min_repeats: 2,
                min_request_repeats: 3,
                max_proposals: 8,
            },
            measure_token_surface: None,
            supports_images: false,
        }
    }
}

/// One change the pane asks the host to make (`SettingsCallbacks`).
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsChange {
    AutoCompact(bool),
    ToolEnabled {
        name: String,
        enabled: bool,
    },
    ToolGroup {
        id: String,
        enabled: bool,
    },
    ToolOutputView(ToolOutputView),
    ToolOutputMaxBytes(u64),
    ToolOutputMaxLines(u64),
    ContextGc(bool),
    Flag {
        name: String,
        value: FlagValue,
    },
    ShowImages(bool),
    ImageWidthCells(u64),
    AutoResizeImages(bool),
    BlockImages(bool),
    EnableSkillCommands(bool),
    Light(bool),
    PluginInstallScope(PluginInstallScope),
    EnablePluginTools(bool),
    Platform(Vec<MarketplacePlatform>),
    SteeringMode(QueueMode),
    FollowUpMode(QueueMode),
    Transport(String),
    ThinkingLevel(String),
    Theme(String),
    /// A theme to show while the theme list moves (`onThemePreview`); on
    /// cancel, the theme the list opened on.
    ThemePreview(String),
    HideThinkingBlock(bool),
    CollapseChangelog(bool),
    EnableInstallTelemetry(bool),
    DoubleEscapeAction(DoubleEscapeAction),
    TreeFilterMode(TreeFilterMode),
    ShowHardwareCursor(bool),
    EditorBorder(EditorBorder),
    EditorPaddingX(u64),
    AutocompleteMaxVisible(u64),
    QuietStartup(bool),
    TipsEnabled(bool),
    ClearOnShrink(bool),
    ShowTerminalProgress(bool),
    Warnings(WarningSettings),
    VoiceSilenceMs(u64),
    WebtoolsTimeoutSecs(u64),
    LearnSetting(LearnSettingKey, u64),
    /// The pane was closed (`onCancel`).
    Cancel,
}

/// Where the pane's changes go.
pub type SettingsCallback = Rc<RefCell<dyn FnMut(SettingsChange)>>;

fn emit(callback: &SettingsCallback, change: SettingsChange) {
    (callback.borrow_mut())(change);
}

/// `LEARN_SETTINGS`: the `/learn` thresholds as rows, with their presets.
const LEARN_SETTINGS: &[(LearnSettingKey, &str, &str, &[u64])] = &[
    (
        LearnSettingKey::MaxSessions,
        "Sessions scanned",
        "How many recent sessions in this directory /learn mines. Raise it on a repo you touch rarely.",
        &[10, 20, 30, 50, 100],
    ),
    (
        LearnSettingKey::MaxAgeDays,
        "Session age limit",
        "Ignore sessions older than this many days. A pattern that stopped is not a rule.",
        &[7, 14, 30, 60, 90, 180],
    ),
    (
        LearnSettingKey::MinRepeats,
        "Directive repeats",
        "Times a directive must recur before it is proposed. The signal/noise dial: raise it for fewer, better-evidenced proposals.",
        &[2, 3, 4, 5],
    ),
    (
        LearnSettingKey::MinRequestRepeats,
        "Workflow repeats",
        "Non-overlapping repeats a tool sequence needs before it is proposed as a skill.",
        &[2, 3, 4, 5, 6],
    ),
    (
        LearnSettingKey::MaxProposals,
        "Max proposals",
        "Cap on each list in the digest. Every proposal costs the model context.",
        &[3, 5, 8, 12, 20],
    ),
];

fn learn_value(learn: &LearnSettings, key: LearnSettingKey) -> u64 {
    match key {
        LearnSettingKey::MaxSessions => learn.max_sessions,
        LearnSettingKey::MaxAgeDays => learn.max_age_days,
        LearnSettingKey::MinRepeats => learn.min_repeats,
        LearnSettingKey::MinRequestRepeats => learn.min_request_repeats,
        LearnSettingKey::MaxProposals => learn.max_proposals,
    }
}

/// Preset list for a numeric row, always containing the value in force
/// (`presetValues`): a hand-set value must not snap away on the first press.
fn preset_values(presets: &[u64], current: u64) -> Vec<String> {
    let mut all = presets.to_vec();
    if !all.contains(&current) {
        all.push(current);
        all.sort_unstable();
    }
    all.iter().map(u64::to_string).collect()
}

fn bool_str(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

fn parse_u64(value: &str) -> u64 {
    value.trim().parse().unwrap_or(0)
}

/// A callback taking a row's value.
type ValueFn = Box<dyn FnMut(&str)>;

/// The shared `(id, value)` change handler.
type ChangeHandler = Rc<dyn Fn(&str, &str)>;

/// A submenu builder, shareable between the rows built from one leaf.
type SubmenuBuilder = Rc<dyn Fn(&str, DoneSlot) -> ComponentHandle>;

/// One row as data, so it can be rebuilt into a [`SettingItem`] each time a
/// list shows it.
#[derive(Clone)]
struct Leaf {
    id: String,
    label: String,
    description: Option<String>,
    current_value: String,
    value_suffix: Option<String>,
    keywords: Option<String>,
    values: Option<Vec<String>>,
    submenu: Option<SubmenuBuilder>,
}

impl Leaf {
    fn cycle(
        id: &str,
        label: &str,
        description: impl Into<String>,
        current: String,
        values: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: Some(description.into()),
            current_value: current,
            value_suffix: None,
            keywords: None,
            values: Some(values),
            submenu: None,
        }
    }

    fn toggle(id: &str, label: &str, description: impl Into<String>, current: bool) -> Self {
        Self::cycle(
            id,
            label,
            description,
            bool_str(current),
            strings(&["true", "false"]),
        )
    }

    fn opens(
        id: &str,
        label: &str,
        description: impl Into<String>,
        current: String,
        submenu: SubmenuBuilder,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: Some(description.into()),
            current_value: current,
            value_suffix: None,
            keywords: None,
            values: None,
            submenu: Some(submenu),
        }
    }

    fn suffix(mut self, suffix: Option<String>) -> Self {
        self.value_suffix = suffix;
        self
    }

    fn keywords(mut self, keywords: String) -> Self {
        self.keywords = Some(keywords);
        self
    }

    fn to_item(&self) -> SettingItem {
        SettingItem {
            id: self.id.clone(),
            label: self.label.clone(),
            description: self.description.clone(),
            current_value: self.current_value.clone(),
            value_suffix: self.value_suffix.clone(),
            keywords: self.keywords.clone(),
            values: self.values.clone(),
            submenu: self.submenu.clone().map(|build| {
                Box::new(move |current: &str, slot: DoneSlot| build(current, slot))
                    as Box<dyn Fn(&str, DoneSlot) -> ComponentHandle>
            }),
        }
    }
}

fn new_list(
    items: Vec<SettingItem>,
    max_visible: usize,
    enable_search: bool,
    on_change: impl FnMut(&str, &str) + 'static,
    on_cancel: impl FnMut() + 'static,
) -> Rc<RefCell<SettingsList>> {
    let mut list = SettingsList::new(
        items,
        max_visible,
        get_settings_list_theme(),
        SettingsListOptions { enable_search },
    );
    list.on_change = Some(Box::new(on_change));
    list.on_cancel = Some(Box::new(on_cancel));
    Rc::new(RefCell::new(list))
}

/// A submenu that is one settings list: the warnings, platform, tools,
/// external-tool, tool-output, flag and category submenus. Public so a
/// caller can reach the list a factory built (`submenu(...).settingsList`).
pub struct ListSubmenu {
    list: Rc<RefCell<SettingsList>>,
    /// Values to force back after the list handles a key: a change callback
    /// runs while the list is borrowed, so it cannot write to it directly.
    reverts: Rc<RefCell<Vec<(String, String)>>>,
}

impl ListSubmenu {
    fn new(list: Rc<RefCell<SettingsList>>) -> Self {
        Self {
            list,
            reverts: Rc::default(),
        }
    }

    /// The list inside (`settingsList`).
    pub fn settings_list(&self) -> Rc<RefCell<SettingsList>> {
        self.list.clone()
    }

    fn handle(self) -> ComponentHandle {
        Rc::new(RefCell::new(self))
    }
}

impl Component for ListSubmenu {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.list.borrow_mut().render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.list.borrow_mut().handle_input(data);
        let reverts: Vec<_> = self.reverts.borrow_mut().drain(..).collect();
        for (id, value) in reverts {
            self.list.borrow_mut().update_value(&id, &value);
        }
    }

    fn invalidate(&mut self) {
        self.list.borrow_mut().invalidate();
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// The list a submenu handle holds, when it is a [`ListSubmenu`].
pub fn submenu_list(handle: &ComponentHandle) -> Option<Rc<RefCell<SettingsList>>> {
    let component = handle.borrow();
    component
        .as_any()?
        .downcast_ref::<ListSubmenu>()
        .map(ListSubmenu::settings_list)
}

/// `WarningSettingsSubmenu`.
fn warning_settings_submenu(
    warnings: WarningSettings,
    on_change: impl Fn(WarningSettings) + 'static,
    slot: DoneSlot,
) -> ComponentHandle {
    let items = vec![
        Leaf::toggle(
            "anthropic-extra-usage",
            "Anthropic extra usage",
            "Warn when Anthropic subscription auth may use paid extra usage",
            warnings.anthropic_extra_usage.unwrap_or(true),
        )
        .to_item(),
        Leaf::toggle(
            "websearch-api-key",
            "Web search API key",
            "Warn when websearch is enabled with no search API key, so it falls back to keyless DuckDuckGo",
            warnings.websearch_api_key.unwrap_or(true),
        )
        .to_item(),
    ];
    let max = items.len().min(10);
    let mut state = warnings;
    let list = new_list(
        items,
        max,
        false,
        move |id, value| {
            let on = value == "true";
            match id {
                "anthropic-extra-usage" => state.anthropic_extra_usage = Some(on),
                "websearch-api-key" => state.websearch_api_key = Some(on),
                _ => return,
            }
            on_change(state);
        },
        move || done(&slot, None),
    );
    ListSubmenu::new(list).handle()
}

/// `PLATFORM_ROWS`, in pane order.
const PLATFORM_ROWS: &[(MarketplacePlatform, &str, &str)] = &[
    (
        MarketplacePlatform::Claude,
        "claude",
        "Claude Code layout: .claude/ scaffolds, and authored plugins drop into ~/.claude/skills/<id>/. The default when nothing is set.",
    ),
    (
        MarketplacePlatform::Github,
        "github (copilot, gh)",
        "Copilot layout: .github/ scaffolds, and authored plugins are produced under ~/.agents/publish/github/<id>/.",
    ),
    (
        MarketplacePlatform::Agents,
        "agents (native)",
        "Cross-vendor .agents/ layout. Scaffolds only - a plugin belongs to no marketplace in this layout, so plugin authoring ignores it.",
    ),
];

/// The platform row's value: the selection, or the fallback when empty.
fn platform_summary(platforms: &[MarketplacePlatform]) -> String {
    if platforms.is_empty() {
        "default (claude)".to_string()
    } else {
        platforms
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `PlatformSubmenu`: one on/off row per target, since the setting is a list.
fn platform_submenu(
    platforms: &[MarketplacePlatform],
    on_change: impl Fn(Vec<MarketplacePlatform>) + 'static,
    slot: DoneSlot,
) -> ComponentHandle {
    let selected: Rc<RefCell<HashSet<MarketplacePlatform>>> =
        Rc::new(RefCell::new(platforms.iter().copied().collect()));
    // Pane order, not click order, so the persisted list is stable.
    let ordered = {
        let selected = selected.clone();
        move || -> Vec<MarketplacePlatform> {
            let selected = selected.borrow();
            PLATFORM_ROWS
                .iter()
                .map(|(p, _, _)| *p)
                .filter(|p| selected.contains(p))
                .collect()
        }
    };
    let items: Vec<SettingItem> = PLATFORM_ROWS
        .iter()
        .map(|(platform, label, description)| {
            let on = if selected.borrow().contains(platform) {
                "on"
            } else {
                "off"
            };
            Leaf::cycle(
                platform.as_str(),
                label,
                *description,
                on.into(),
                strings(&["on", "off"]),
            )
            .to_item()
        })
        .collect();
    let max = items.len().min(10);
    let change_ordered = ordered.clone();
    let list = new_list(
        items,
        max,
        false,
        move |id, value| {
            let Some(platform) = MarketplacePlatform::parse(id) else {
                return;
            };
            if value == "on" {
                selected.borrow_mut().insert(platform);
            } else {
                selected.borrow_mut().remove(&platform);
            }
            on_change(change_ordered());
        },
        move || done(&slot, Some(platform_summary(&ordered()))),
    );
    ListSubmenu::new(list).handle()
}

/// Trailing sentence for a row inert until a binary shows up (`gateNote`).
fn gate_note(gate: Option<&ExternalToolStatus>) -> String {
    match gate {
        Some(gate) if !gate.installed => {
            if gate.downloadable {
                format!(
                    " Needs the {} binary, which {APP_NAME} fetches the first time this is used. Until then: {}",
                    gate.doc.tool, gate.doc.fallback
                )
            } else {
                format!(
                    " Needs the {} binary, and this environment will not fetch it. Until then: {}",
                    gate.doc.tool, gate.doc.fallback
                )
            }
        }
        _ => String::new(),
    }
}

/// "3 of 5 installed" (`externalCount`).
fn external_count(statuses: &[ExternalToolStatus]) -> String {
    let installed = statuses.iter().filter(|s| s.installed).count();
    format!("{installed} of {} installed", statuses.len())
}

/// The capability words (`externalSummary`).
fn external_summary(statuses: &[ExternalToolStatus]) -> String {
    statuses
        .iter()
        .map(|s| s.doc.tool)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Value-column marker for a gated row (`gateSuffix`).
fn gate_suffix(gate: Option<&ExternalToolStatus>) -> Option<String> {
    match gate {
        Some(gate) if !gate.installed => Some(format!("needs {}", gate.doc.tool)),
        _ => None,
    }
}

const CORE_TOOLS: &[&str] = &["read", "bash", "edit", "write"];
const GROUP_PREFIX: &str = "group:";

/// `ToolsSubmenu`: group switches first, then one on/off row per tool. The
/// last core tool cannot be turned off.
fn tools_submenu(
    tools: &[ToolToggleInfo],
    groups: &[ToolGroupInfo],
    gates: &HashMap<String, ExternalToolStatus>,
    on_change: impl Fn(&str, bool) + 'static,
    on_group_change: impl Fn(&str, bool) + 'static,
    slot: DoneSlot,
) -> ComponentHandle {
    let enabled: Rc<RefCell<HashMap<String, bool>>> = Rc::new(RefCell::new(
        tools.iter().map(|t| (t.name.clone(), t.enabled)).collect(),
    ));
    let group_items = groups.iter().map(|group| {
        let id = format!("{GROUP_PREFIX}{}", group.id);
        let gate = gates.get(&id);
        Leaf::cycle(
            &id,
            &format!("[group] {}", group.label),
            format!(
                "{} Governs whether these tools exist; applies on the next session.{}",
                group.description,
                gate_note(gate)
            ),
            if group.enabled { "on" } else { "off" }.into(),
            strings(&["on", "off"]),
        )
        // Settable with the binary missing: the setting is what fetches it.
        .suffix(gate_suffix(gate))
        .to_item()
    });
    let tool_items = tools.iter().map(|tool| {
        let description = if CORE_TOOLS.contains(&tool.name.as_str()) {
            "Core tool. Disabling leaves the agent unable to perform this action in every session."
        } else {
            "Disable to remove this tool from the agent this session and every future session."
        };
        Leaf::cycle(
            &tool.name,
            &tool.name,
            description,
            if tool.enabled { "on" } else { "off" }.into(),
            strings(&["on", "off"]),
        )
        .suffix(
            tool.tokens
                .map(|t| format!("{} tok/turn", token_count(t as u64))),
        )
        .to_item()
    });
    let items: Vec<SettingItem> = group_items.chain(tool_items).collect();
    let max = items.len().min(12);
    let reverts: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
    let pending = reverts.clone();
    let list = new_list(
        items,
        max,
        true,
        move |id, value| {
            let want = value == "on";
            if let Some(group) = id.strip_prefix(GROUP_PREFIX) {
                on_group_change(group, want);
                return;
            }
            if !want && CORE_TOOLS.contains(&id) {
                let enabled = enabled.borrow();
                let remaining = CORE_TOOLS
                    .iter()
                    .filter(|n| **n != id && enabled.get(**n).copied().unwrap_or(false))
                    .count();
                if remaining == 0 {
                    pending
                        .borrow_mut()
                        .push((id.to_string(), "on".to_string()));
                    return;
                }
            }
            enabled.borrow_mut().insert(id.to_string(), want);
            on_change(id, want);
        },
        move || done(&slot, None),
    );
    let mut submenu = ListSubmenu::new(list);
    submenu.reverts = reverts;
    submenu.handle()
}

/// `ExternalToolDetailSubmenu`: read-only rows about one binary.
fn external_tool_detail_submenu(status: &ExternalToolStatus, slot: DoneSlot) -> ComponentHandle {
    let acquisition = match status.doc.acquisition {
        Acquisition::Startup => "Downloaded in the background at startup.",
        Acquisition::OnDemand => "Downloaded the first time the feature is used.",
        Acquisition::Manual => "Never downloaded automatically; install it yourself.",
    };
    let path = status.path.as_deref().unwrap_or("");
    let location = if status.installed {
        match status.source {
            Some(ManagedToolSource::Path) => format!("found on PATH as `{path}`"),
            Some(ManagedToolSource::Override) => {
                format!("{} points at {path}", status.override_env)
            }
            _ => format!("{APP_NAME}'s own copy at {path}"),
        }
    } else if status.downloadable {
        format!("{acquisition} Not on this machine yet.")
    } else {
        "Not installed, and this environment will not download it (offline mode, or no published build for this platform).".to_string()
    };
    let enables = status.doc.enables.len();
    let rows: Vec<(&str, String, String)> = vec![
        ("Status", status_label(status).to_string(), location),
        (
            "Enables",
            format!("{enables} feature{}", if enables == 1 { "" } else { "s" }),
            status.doc.enables.join("; "),
        ),
        (
            "Without it",
            "fallback".to_string(),
            status.doc.fallback.to_string(),
        ),
        (
            "Source",
            if status.repo.is_empty() {
                "-".to_string()
            } else {
                status.repo.clone()
            },
            format!(
                "Release archives come from github.com/{}. {acquisition}",
                status.repo
            ),
        ),
        (
            "Env",
            status.doc.env.len().to_string(),
            status.doc.env.join("  |  "),
        ),
    ];
    let items: Vec<SettingItem> = rows
        .into_iter()
        .enumerate()
        .map(|(i, (label, value, description))| SettingItem {
            id: format!("detail-{i}"),
            label: label.to_string(),
            description: Some(description),
            current_value: value,
            value_suffix: None,
            keywords: None,
            values: None,
            submenu: None,
        })
        .collect();
    let max = items.len();
    let list = new_list(items, max, false, |_, _| {}, move || done(&slot, None));
    ListSubmenu::new(list).handle()
}

/// `ExternalToolsSubmenu`: one row per managed binary.
fn external_tools_submenu(statuses: &[ExternalToolStatus], slot: DoneSlot) -> ComponentHandle {
    let items: Vec<SettingItem> = statuses
        .iter()
        .map(|status| {
            let detail = status.clone();
            let suffix = if status.installed {
                None
            } else if status.downloadable {
                Some("auto-fetch".to_string())
            } else {
                Some("manual".to_string())
            };
            let keywords = [status.doc.tool, status.repo.as_str()]
                .into_iter()
                .chain(status.doc.settings_keys.iter().copied())
                .chain(status.doc.enables.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            Leaf::opens(
                &format!("ext-{}", status.doc.tool),
                status.doc.label,
                format!("{} Without it: {}", status.doc.summary, status.doc.fallback),
                status_label(status).to_string(),
                Rc::new(move |_current: &str, slot: DoneSlot| {
                    external_tool_detail_submenu(&detail, slot)
                }),
            )
            .suffix(suffix)
            .keywords(keywords)
            .to_item()
        })
        .collect();
    let max = items.len().min(10);
    let list = new_list(items, max, true, |_, _| {}, move || done(&slot, None));
    ListSubmenu::new(list).handle()
}

/// `TOOL_OUTPUT_BYTE_PRESETS`.
const TOOL_OUTPUT_BYTE_PRESETS: &[(&str, u64)] = &[
    ("8 KB", 8 * 1024),
    ("16 KB", 16 * 1024),
    ("32 KB", 32 * 1024),
    ("64 KB", 64 * 1024),
    ("128 KB", 128 * 1024),
];

fn bytes_to_label(bytes: u64) -> String {
    match TOOL_OUTPUT_BYTE_PRESETS.iter().find(|(_, b)| *b == bytes) {
        Some((label, _)) => label.to_string(),
        None => format!("{} KB", (bytes as f64 / 1024.0).round() as u64),
    }
}

/// Byte-cap labels, including a hand-set cap that matches no preset.
fn byte_labels(current: u64) -> Vec<String> {
    let mut all: Vec<(String, u64)> = TOOL_OUTPUT_BYTE_PRESETS
        .iter()
        .map(|(l, b)| (l.to_string(), *b))
        .collect();
    if !TOOL_OUTPUT_BYTE_PRESETS.iter().any(|(_, b)| *b == current) {
        all.push((bytes_to_label(current), current));
    }
    all.sort_by_key(|(_, b)| *b);
    all.into_iter().map(|(l, _)| l).collect()
}

/// `ToolSettingsSubmenu`: how much of a tool result is rendered, and caps.
fn tool_settings_submenu(
    view: ToolOutputView,
    max_bytes: u64,
    max_lines: u64,
    callback: SettingsCallback,
    slot: DoneSlot,
) -> ComponentHandle {
    let items = vec![
        Leaf::cycle(
            "tool-output-view",
            "View",
            format!(
                "How much of a tool call the transcript shows, least to most. 'radar': {}. 'peek': {}. 'full': {}.",
                tool_output_view_description(ToolOutputView::Radar),
                tool_output_view_description(ToolOutputView::Peek),
                tool_output_view_description(ToolOutputView::Full)
            ),
            view.as_str().into(),
            TOOL_OUTPUT_VIEWS.iter().map(|v| v.as_str().to_string()).collect(),
        )
        .to_item(),
        Leaf::cycle(
            "output-max-bytes",
            "Max bytes",
            "Byte cap on a single read/bash result before truncation. Applies to future tool calls.",
            bytes_to_label(max_bytes),
            byte_labels(max_bytes),
        )
        .to_item(),
        Leaf::cycle(
            "output-max-lines",
            "Max lines",
            "Line cap on a single read/bash result before truncation. Applies to future tool calls.",
            max_lines.to_string(),
            preset_values(&[200, 400, 800, 1600, 3200], max_lines),
        )
        .to_item(),
    ];
    let max = items.len().min(10);
    let list = new_list(
        items,
        max,
        false,
        move |id, value| match id {
            "tool-output-view" => {
                if let Some(view) = ToolOutputView::parse(value) {
                    emit(&callback, SettingsChange::ToolOutputView(view));
                }
            }
            "output-max-bytes" => {
                // A hand-set cap's label is "<n> KB" by construction.
                let bytes = match TOOL_OUTPUT_BYTE_PRESETS.iter().find(|(l, _)| *l == value) {
                    Some((_, b)) => Some(*b),
                    None => value
                        .trim_end_matches("KB")
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .map(|kb| (kb * 1024.0).round() as u64),
                };
                if let Some(bytes) = bytes.filter(|b| *b > 0) {
                    emit(&callback, SettingsChange::ToolOutputMaxBytes(bytes));
                }
            }
            "output-max-lines" => emit(
                &callback,
                SettingsChange::ToolOutputMaxLines(parse_u64(value)),
            ),
            _ => {}
        },
        move || done(&slot, None),
    );
    ListSubmenu::new(list).handle()
}

/// `FlagStringEditSubmenu`: a one-line editor for a string flag.
struct FlagStringEditSubmenu {
    container: Container,
    input: Rc<RefCell<Input>>,
}

impl FlagStringEditSubmenu {
    fn new(flag_name: &str, current: &str, slot: DoneSlot) -> Self {
        let t = theme();
        let mut container = Container::default();
        container.add_child(Rc::new(RefCell::new(Text::new(
            t.bold(&t.fg("accent", &format!("Flag: --{flag_name}"))),
            0,
            0,
        ))));
        container.add_child(Rc::new(RefCell::new(Text::new(
            t.fg("muted", "Enter a value · Enter to save · Esc to cancel"),
            0,
            0,
        ))));
        container.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        let mut input = Input::new();
        style_input(&mut input);
        input.set_value(current);
        let submit_slot = slot.clone();
        input.on_submit = Some(Box::new(move |value: &str| {
            done(&submit_slot, Some(value.to_string()))
        }));
        input.on_escape = Some(Box::new(move || done(&slot, None)));
        let input = Rc::new(RefCell::new(input));
        container.add_child(input.clone());
        Self { container, input }
    }
}

impl Component for FlagStringEditSubmenu {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.container.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.input.borrow_mut().handle_input(data);
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}

/// `FlagsSubmenu`: boolean flags toggle, string flags open an editor.
fn flags_submenu(
    flags: &[FlagInfo],
    callback: SettingsCallback,
    slot: DoneSlot,
) -> ComponentHandle {
    let items: Vec<SettingItem> = flags
        .iter()
        .map(|flag| {
            let base = flag
                .description
                .clone()
                .unwrap_or_else(|| "Extension-registered flag.".to_string());
            let description = format!(
                "{base} Persists across sessions; some flags need a restart to fully apply."
            );
            match &flag.value {
                FlagValue::Bool(on) => Leaf::cycle(
                    &flag.name,
                    &flag.name,
                    description,
                    if *on { "on" } else { "off" }.into(),
                    strings(&["on", "off"]),
                ),
                FlagValue::String(value) => {
                    let name = flag.name.clone();
                    Leaf::opens(
                        &flag.name,
                        &flag.name,
                        description,
                        value.clone(),
                        Rc::new(move |current: &str, slot: DoneSlot| {
                            Rc::new(RefCell::new(FlagStringEditSubmenu::new(
                                &name, current, slot,
                            ))) as ComponentHandle
                        }),
                    )
                }
            }
            .to_item()
        })
        .collect();
    let booleans: HashSet<String> = flags
        .iter()
        .filter(|f| matches!(f.value, FlagValue::Bool(_)))
        .map(|f| f.name.clone())
        .collect();
    let max = items.len().min(12);
    let list = new_list(
        items,
        max,
        true,
        move |id, value| {
            let value = if booleans.contains(id) {
                FlagValue::Bool(value == "on")
            } else {
                FlagValue::String(value.to_string())
            };
            emit(
                &callback,
                SettingsChange::Flag {
                    name: id.to_string(),
                    value,
                },
            );
        },
        move || done(&slot, None),
    );
    ListSubmenu::new(list).handle()
}

/// `SelectSubmenu`: a titled select list (thinking level, theme).
pub struct SelectSubmenu {
    container: Container,
    list: Rc<RefCell<SelectList>>,
}

impl SelectSubmenu {
    fn new(
        title: &str,
        description: &str,
        options: Vec<SelectItem>,
        current: &str,
        on_select: impl FnMut(&str) + 'static,
        on_cancel: impl FnMut() + 'static,
        on_selection_change: Option<ValueFn>,
    ) -> Self {
        let t = theme();
        let mut container = Container::default();
        container.add_child(Rc::new(RefCell::new(Text::new(
            t.bold(&t.fg("accent", title)),
            0,
            0,
        ))));
        if !description.is_empty() {
            container.add_child(Rc::new(RefCell::new(Text::new(
                t.fg("muted", description),
                0,
                0,
            ))));
        }
        container.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        let current_index = options.iter().position(|o| o.value == current);
        let max = options.len().min(10);
        let mut list = SelectList::new(options, max, get_select_list_theme(), layout(12, 32));
        if let Some(index) = current_index {
            list.set_selected_index(index);
        }
        let mut on_select = on_select;
        list.on_select = Some(Box::new(move |item: &SelectItem| on_select(&item.value)));
        list.on_cancel = Some(Box::new(on_cancel));
        if let Some(mut change) = on_selection_change {
            list.on_selection_change = Some(Box::new(move |item: &SelectItem| change(&item.value)));
        }
        let list = Rc::new(RefCell::new(list));
        container.add_child(list.clone());
        container.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        container.add_child(Rc::new(RefCell::new(Text::new(
            t.fg("dim", "  Enter to select · Esc to go back"),
            0,
            0,
        ))));
        Self { container, list }
    }
}

impl Component for SelectSubmenu {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.container.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.list.borrow_mut().handle_input(data);
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}

/// Token counts grouped and exact (`tokenCount`): the pane compares numbers,
/// and "2.7k" would hide the difference.
fn token_count(tokens: u64) -> String {
    let digits = tokens.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The fixed per-turn cost, as one line under the pane (`formatSurfaceLine`).
fn format_surface_line(surface: &PromptSurface) -> String {
    let n = surface.tools.len();
    let tools = format!("{n} tool{}", if n == 1 { "" } else { "s" });
    format!(
        "  Per-turn surface: {} tokens  {}",
        token_count(surface.total_tokens as u64),
        theme().fg(
            "dim",
            &format!(
                "({} system prompt + {} schemas, {tools})",
                token_count(surface.system_prompt_tokens as u64),
                token_count(surface.tool_schema_tokens as u64)
            )
        )
    )
}

/// Re-price the pane after a change (`refreshTokenSurface`).
#[derive(Clone)]
struct SurfaceLine {
    text: Option<Rc<RefCell<Text>>>,
    measure: Option<Rc<dyn Fn() -> PromptSurface>>,
}

impl SurfaceLine {
    fn refresh(&self) {
        if let (Some(text), Some(measure)) = (&self.text, &self.measure) {
            text.borrow_mut().set_text(format_surface_line(&measure()));
        }
    }
}

/// `SettingsSelectorComponent`: the whole pane in the prompt's frame.
pub struct SettingsSelectorComponent {
    frame: InputFrame,
    surface: SurfaceLine,
    list: Rc<RefCell<SettingsList>>,
}

impl SettingsSelectorComponent {
    pub fn new(config: SettingsConfig, callback: impl FnMut(SettingsChange) + 'static) -> Self {
        let callback: SettingsCallback = Rc::new(RefCell::new(callback));
        let config = Rc::new(config);
        let follow_up_key = key_display_text("app.message.followUp");
        let current_warnings = Rc::new(RefCell::new(config.warnings));

        let project_pinned: HashSet<String> =
            config.project_pinned_settings.iter().cloned().collect();
        let pinned_note = |key: &str| -> String {
            if project_pinned.contains(key) {
                format!(" This repo's {CONFIG_DIR_NAME}/settings.json sets {key}, which overrides this row from the next session on.")
            } else {
                String::new()
            }
        };
        let row_gates = Rc::new(build_row_gates(&config.external_tools));
        let initial_surface = config.measure_token_surface.as_ref().map(|m| m());
        let surface_text = initial_surface
            .as_ref()
            .map(|s| Rc::new(RefCell::new(Text::new(format_surface_line(s), 0, 0))));
        let surface = SurfaceLine {
            text: surface_text.clone(),
            measure: config.measure_token_surface.clone(),
        };
        let tools_on = config.tools.iter().filter(|t| t.enabled).count();
        let tools_off = config.tools.len() - tools_on;

        // Every leaf, in the order the flat list of the original has them.
        let mut leaves: Vec<Leaf> = Vec::new();
        leaves.push(Leaf::toggle(
            "autocompact",
            "Auto-compact",
            "Automatically compact context when it gets too large",
            config.auto_compact,
        ));
        if config.supports_images {
            leaves.push(Leaf::toggle(
                "show-images",
                "Show images",
                "Render images inline in terminal",
                config.show_images,
            ));
            leaves.push(Leaf::cycle(
                "image-width-cells",
                "Image width",
                "Preferred inline image width in terminal cells",
                config.image_width_cells.to_string(),
                preset_values(&[60, 80, 120], config.image_width_cells),
            ));
        }
        leaves.push(Leaf::toggle(
            "auto-resize-images",
            "Auto-resize images",
            "Resize large images to 2000x2000 max for better model compatibility",
            config.auto_resize_images,
        ));
        leaves.push(Leaf::toggle(
            "block-images",
            "Block images",
            "Prevent images from being sent to LLM providers",
            config.block_images,
        ));
        leaves.push(Leaf::toggle(
            "context-gc",
            "Context GC",
            "Stub superseded read results (files later edited or re-read) out of the outgoing context.",
            config.context_gc,
        ));
        leaves.push(Leaf::toggle(
            "light",
            "Light preset",
            "Low-token preset for small or local models: read/write/edit/bash only with stripped schemas, a terse system prompt, and no subagents/TodoWrite/skills/context files. Applies on the next session.",
            config.light,
        ));
        leaves.push(Leaf::toggle(
            "skill-commands",
            "Skill commands",
            "Register skills as /skill:name commands",
            config.enable_skill_commands,
        ));
        leaves.push(Leaf::toggle(
            "plugin-tools",
            "Plugin system",
            format!(
                "Autonomous plugin system: the lifecycle tools (SearchPlugins, InstallPlugin, ...), ProposePlugin, and the plugin-reuse nudge. Tools arrive on the next session; the nudge follows at once.{}",
                pinned_note("enablePluginTools")
            ),
            config.enable_plugin_tools,
        ));
        leaves.push(Leaf::cycle(
            "plugin-install-scope",
            "Plugin install scope",
            "Where autonomous plugin installs go: user (~/.agents) or project (this repo, shared)",
            config.plugin_install_scope.as_str().into(),
            strings(&["user", "project"]),
        ));
        let current_platforms = Rc::new(RefCell::new(config.platform.clone()));
        {
            let current_platforms = current_platforms.clone();
            let callback = callback.clone();
            leaves.push(Leaf::opens(
                "platform",
                "Platform",
                format!(
                    "Vendor layout(s) {APP_NAME} writes artifacts in: authored plugins and the /new-skill //new-agent //new-command scaffolds.{}",
                    pinned_note("platform")
                ),
                platform_summary(&config.platform),
                Rc::new(move |_current: &str, slot: DoneSlot| {
                    let platforms = current_platforms.borrow().clone();
                    let current_platforms = current_platforms.clone();
                    let callback = callback.clone();
                    platform_submenu(
                        &platforms,
                        move |platforms| {
                            *current_platforms.borrow_mut() = platforms.clone();
                            emit(&callback, SettingsChange::Platform(platforms));
                        },
                        slot,
                    )
                }),
            ));
        }
        leaves.push(Leaf::toggle(
            "show-hardware-cursor",
            "Show hardware cursor",
            "Show the terminal cursor while still positioning it for IME support",
            config.show_hardware_cursor,
        ));
        leaves.push(Leaf::cycle(
            "editor-border",
            "Editor border",
            "Box draws side borders, rule draws horizontal lines only",
            config.editor_border.as_str().into(),
            strings(&["box", "rule"]),
        ));
        leaves.push(Leaf::cycle(
            "editor-padding",
            "Editor padding",
            "Horizontal padding for input editor (0-3)",
            config.editor_padding_x.to_string(),
            strings(&["0", "1", "2", "3"]),
        ));
        leaves.push(Leaf::cycle(
            "autocomplete-max-visible",
            "Autocomplete max items",
            "Max visible items in autocomplete dropdown (3-20)",
            config.autocomplete_max_visible.to_string(),
            preset_values(&[3, 5, 7, 10, 15, 20], config.autocomplete_max_visible),
        ));
        leaves.push(Leaf::toggle(
            "clear-on-shrink",
            "Clear on shrink",
            "Clear empty rows when content shrinks (may cause flicker)",
            config.clear_on_shrink,
        ));
        leaves.push(Leaf::toggle(
            "terminal-progress",
            "Terminal progress",
            "Show OSC 9;4 progress indicators in the terminal tab bar",
            config.show_terminal_progress,
        ));
        let voice_gate = row_gates.get("voice-silence-ms");
        leaves.push(
            Leaf::cycle(
                "voice-silence-ms",
                "Voice silence window",
                format!(
                    "Trailing-silence (ms) before voice capture auto-stops (300-10000). Env: VOICETOOLS_SILENCE_MS.{}",
                    gate_note(voice_gate)
                ),
                config.voice_silence_ms.to_string(),
                preset_values(
                    &[300, 500, 800, 1200, 2000, 3000, 5000, 8000, 10000],
                    config.voice_silence_ms,
                ),
            )
            .suffix(gate_suffix(voice_gate)),
        );
        let web_gate = row_gates.get("webtools-timeout-secs");
        leaves.push(
            Leaf::cycle(
                "webtools-timeout-secs",
                "Web tools timeout",
                format!(
                    "Per-request timeout (secs) for webfetch/websearch (1-120). Env: HOOCODE_WEBTOOLS_TIMEOUT.{}",
                    gate_note(web_gate)
                ),
                config.webtools_timeout_secs.to_string(),
                preset_values(&[5, 10, 15, 30, 60, 120], config.webtools_timeout_secs),
            )
            .suffix(gate_suffix(web_gate)),
        );
        for (key, label, description, presets) in LEARN_SETTINGS {
            let value = learn_value(&config.learn, *key);
            leaves.push(Leaf::cycle(
                key.as_str(),
                label,
                *description,
                value.to_string(),
                preset_values(presets, value),
            ));
        }
        // The rows of the original's initial list that the splices above
        // were inserted around.
        leaves.push(Leaf::cycle(
            "steering-mode",
            "Steering mode",
            "Enter while streaming queues steering messages. 'one-at-a-time': deliver one, wait for response. 'all': deliver all at once.",
            config.steering_mode.as_str().into(),
            strings(&["one-at-a-time", "all"]),
        ));
        leaves.push(Leaf::cycle(
            "follow-up-mode",
            "Follow-up mode",
            format!("{follow_up_key} queues follow-up messages until agent stops. 'one-at-a-time': deliver one, wait for response. 'all': deliver all at once."),
            config.follow_up_mode.as_str().into(),
            strings(&["one-at-a-time", "all"]),
        ));
        leaves.push(Leaf::cycle(
            "transport",
            "Transport",
            "Preferred transport for providers that support multiple transports",
            config.transport.clone(),
            strings(&["sse", "websocket", "websocket-cached", "auto"]),
        ));
        leaves.push(Leaf::toggle(
            "hide-thinking",
            "Hide thinking",
            "Hide thinking blocks in assistant responses",
            config.hide_thinking_block,
        ));
        leaves.push(Leaf::toggle(
            "collapse-changelog",
            "Collapse changelog",
            "Show condensed changelog after updates",
            config.collapse_changelog,
        ));
        leaves.push(Leaf::toggle(
            "quiet-startup",
            "Quiet startup",
            "Disable verbose printing at startup",
            config.quiet_startup,
        ));
        leaves.push(Leaf::toggle(
            "tips",
            "Tips",
            "Show an occasional tip on the band above the prompt while idle or streaming. Never interrupts, never repeats until it has run out of things to say.",
            config.tips_enabled,
        ));
        leaves.push(Leaf::toggle(
            "install-telemetry",
            "Install telemetry",
            "Send an anonymous version/update ping after changelog-detected updates",
            config.enable_install_telemetry,
        ));
        leaves.push(Leaf::cycle(
            "double-escape-action",
            "Double-escape action",
            "Action when pressing Escape twice with empty editor",
            config.double_escape_action.as_str().into(),
            strings(&["tree", "fork", "none"]),
        ));
        leaves.push(Leaf::cycle(
            "tree-filter-mode",
            "Tree filter mode",
            "Default filter when opening /tree",
            config.tree_filter_mode.as_str().into(),
            strings(&["default", "no-tools", "user-only", "labeled-only", "all"]),
        ));
        {
            let current_warnings = current_warnings.clone();
            let callback = callback.clone();
            leaves.push(Leaf::opens(
                "warnings",
                "Warnings",
                "Enable or disable individual warnings",
                "configure".into(),
                Rc::new(move |_current: &str, slot: DoneSlot| {
                    let warnings = *current_warnings.borrow();
                    let current_warnings = current_warnings.clone();
                    let callback = callback.clone();
                    warning_settings_submenu(
                        warnings,
                        move |warnings| {
                            *current_warnings.borrow_mut() = warnings;
                            emit(&callback, SettingsChange::Warnings(warnings));
                        },
                        slot,
                    )
                }),
            ));
        }
        {
            let config = config.clone();
            let callback = callback.clone();
            leaves.push(Leaf::opens(
                "thinking",
                "Thinking level",
                "Reasoning depth for thinking-capable models",
                config.thinking_level.clone(),
                Rc::new(move |current: &str, slot: DoneSlot| {
                    let options = config
                        .available_thinking_levels
                        .iter()
                        .map(|level| SelectItem {
                            value: level.clone(),
                            label: level.clone(),
                            description: thinking_level_description(level).map(String::from),
                        })
                        .collect();
                    let callback = callback.clone();
                    let select_slot = slot.clone();
                    Rc::new(RefCell::new(SelectSubmenu::new(
                        "Thinking Level",
                        "Select reasoning depth for thinking-capable models",
                        options,
                        current,
                        move |value| {
                            if ThinkingLevelSetting::parse(value).is_some() {
                                emit(&callback, SettingsChange::ThinkingLevel(value.to_string()));
                            }
                            done(&select_slot, Some(value.to_string()));
                        },
                        move || done(&slot, None),
                        None,
                    ))) as ComponentHandle
                }),
            ));
        }
        {
            let config = config.clone();
            let callback = callback.clone();
            leaves.push(Leaf::opens(
                "theme",
                "Theme",
                "Color theme for the interface",
                config.current_theme.clone(),
                Rc::new(move |current: &str, slot: DoneSlot| {
                    let options = config
                        .available_themes
                        .iter()
                        .map(|t| SelectItem {
                            value: t.clone(),
                            label: t.clone(),
                            description: get_theme_description(t),
                        })
                        .collect();
                    let select_callback = callback.clone();
                    let cancel_callback = callback.clone();
                    let preview_callback = callback.clone();
                    let select_slot = slot.clone();
                    let original = current.to_string();
                    Rc::new(RefCell::new(SelectSubmenu::new(
                        "Theme",
                        "Select color theme",
                        options,
                        current,
                        move |value| {
                            emit(&select_callback, SettingsChange::Theme(value.to_string()));
                            done(&select_slot, Some(value.to_string()));
                        },
                        move || {
                            // Restore the theme the list opened on.
                            emit(
                                &cancel_callback,
                                SettingsChange::ThemePreview(original.clone()),
                            );
                            done(&slot, None);
                        },
                        Some(Box::new(move |value: &str| {
                            emit(
                                &preview_callback,
                                SettingsChange::ThemePreview(value.to_string()),
                            );
                        })),
                    ))) as ComponentHandle
                }),
            ));
        }
        let leaves = Rc::new(RefCell::new(leaves));

        // The tool and flag rows, together near the top.
        let mut top: Vec<Leaf> = Vec::new();
        {
            let config = config.clone();
            let callback = callback.clone();
            let row_gates = row_gates.clone();
            let surface = surface.clone();
            top.push(
                Leaf::opens(
                    "tools",
                    "Tools",
                    "Enable/disable tools and tool groups (web, semantic search), each priced by what its schema costs per turn. Changes persist across sessions.",
                    if tools_off > 0 {
                        format!("{tools_on} on · {tools_off} off")
                    } else {
                        format!("{tools_on} on")
                    },
                    Rc::new(move |_current: &str, slot: DoneSlot| {
                        let tool_callback = callback.clone();
                        let group_callback = callback.clone();
                        let surface = surface.clone();
                        tools_submenu(
                            &config.tools,
                            &config.tool_groups,
                            &row_gates,
                            move |name, enabled| {
                                emit(
                                    &tool_callback,
                                    SettingsChange::ToolEnabled {
                                        name: name.to_string(),
                                        enabled,
                                    },
                                );
                                // Applied live by the host: the surface is stale.
                                surface.refresh();
                            },
                            move |id, enabled| {
                                emit(
                                    &group_callback,
                                    SettingsChange::ToolGroup {
                                        id: id.to_string(),
                                        enabled,
                                    },
                                )
                            },
                            slot,
                        )
                    }),
                )
                .suffix(
                    initial_surface
                        .as_ref()
                        .map(|s| format!("{} tok/turn", token_count(s.tool_schema_tokens as u64))),
                ),
            );
        }
        {
            let config = config.clone();
            let callback = callback.clone();
            top.push(Leaf::opens(
                "tool-output",
                "Tool output",
                "How much of a tool result is rendered, and where it is truncated.",
                config.tool_output_view.as_str().into(),
                Rc::new(move |_current: &str, slot: DoneSlot| {
                    tool_settings_submenu(
                        config.tool_output_view,
                        config.tool_output_max_bytes,
                        config.tool_output_max_lines,
                        callback.clone(),
                        slot,
                    )
                }),
            ));
        }
        if !config.flags.is_empty() {
            let n = config.flags.len();
            let config = config.clone();
            let callback = callback.clone();
            top.push(Leaf::opens(
                "flags",
                "Flags",
                "Set flags registered by extensions. Changes persist across sessions.",
                format!("{n} flag{}", if n == 1 { "" } else { "s" }),
                Rc::new(move |_current: &str, slot: DoneSlot| {
                    flags_submenu(&config.flags, callback.clone(), slot)
                }),
            ));
        }

        // The shared change handler for every leaf (`applyChange`).
        let apply_change: ChangeHandler = {
            let callback = callback.clone();
            let surface = surface.clone();
            Rc::new(move |id: &str, value: &str| {
                let on = value == "true";
                let n = parse_u64(value);
                let change = match id {
                    "autocompact" => Some(SettingsChange::AutoCompact(on)),
                    "show-images" => Some(SettingsChange::ShowImages(on)),
                    "image-width-cells" => Some(SettingsChange::ImageWidthCells(n)),
                    "auto-resize-images" => Some(SettingsChange::AutoResizeImages(on)),
                    "block-images" => Some(SettingsChange::BlockImages(on)),
                    "context-gc" => Some(SettingsChange::ContextGc(on)),
                    "skill-commands" => Some(SettingsChange::EnableSkillCommands(on)),
                    "light" => Some(SettingsChange::Light(on)),
                    "plugin-tools" => Some(SettingsChange::EnablePluginTools(on)),
                    "plugin-install-scope" => {
                        PluginInstallScope::parse(value).map(SettingsChange::PluginInstallScope)
                    }
                    // Display only: each toggle in the submenu applied itself.
                    "platform" => None,
                    "steering-mode" => QueueMode::parse(value).map(SettingsChange::SteeringMode),
                    "follow-up-mode" => QueueMode::parse(value).map(SettingsChange::FollowUpMode),
                    "transport" => Some(SettingsChange::Transport(value.to_string())),
                    "hide-thinking" => Some(SettingsChange::HideThinkingBlock(on)),
                    "collapse-changelog" => Some(SettingsChange::CollapseChangelog(on)),
                    "quiet-startup" => Some(SettingsChange::QuietStartup(on)),
                    "tips" => Some(SettingsChange::TipsEnabled(on)),
                    "install-telemetry" => Some(SettingsChange::EnableInstallTelemetry(on)),
                    "double-escape-action" => {
                        DoubleEscapeAction::parse(value).map(SettingsChange::DoubleEscapeAction)
                    }
                    "tree-filter-mode" => {
                        TreeFilterMode::parse(value).map(SettingsChange::TreeFilterMode)
                    }
                    "show-hardware-cursor" => Some(SettingsChange::ShowHardwareCursor(on)),
                    "editor-border" => EditorBorder::parse(value).map(SettingsChange::EditorBorder),
                    "editor-padding" => Some(SettingsChange::EditorPaddingX(n)),
                    "autocomplete-max-visible" => Some(SettingsChange::AutocompleteMaxVisible(n)),
                    "clear-on-shrink" => Some(SettingsChange::ClearOnShrink(on)),
                    "terminal-progress" => Some(SettingsChange::ShowTerminalProgress(on)),
                    "voice-silence-ms" => Some(SettingsChange::VoiceSilenceMs(n)),
                    "webtools-timeout-secs" => Some(SettingsChange::WebtoolsTimeoutSecs(n)),
                    // The /learn rows are keyed by their settings.json name.
                    _ => LearnSettingKey::parse(id).map(|key| SettingsChange::LearnSetting(key, n)),
                };
                if let Some(change) = change {
                    emit(&callback, change);
                }
                // Re-measuring after every change is cheaper than knowing
                // which rows move the surface.
                surface.refresh();
            })
        };

        // A category: a row whose submenu lists some of the leaves.
        let category_row = |id: &str, label: &str, description: &str, ids: Vec<String>| -> Leaf {
            let members: Vec<Leaf> = {
                let leaves = leaves.borrow();
                ids.iter()
                    .filter_map(|id| leaves.iter().find(|l| &l.id == id).cloned())
                    .collect()
            };
            let n = members.len();
            // The member labels ride along as search text, so searching for a
            // setting lands on the category that holds it.
            let keywords = members
                .iter()
                .map(|m| m.label.clone())
                .collect::<Vec<_>>()
                .join(" ");
            let leaves = leaves.clone();
            let apply_change = apply_change.clone();
            Leaf::opens(
                id,
                label,
                description,
                format!("{n} setting{}", if n == 1 { "" } else { "s" }),
                Rc::new(move |_current: &str, slot: DoneSlot| {
                    let items: Vec<SettingItem> = {
                        let leaves = leaves.borrow();
                        ids.iter()
                            .filter_map(|id| leaves.iter().find(|l| &l.id == id))
                            .map(Leaf::to_item)
                            .collect()
                    };
                    let max = items.len().min(10);
                    let leaves = leaves.clone();
                    let apply_change = apply_change.clone();
                    let list = new_list(
                        items,
                        max,
                        true,
                        move |id, value| {
                            // The shared row keeps the value for the next opening.
                            if let Some(leaf) = leaves.borrow_mut().iter_mut().find(|l| l.id == id)
                            {
                                leaf.current_value = value.to_string();
                            }
                            apply_change(id, value);
                        },
                        move || done(&slot, None),
                    );
                    ListSubmenu::new(list).handle()
                }),
            )
            .keywords(keywords)
        };
        let ids = |ids: &[&str]| -> Vec<String> { strings(ids) };

        top.push(category_row(
            "cat-context",
            "Context",
            "What the model is sent and how much of it: compaction, superseded reads, and the low-token preset.",
            ids(&["autocompact", "context-gc", "light"]),
        ));
        top.push(category_row(
            "cat-behavior",
            "Behavior",
            "Agent and session behavior: steering, follow-up, thinking, escape, tree filter, transport.",
            ids(&[
                "steering-mode",
                "follow-up-mode",
                "thinking",
                "double-escape-action",
                "tree-filter-mode",
                "transport",
            ]),
        ));
        top.push(category_row(
            "cat-interface",
            "Interface",
            "Appearance and editor: theme, thinking visibility, cursor, border, padding, autocomplete, terminal.",
            ids(&[
                "theme",
                "hide-thinking",
                "show-hardware-cursor",
                "editor-border",
                "editor-padding",
                "autocomplete-max-visible",
                "clear-on-shrink",
                "terminal-progress",
            ]),
        ));
        top.push(category_row(
            "cat-plugins",
            "Plugins",
            &format!("The autonomous plugin system's master switch, the vendor layout {APP_NAME} writes, and where autonomous installs land."),
            ids(&["plugin-tools", "platform", "plugin-install-scope"]),
        ));
        top.push(category_row(
            "cat-images",
            "Images",
            "Inline image rendering and resizing.",
            ids(&[
                "show-images",
                "image-width-cells",
                "auto-resize-images",
                "block-images",
            ]),
        ));
        top.push(category_row(
            "cat-learn",
            "Learning",
            "Thresholds /learn mines sessions with: how far back to look, and how often something must repeat.",
            LEARN_SETTINGS.iter().map(|(k, ..)| k.as_str().to_string()).collect(),
        ));
        {
            let statuses = config.external_tools.clone();
            let keywords = config
                .external_tools
                .iter()
                .flat_map(|s| {
                    [s.doc.tool, s.doc.label]
                        .into_iter()
                        .chain(s.doc.settings_keys.iter().copied())
                })
                .collect::<Vec<_>>()
                .join(" ");
            top.push(
                Leaf::opens(
                    "cat-external",
                    "External tools",
                    format!(
                        "Optional Rust binaries that expand what {APP_NAME} can do: {}. {APP_NAME} runs without every one of them - each row says what it adds and what happens without it.",
                        external_summary(&config.external_tools)
                    ),
                    external_count(&config.external_tools),
                    Rc::new(move |_current: &str, slot: DoneSlot| {
                        external_tools_submenu(&statuses, slot)
                    }),
                )
                .keywords(keywords),
            );
        }
        top.push(category_row(
            "cat-advanced",
            "Advanced",
            "Startup, tips, telemetry, skills, warnings, voice, and web tools.",
            ids(&[
                "quiet-startup",
                "tips",
                "collapse-changelog",
                "install-telemetry",
                "skill-commands",
                "warnings",
                "voice-silence-ms",
                "webtools-timeout-secs",
            ]),
        ));

        let items: Vec<SettingItem> = top.iter().map(Leaf::to_item).collect();
        let max = items.len().min(10);
        let cancel_callback = callback.clone();
        let list = new_list(
            items,
            max,
            true,
            move |id, value| apply_change(id, value),
            move || emit(&cancel_callback, SettingsChange::Cancel),
        );

        let mut frame = InputFrame::new(InputFrameOptions::default());
        frame.set_title("settings");
        frame.add_child(list.clone());
        if let Some(text) = surface_text {
            frame.add_child(text);
        }
        Self {
            frame,
            surface,
            list,
        }
    }

    /// Re-price the per-turn surface. A host that applies the pane's changes
    /// after the callback returns (rather than inside it) calls this once they
    /// are applied, since the pane's own re-measure ran before them.
    pub fn refresh_token_surface(&self) {
        self.surface.refresh();
    }

    /// `getSettingsList()`: the list the keyboard goes to.
    pub fn settings_list(&self) -> Rc<RefCell<SettingsList>> {
        self.list.clone()
    }
}

impl Component for SettingsSelectorComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.list.borrow_mut().handle_input(data);
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_are_grouped() {
        assert_eq!(token_count(0), "0");
        assert_eq!(token_count(849), "849");
        assert_eq!(token_count(2710), "2,710");
        assert_eq!(token_count(1234567), "1,234,567");
    }

    #[test]
    fn a_hand_set_cap_joins_the_byte_presets() {
        assert_eq!(bytes_to_label(8192), "8 KB");
        assert_eq!(bytes_to_label(40 * 1024), "40 KB");
        assert_eq!(
            byte_labels(40 * 1024),
            strings(&["8 KB", "16 KB", "32 KB", "40 KB", "64 KB", "128 KB"])
        );
    }
}
