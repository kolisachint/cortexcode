//! The keyboard map (`core/keybindings.ts`).
//!
//! Bindings are grouped by intention — Flow, Compose, Steer, Read, Screen,
//! Scroll, Go, then the overlays that print their own keys — and the
//! declaration order *is* the grouping: `keybindings.json` is written in it.
//! The layout rules (alt sets a value, ctrl acts on what is drawn, a dial
//! steps back with shift, no bare `shift+<letter>`) are held by the
//! `keybinding_layout` tests; the rationale for each key is in hoocode's
//! `core/keybindings.ts`.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use cortexcode_tui_keys::{KeybindingDefinition, KeybindingsManager, TUI_KEYBINDINGS};
use serde_json::{Map, Value};

/// One binding: id, default keys, description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingEntry {
    pub id: &'static str,
    pub default_keys: Vec<&'static str>,
    pub description: &'static str,
}

fn entry(id: &'static str, keys: &[&'static str], description: &'static str) -> KeybindingEntry {
    KeybindingEntry {
        id,
        default_keys: keys.to_vec(),
        description,
    }
}

/// The app bindings, in declaration order (after the TUI library's).
pub fn app_keybindings() -> Vec<KeybindingEntry> {
    let windows = cfg!(windows);
    vec![
        // ── Flow — getting out, getting back
        entry("app.interrupt", &["escape"], "Cancel or abort"),
        entry("app.clear", &["ctrl+c"], "Clear editor"),
        entry("app.exit", &["ctrl+d"], "Exit when editor is empty"),
        entry(
            "app.suspend",
            if windows { &[] } else { &["ctrl+z"] },
            "Suspend to background",
        ),
        // ── Compose — the message in your hands
        entry("app.editor.external", &["alt+e"], "Open external editor"),
        entry(
            "app.input.voiceTranscribe",
            &["alt+r"],
            "Record voice and transcribe into the editor",
        ),
        entry(
            "app.clipboard.pasteImage",
            if windows { &["alt+v"] } else { &["ctrl+v"] },
            "Paste image from clipboard",
        ),
        entry(
            "app.clipboard.copyMessage",
            &["alt+q"],
            "Copy the agent's last message to the clipboard",
        ),
        entry(
            "app.message.followUp",
            &["alt+enter"],
            "Queue follow-up message",
        ),
        entry(
            "app.message.dequeue",
            &["alt+up"],
            "Restore queued messages",
        ),
        // ── Steer — what the agent is before it runs
        entry(
            "app.mode.cycleForward",
            &["alt+a"],
            "Cycle agent mode (ask → plan → build → debug)",
        ),
        entry(
            "app.mode.cycleBackward",
            &["shift+alt+a"],
            "Cycle agent mode backward",
        ),
        entry("app.model.cycleForward", &["alt+m"], "Cycle to next model"),
        entry(
            "app.model.cycleBackward",
            &["shift+alt+m"],
            "Cycle to previous model",
        ),
        entry("app.model.select", &[], "Open model selector"),
        entry(
            "app.thinking.cycleForward",
            &["alt+t", "shift+tab"],
            "Cycle thinking level (off → … → high)",
        ),
        entry(
            "app.thinking.cycleBackward",
            &["shift+alt+t"],
            "Cycle thinking level backward",
        ),
        // ── Read — what you see of what it did
        entry(
            "app.view.cycleForward",
            &["alt+o"],
            "Cycle tool output view (radar → peek → full)",
        ),
        entry(
            "app.view.cycleBackward",
            &["shift+alt+o"],
            "Cycle tool output view backward",
        ),
        entry(
            "app.tools.expand",
            &["ctrl+o"],
            "Jump to the full view from wherever you are, and back again",
        ),
        entry("app.thinking.toggle", &["ctrl+t"], "Toggle thinking blocks"),
        entry(
            "app.tasks.cycleForward",
            &["alt+l"],
            "Cycle task panel view (tasks → subagents → teams, skips empty lenses)",
        ),
        entry(
            "app.tasks.cycleBackward",
            &["shift+alt+l"],
            "Cycle task panel view backward",
        ),
        entry(
            "app.team.focus",
            &["alt+n"],
            "Focus the team roster (navigate roles, n nudge, a attach)",
        ),
        // ── Screen — how much room there is
        entry(
            "app.chrome.cycleForward",
            &["alt+z"],
            "Cycle chrome density (full → compact → bare)",
        ),
        entry(
            "app.chrome.cycleBackward",
            &["shift+alt+z"],
            "Cycle chrome density backward",
        ),
        // ── Scroll — where in the transcript you are looking
        entry(
            "app.scroll.pageUp",
            &["pageUp"],
            "Scroll the transcript up a page (pins the view)",
        ),
        entry(
            "app.scroll.pageDown",
            &["pageDown"],
            "Scroll the transcript down a page",
        ),
        entry(
            "app.scroll.top",
            &["ctrl+home"],
            "Jump to the start of the transcript",
        ),
        entry(
            "app.scroll.bottom",
            &["ctrl+end"],
            "Jump back to live output",
        ),
        entry("app.scroll.search", &["ctrl+r"], "Search the transcript"),
        entry(
            "app.scroll.previousMessage",
            &["ctrl+up"],
            "Jump to your previous message",
        ),
        entry(
            "app.scroll.nextMessage",
            &["ctrl+down"],
            "Jump to your next message",
        ),
        // ── Go — sessions and places
        entry(
            "app.session.resume",
            &["alt+h"],
            "Resume a session from history",
        ),
        entry("app.session.tree", &[], "Open session tree"),
        entry("app.session.new", &[], "Start a new session"),
        entry("app.session.fork", &[], "Fork current session"),
        entry(
            "app.session.changeDirectory",
            &["alt+w"],
            "Change working directory (move to another repo without quitting)",
        ),
        entry(
            "app.session.color.cycleForward",
            &["alt+c"],
            "Cycle the session chip's color",
        ),
        entry(
            "app.session.color.cycleBackward",
            &["shift+alt+c"],
            "Cycle the session chip's color backward",
        ),
        entry("app.settings.open", &["alt+s"], "Open settings"),
        entry("app.hotkeys.open", &["alt+k"], "Show keyboard shortcuts"),
        // ── Overlays — live only while their surface is open
        entry(
            "app.team.nudge",
            &["n"],
            "Nudge the selected team role (team focus mode)",
        ),
        entry(
            "app.team.attach",
            &["a"],
            "Attach to the selected team role (team focus mode)",
        ),
        entry(
            "app.scroll.lineUp",
            &["up"],
            "Scroll up a line (pinned view)",
        ),
        entry(
            "app.scroll.lineDown",
            &["down"],
            "Scroll down a line (pinned view)",
        ),
        entry(
            "app.scroll.exit",
            &["escape"],
            "Leave the pinned view and follow live output",
        ),
        entry(
            "app.scroll.searchInView",
            &["/"],
            "Search the transcript (pinned view)",
        ),
        entry(
            "app.scroll.searchNext",
            &["n"],
            "Next search match (pinned view)",
        ),
        entry(
            "app.scroll.searchPrevious",
            &["p"],
            "Previous search match (pinned view)",
        ),
        entry(
            "app.options.next",
            &["right"],
            "Confirm and advance to the next question",
        ),
        entry(
            "app.options.back",
            &["left"],
            "Go back to the previous question",
        ),
        entry(
            "app.session.togglePath",
            &["alt+p"],
            "Toggle session path display",
        ),
        entry(
            "app.session.toggleSort",
            &["alt+o"],
            "Toggle session sort order",
        ),
        entry(
            "app.session.toggleNamedFilter",
            &["alt+n"],
            "Toggle named session filter",
        ),
        entry("app.session.rename", &["alt+r"], "Rename session"),
        entry("app.session.delete", &["alt+x"], "Delete session"),
        entry(
            "app.session.deleteNoninvasive",
            &["ctrl+backspace"],
            "Delete session when query is empty",
        ),
        entry("app.models.save", &["alt+s"], "Save model selection"),
        entry("app.models.enableAll", &["alt+a"], "Enable all models"),
        entry("app.models.clearAll", &["alt+x"], "Clear all models"),
        entry(
            "app.models.toggleProvider",
            &["alt+g"],
            "Toggle all models for provider",
        ),
        entry(
            "app.models.reorderUp",
            &["alt+up"],
            "Move model up in order",
        ),
        entry(
            "app.models.reorderDown",
            &["alt+down"],
            "Move model down in order",
        ),
        entry(
            "app.tree.foldOrUp",
            &["ctrl+left", "alt+left"],
            "Fold tree branch or move up",
        ),
        entry(
            "app.tree.unfoldOrDown",
            &["ctrl+right", "alt+right"],
            "Unfold tree branch or move down",
        ),
        entry("app.tree.editLabel", &["alt+l"], "Edit tree label"),
        entry(
            "app.tree.toggleLabelTimestamp",
            &["alt+t"],
            "Toggle tree label timestamps",
        ),
        entry(
            "app.tree.filter.default",
            &["alt+1"],
            "Tree filter: default view",
        ),
        entry(
            "app.tree.filter.noTools",
            &["alt+2"],
            "Tree filter: hide tool results",
        ),
        entry(
            "app.tree.filter.userOnly",
            &["alt+3"],
            "Tree filter: user messages only",
        ),
        entry(
            "app.tree.filter.labeledOnly",
            &["alt+4"],
            "Tree filter: labeled entries only",
        ),
        entry(
            "app.tree.filter.all",
            &["alt+5"],
            "Tree filter: show all entries",
        ),
        entry(
            "app.tree.filter.cycleForward",
            &["alt+c"],
            "Tree filter: cycle forward",
        ),
        entry(
            "app.tree.filter.cycleBackward",
            &["shift+alt+c"],
            "Tree filter: cycle backward",
        ),
    ]
}

