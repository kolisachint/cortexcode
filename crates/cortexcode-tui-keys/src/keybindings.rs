//! Keybinding registry: default keybindings, user overrides, and conflict
//! detection.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `keybindings.ts`.

use std::collections::{HashMap, HashSet};

use crate::matching::matches_key;

/// A single keybinding's default key(s) and description.
#[derive(Debug, Clone)]
pub struct KeybindingDefinition {
    pub default_keys: Vec<String>,
    pub description: String,
}

impl KeybindingDefinition {
    pub fn new(default_keys: &[&str], description: &str) -> Self {
        Self {
            default_keys: default_keys.iter().map(|s| s.to_string()).collect(),
            description: description.to_string(),
        }
    }
}

/// A key claimed by more than one keybinding in the user's config.
#[derive(Debug, Clone)]
pub struct KeybindingConflict {
    pub key: String,
    pub keybindings: Vec<String>,
}

fn normalize_keys(keys: Option<&[String]>) -> Vec<String> {
    let Some(keys) = keys else { return Vec::new() };
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for key in keys {
        if seen.insert(key.clone()) {
            result.push(key.clone());
        }
    }
    result
}

/// Resolves a set of keybinding definitions against user overrides, with
/// conflict detection.
pub struct KeybindingsManager {
    definitions: HashMap<String, KeybindingDefinition>,
    user_bindings: HashMap<String, Vec<String>>,
    keys_by_id: HashMap<String, Vec<String>>,
    conflicts: Vec<KeybindingConflict>,
}

impl KeybindingsManager {
    pub fn new(
        definitions: HashMap<String, KeybindingDefinition>,
        user_bindings: HashMap<String, Vec<String>>,
    ) -> Self {
        let mut manager = Self {
            definitions,
            user_bindings,
            keys_by_id: HashMap::new(),
            conflicts: Vec::new(),
        };
        manager.rebuild();
        manager
    }

    fn rebuild(&mut self) {
        self.keys_by_id.clear();
        self.conflicts.clear();

        let mut user_claims: HashMap<String, HashSet<String>> = HashMap::new();
        for (keybinding, keys) in &self.user_bindings {
            if !self.definitions.contains_key(keybinding) {
                continue;
            }
            for key in normalize_keys(Some(keys)) {
                user_claims
                    .entry(key)
                    .or_default()
                    .insert(keybinding.clone());
            }
        }

        for (key, keybindings) in &user_claims {
            if keybindings.len() > 1 {
                let mut keybindings: Vec<String> = keybindings.iter().cloned().collect();
                keybindings.sort();
                self.conflicts.push(KeybindingConflict {
                    key: key.clone(),
                    keybindings,
                });
            }
        }

        let ids: Vec<String> = self.definitions.keys().cloned().collect();
        for id in ids {
            let definition = &self.definitions[&id];
            let keys = match self.user_bindings.get(&id) {
                Some(user_keys) => normalize_keys(Some(user_keys)),
                None => normalize_keys(Some(&definition.default_keys)),
            };
            self.keys_by_id.insert(id, keys);
        }
    }

    /// Whether raw terminal input `data` matches any key bound to `keybinding`.
    pub fn matches(&self, data: &str, keybinding: &str) -> bool {
        self.keys_by_id
            .get(keybinding)
            .is_some_and(|keys| keys.iter().any(|key| matches_key(data, key)))
    }

    pub fn get_keys(&self, keybinding: &str) -> Vec<String> {
        self.keys_by_id.get(keybinding).cloned().unwrap_or_default()
    }

    pub fn get_definition(&self, keybinding: &str) -> Option<&KeybindingDefinition> {
        self.definitions.get(keybinding)
    }

    pub fn get_conflicts(&self) -> &[KeybindingConflict] {
        &self.conflicts
    }

    pub fn set_user_bindings(&mut self, user_bindings: HashMap<String, Vec<String>>) {
        self.user_bindings = user_bindings;
        self.rebuild();
    }

    pub fn get_user_bindings(&self) -> &HashMap<String, Vec<String>> {
        &self.user_bindings
    }

    pub fn get_resolved_bindings(&self) -> HashMap<String, Vec<String>> {
        self.definitions
            .keys()
            .map(|id| {
                (
                    id.clone(),
                    self.keys_by_id.get(id).cloned().unwrap_or_default(),
                )
            })
            .collect()
    }
}

