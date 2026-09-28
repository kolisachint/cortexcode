//! Key text for hint lines (`modes/interactive/components/keybinding-hints.ts`).

use cortexcode_code_tui_theme::theme;
use cortexcode_tui_keys::{get_keybindings, matches_key};

use crate::keybindings::keybinding;

/// On macOS the alt key is labelled Option.
fn format_key_part(part: &str, capitalize: bool) -> String {
    let display = if cfg!(target_os = "macos") && part.eq_ignore_ascii_case("alt") {
        "option"
    } else {
        part
    };
    if !capitalize {
        return display.to_string();
    }
    let mut chars = display.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `formatKeyText`: `a/b` alternatives, `+`-joined chords. A bare
/// single-character key prints exactly as typed (an uppercase letter would
/// name `shift+<letter>`); inside a chord `capitalize` capitalizes each part.
pub fn format_key_text(key: &str, capitalize: bool) -> String {
    key.split('/')
        .map(|k| {
            let parts: Vec<&str> = k.split('+').collect();
            if parts.len() == 1 && parts[0].encode_utf16().count() == 1 {
                return parts[0].to_string();
            }
            parts
                .iter()
                .map(|part| format_key_part(part, capitalize))
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn format_keys(keys: &[String], capitalize: bool) -> String {
    if keys.is_empty() {
        return String::new();
    }
    format_key_text(&keys.join("/"), capitalize)
}

/// `keyText`: every key bound to `id`, as typed.
pub fn key_text(id: &str) -> String {
    format_keys(&get_keybindings().get_keys(id), false)
}

/// `keyDisplayText`: every key bound to `id`, capitalized.
pub fn key_display_text(id: &str) -> String {
    format_keys(&get_keybindings().get_keys(id), true)
}

/// `keyDisplayLabel`: the taught (first) key only, capitalized.
pub fn key_display_label(id: &str) -> String {
    get_keybindings()
        .get_keys(id)
        .first()
        .map(|k| format_key_text(k, true))
        .unwrap_or_default()
}

/// `keyHint`: `<keys> <description>` in dim/muted.
pub fn key_hint(id: &str, description: &str) -> String {
    let t = theme();
    t.fg("dim", &key_text(id)) + &t.fg("muted", &format!(" {description}"))
}

/// `rawKeyHint`: a hint for a literal key.
pub fn raw_key_hint(key: &str, description: &str) -> String {
    let t = theme();
    t.fg("dim", &format_key_text(key, false)) + &t.fg("muted", &format!(" {description}"))
}

fn app_default_keys(id: &str) -> Vec<&'static str> {
    keybinding(id).map(|e| e.default_keys).unwrap_or_default()
}

/// `matchesAppKey`: match against the installed manager when it knows `id`,
/// else against the app definition's default keys (so a component never goes
/// dead under the bare library defaults).
pub fn matches_app_key(data: &str, id: &str) -> bool {
    let keybindings = get_keybindings();
    if keybindings.get_definition(id).is_some() {
        return keybindings.matches(data, id);
    }
    app_default_keys(id)
        .iter()
        .any(|key| matches_key(data, key))
}

/// `appKeyLabel`: the first configured key for an app binding.
pub fn app_key_label(id: &str) -> String {
    let keybindings = get_keybindings();
    if keybindings.get_definition(id).is_some() {
        if let Some(first) = keybindings.get_keys(id).into_iter().next() {
            return first;
        }
    }
    app_default_keys(id)
        .first()
        .map(|k| k.to_string())
        .unwrap_or_default()
}