/// `KEYBINDINGS`: the TUI library's bindings, then the app's, in declaration
/// order.
pub fn keybindings() -> Vec<KeybindingEntry> {
    let mut all: Vec<KeybindingEntry> = TUI_KEYBINDINGS
        .iter()
        .map(|(id, keys, description)| entry(id, keys, description))
        .collect();
    all.extend(app_keybindings());
    all
}

/// The definition of one binding, if it exists.
pub fn keybinding(id: &str) -> Option<KeybindingEntry> {
    keybindings().into_iter().find(|e| e.id == id)
}

/// [`keybindings`] as the TUI manager's definition map.
pub fn keybinding_definitions() -> HashMap<String, KeybindingDefinition> {
    keybindings()
        .into_iter()
        .map(|e| {
            (
                e.id.to_string(),
                KeybindingDefinition::new(&e.default_keys, e.description),
            )
        })
        .collect()
}

/// Legacy (un-namespaced or renamed) ids and what each is called now.
pub const KEYBINDING_NAME_MIGRATIONS: [(&str, &str); 62] = [
    ("cursorUp", "tui.editor.cursorUp"),
    ("cursorDown", "tui.editor.cursorDown"),
    ("cursorLeft", "tui.editor.cursorLeft"),
    ("cursorRight", "tui.editor.cursorRight"),
    ("cursorWordLeft", "tui.editor.cursorWordLeft"),
    ("cursorWordRight", "tui.editor.cursorWordRight"),
    ("cursorLineStart", "tui.editor.cursorLineStart"),
    ("cursorLineEnd", "tui.editor.cursorLineEnd"),
    ("jumpForward", "tui.editor.jumpForward"),
    ("jumpBackward", "tui.editor.jumpBackward"),
    ("pageUp", "tui.editor.pageUp"),
    ("pageDown", "tui.editor.pageDown"),
    ("deleteCharBackward", "tui.editor.deleteCharBackward"),
    ("deleteCharForward", "tui.editor.deleteCharForward"),
    ("deleteWordBackward", "tui.editor.deleteWordBackward"),
    ("deleteWordForward", "tui.editor.deleteWordForward"),
    ("deleteToLineStart", "tui.editor.deleteToLineStart"),
    ("deleteToLineEnd", "tui.editor.deleteToLineEnd"),
    ("yank", "tui.editor.yank"),
    ("yankPop", "tui.editor.yankPop"),
    ("undo", "tui.editor.undo"),
    ("newLine", "tui.input.newLine"),
    ("submit", "tui.input.submit"),
    ("tab", "tui.input.tab"),
    ("copy", "tui.input.copy"),
    ("selectUp", "tui.select.up"),
    ("selectDown", "tui.select.down"),
    ("selectPageUp", "tui.select.pageUp"),
    ("selectPageDown", "tui.select.pageDown"),
    ("selectConfirm", "tui.select.confirm"),
    ("selectCancel", "tui.select.cancel"),
    ("app.thinking.cycle", "app.thinking.cycleForward"),
    ("app.mode.cycle", "app.mode.cycleForward"),
    ("app.tasks.cycleView", "app.tasks.cycleForward"),
    ("interrupt", "app.interrupt"),
    ("clear", "app.clear"),
    ("exit", "app.exit"),
    ("suspend", "app.suspend"),
    ("cycleThinkingLevel", "app.thinking.cycleForward"),
    ("cycleModelForward", "app.model.cycleForward"),
    ("cycleModelBackward", "app.model.cycleBackward"),
    ("selectModel", "app.model.select"),
    ("expandTools", "app.tools.expand"),
    ("toggleThinking", "app.thinking.toggle"),
    ("toggleSessionNamedFilter", "app.session.toggleNamedFilter"),
    ("externalEditor", "app.editor.external"),
    ("followUp", "app.message.followUp"),
    ("dequeue", "app.message.dequeue"),
    ("pasteImage", "app.clipboard.pasteImage"),
    ("newSession", "app.session.new"),
    ("tree", "app.session.tree"),
    ("fork", "app.session.fork"),
    ("resume", "app.session.resume"),
    ("treeFoldOrUp", "app.tree.foldOrUp"),
    ("treeUnfoldOrDown", "app.tree.unfoldOrDown"),
    ("treeEditLabel", "app.tree.editLabel"),
    ("treeToggleLabelTimestamp", "app.tree.toggleLabelTimestamp"),
    ("toggleSessionPath", "app.session.togglePath"),
    ("toggleSessionSort", "app.session.toggleSort"),
    ("renameSession", "app.session.rename"),
    ("deleteSession", "app.session.delete"),
    ("deleteSessionNoninvasive", "app.session.deleteNoninvasive"),
];