/// The built-in TUI keybinding definitions (editor navigation/editing,
/// generic input actions, and generic selection actions).
pub fn default_tui_keybindings() -> HashMap<String, KeybindingDefinition> {
    let entries: &[(&str, &[&str], &str)] = &[
        ("tui.editor.cursorUp", &["up"], "Move cursor up"),
        ("tui.editor.cursorDown", &["down"], "Move cursor down"),
        (
            "tui.editor.cursorLeft",
            &["left", "ctrl+b"],
            "Move cursor left",
        ),
        (
            "tui.editor.cursorRight",
            &["right", "ctrl+f"],
            "Move cursor right",
        ),
        (
            "tui.editor.cursorWordLeft",
            &["alt+left", "ctrl+left", "alt+b"],
            "Move cursor word left",
        ),
        (
            "tui.editor.cursorWordRight",
            &["alt+right", "ctrl+right", "alt+f"],
            "Move cursor word right",
        ),
        (
            "tui.editor.cursorLineStart",
            &["home", "ctrl+a"],
            "Move to line start",
        ),
        (
            "tui.editor.cursorLineEnd",
            &["end", "ctrl+e"],
            "Move to line end",
        ),
        (
            "tui.editor.jumpForward",
            &["ctrl+]"],
            "Jump forward to character",
        ),
        (
            "tui.editor.jumpBackward",
            &["ctrl+alt+]"],
            "Jump backward to character",
        ),
        ("tui.editor.pageUp", &["pageUp"], "Page up"),
        ("tui.editor.pageDown", &["pageDown"], "Page down"),
        (
            "tui.editor.deleteCharBackward",
            &["backspace"],
            "Delete character backward",
        ),
        (
            "tui.editor.deleteCharForward",
            &["delete", "ctrl+d"],
            "Delete character forward",
        ),
        (
            "tui.editor.deleteWordBackward",
            &["ctrl+w", "alt+backspace"],
            "Delete word backward",
        ),
        (
            "tui.editor.deleteWordForward",
            &["alt+d", "alt+delete"],
            "Delete word forward",
        ),
        (
            "tui.editor.deleteToLineStart",
            &["ctrl+u"],
            "Delete to line start",
        ),
        (
            "tui.editor.deleteToLineEnd",
            &["ctrl+k"],
            "Delete to line end",
        ),
        ("tui.editor.yank", &["ctrl+y"], "Yank"),
        ("tui.editor.yankPop", &["alt+y"], "Yank pop"),
        ("tui.editor.undo", &["ctrl+-"], "Undo"),
        ("tui.input.newLine", &["shift+enter"], "Insert newline"),
        ("tui.input.submit", &["enter"], "Submit input"),
        ("tui.input.tab", &["tab"], "Tab / autocomplete"),
        ("tui.input.copy", &["ctrl+c"], "Copy selection"),
        ("tui.select.up", &["up"], "Move selection up"),
        ("tui.select.down", &["down"], "Move selection down"),
        ("tui.select.pageUp", &["pageUp"], "Selection page up"),
        ("tui.select.pageDown", &["pageDown"], "Selection page down"),
        ("tui.select.confirm", &["enter"], "Confirm selection"),
        (
            "tui.select.cancel",
            &["escape", "ctrl+c"],
            "Cancel selection",
        ),
    ];
    entries
        .iter()
        .map(|(id, keys, desc)| (id.to_string(), KeybindingDefinition::new(keys, desc)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> KeybindingsManager {
        KeybindingsManager::new(default_tui_keybindings(), HashMap::new())
    }

    #[test]
    fn test_default_bindings_match() {
        let m = manager();
        assert!(m.matches("\x1b[A", "tui.editor.cursorUp"));
        assert!(m.matches("\x02", "tui.editor.cursorLeft")); // ctrl+b
    }

    #[test]
    fn test_unknown_keybinding_matches_nothing() {
        let m = manager();
        assert!(!m.matches("\x1b[A", "not.a.real.binding"));
    }

    #[test]
    fn test_user_override_replaces_default() {
        let mut user = HashMap::new();
        user.insert(
            "tui.editor.cursorUp".to_string(),
            vec!["ctrl+p".to_string()],
        );
        let m = KeybindingsManager::new(default_tui_keybindings(), user);
        assert!(
            !m.matches("\x1b[A", "tui.editor.cursorUp"),
            "default key should no longer match"
        );
        assert!(
            m.matches("\x10", "tui.editor.cursorUp"),
            "overridden key should match"
        );
    }

    #[test]
    fn test_conflict_detection() {
        let mut user = HashMap::new();
        user.insert(
            "tui.editor.cursorUp".to_string(),
            vec!["ctrl+p".to_string()],
        );
        user.insert(
            "tui.editor.cursorDown".to_string(),
            vec!["ctrl+p".to_string()],
        );
        let m = KeybindingsManager::new(default_tui_keybindings(), user);
        let conflicts = m.get_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].key, "ctrl+p");
        assert_eq!(conflicts[0].keybindings.len(), 2);
    }

    #[test]
    fn test_get_keys_returns_defaults() {
        let m = manager();
        assert_eq!(
            m.get_keys("tui.editor.cursorLeft"),
            vec!["left".to_string(), "ctrl+b".to_string()]
        );
    }

    #[test]
    fn test_get_resolved_bindings_includes_all_definitions() {
        let m = manager();
        let resolved = m.get_resolved_bindings();
        assert_eq!(resolved.len(), default_tui_keybindings().len());
    }

    #[test]
    fn test_set_user_bindings_rebuilds() {
        let mut m = manager();
        assert!(m.matches("\x1b[A", "tui.editor.cursorUp"));
        let mut user = HashMap::new();
        user.insert(
            "tui.editor.cursorUp".to_string(),
            vec!["ctrl+p".to_string()],
        );
        m.set_user_bindings(user);
        assert!(!m.matches("\x1b[A", "tui.editor.cursorUp"));
        assert!(m.matches("\x10", "tui.editor.cursorUp"));
    }

    #[test]
    fn test_normalize_keys_dedupes() {
        let mut user = HashMap::new();
        user.insert(
            "tui.editor.cursorUp".to_string(),
            vec!["up".to_string(), "up".to_string()],
        );
        let m = KeybindingsManager::new(default_tui_keybindings(), user);
        assert_eq!(m.get_keys("tui.editor.cursorUp"), vec!["up".to_string()]);
    }
}