fn migrated_name(key: &str) -> Option<&'static str> {
    KEYBINDING_NAME_MIGRATIONS
        .iter()
        .find(|(old, _)| *old == key)
        .map(|(_, new)| *new)
}

/// `toKeybindingsConfig`: keep string and string-array bindings; a string
/// becomes a one-key list.
pub fn to_keybindings_config(value: &Value) -> HashMap<String, Vec<String>> {
    let Value::Object(map) = value else {
        return HashMap::new();
    };
    let mut config = HashMap::new();
    for (key, binding) in map {
        match binding {
            Value::String(s) => {
                config.insert(key.clone(), vec![s.clone()]);
            }
            Value::Array(items) if items.iter().all(Value::is_string) => {
                config.insert(
                    key.clone(),
                    items
                        .iter()
                        .map(|v| v.as_str().unwrap_or_default().to_string())
                        .collect(),
                );
            }
            _ => {}
        }
    }
    config
}

/// `migrateKeybindingsConfig`: rename legacy ids (a namespaced value wins
/// over its legacy twin) and order the result.
pub fn migrate_keybindings_config(raw: &Map<String, Value>) -> (Map<String, Value>, bool) {
    let mut config = Map::new();
    let mut migrated = false;
    for (key, value) in raw {
        let next_key = migrated_name(key).unwrap_or(key);
        if next_key != key {
            migrated = true;
            if raw.contains_key(next_key) {
                continue;
            }
        }
        config.insert(next_key.to_string(), value.clone());
    }
    (order_keybindings_config(&config), migrated)
}

/// `orderKeybindingsConfig`: known ids in declaration order, then the rest
/// sorted.
pub fn order_keybindings_config(config: &Map<String, Value>) -> Map<String, Value> {
    let mut ordered = Map::new();
    for e in keybindings() {
        if let Some(v) = config.get(e.id) {
            ordered.insert(e.id.to_string(), v.clone());
        }
    }
    let mut extras: Vec<&String> = config
        .keys()
        .filter(|k| !ordered.contains_key(*k))
        .collect();
    extras.sort();
    for key in extras {
        ordered.insert(key.clone(), config[key].clone());
    }
    ordered
}

fn load_raw_config(path: &Path) -> Option<Map<String, Value>> {
    let content = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&content).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn load_from_file(path: &Path) -> HashMap<String, Vec<String>> {
    let Some(raw) = load_raw_config(path) else {
        return HashMap::new();
    };
    to_keybindings_config(&Value::Object(migrate_keybindings_config(&raw).0))
}

/// The keybindings file migration from `migrations.ts`: rewrite legacy ids in
/// `<agent_dir>/keybindings.json` in place. Malformed files are left alone.
pub fn migrate_keybindings_config_file(agent_dir: &Path) {
    let path = agent_dir.join("keybindings.json");
    let Some(raw) = load_raw_config(&path) else {
        return;
    };
    let (config, migrated) = migrate_keybindings_config(&raw);
    if !migrated {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(&Value::Object(config)) {
        let _ = std::fs::write(&path, format!("{text}\n"));
    }
}

/// The app's keybindings manager: the TUI manager over [`keybindings`], plus
/// the `keybindings.json` it was loaded from.
#[derive(Debug, Clone)]
pub struct AppKeybindingsManager {
    manager: KeybindingsManager,
    config_path: Option<PathBuf>,
}

impl AppKeybindingsManager {
    pub fn new(user_bindings: HashMap<String, Vec<String>>, config_path: Option<PathBuf>) -> Self {
        Self {
            manager: KeybindingsManager::new(keybinding_definitions(), user_bindings),
            config_path,
        }
    }

    /// `KeybindingsManager.create`: load `<agent_dir>/keybindings.json`
    /// (legacy names migrated in memory).
    pub fn create(agent_dir: Option<&Path>) -> Self {
        let agent_dir = agent_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(cortexcode_code_paths::agent_dir);
        let config_path = agent_dir.join("keybindings.json");
        let user_bindings = load_from_file(&config_path);
        Self::new(user_bindings, Some(config_path))
    }

    /// Re-read the config file.
    pub fn reload(&mut self) {
        if let Some(path) = &self.config_path {
            let bindings = load_from_file(path);
            self.manager.set_user_bindings(bindings);
        }
    }

    /// `getEffectiveConfig`: every binding's resolved keys.
    pub fn get_effective_config(&self) -> HashMap<String, Vec<String>> {
        self.manager.get_resolved_bindings()
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// Install as the manager library components consult.
    pub fn install(&self) {
        cortexcode_tui_keys::set_keybindings(self.manager.clone());
    }

    pub fn into_manager(self) -> KeybindingsManager {
        self.manager
    }
}

impl Default for AppKeybindingsManager {
    fn default() -> Self {
        Self::new(HashMap::new(), None)
    }
}

impl Deref for AppKeybindingsManager {
    type Target = KeybindingsManager;
    fn deref(&self) -> &KeybindingsManager {
        &self.manager
    }
}

impl DerefMut for AppKeybindingsManager {
    fn deref_mut(&mut self) -> &mut KeybindingsManager {
        &mut self.manager
    }
}
