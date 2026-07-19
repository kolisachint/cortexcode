//! Parse raw terminal input into a key identifier string (`"ctrl+c"`, `"escape"`, ...).
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `keys.ts` (`parseKey`).

use crate::constants::{
    arrow_codepoints, codepoints, functional_codepoints, is_symbol_key,
    is_windows_terminal_session, legacy_sequence_key_id, modifiers,
    normalize_kitty_functional_codepoint, normalize_shifted_letter_identity_codepoint, LOCK_MASK,
};
use crate::kitty::{parse_kitty_sequence, parse_modify_other_keys_sequence};
use crate::state::is_kitty_protocol_active;

fn format_key_name_with_modifiers(key_name: &str, modifier: u32) -> Option<String> {
    let effective_mod = modifier & !LOCK_MASK;
    let supported_mask = modifiers::SHIFT | modifiers::CTRL | modifiers::ALT | modifiers::SUPER;
    if (effective_mod & !supported_mask) != 0 {
        return None;
    }
    let mut mods = Vec::new();
    if effective_mod & modifiers::SHIFT != 0 {
        mods.push("shift");
    }
    if effective_mod & modifiers::CTRL != 0 {
        mods.push("ctrl");
    }
    if effective_mod & modifiers::ALT != 0 {
        mods.push("alt");
    }
    if effective_mod & modifiers::SUPER != 0 {
        mods.push("super");
    }
    Some(if mods.is_empty() {
        key_name.to_string()
    } else {
        format!("{}+{key_name}", mods.join("+"))
    })
}

fn format_parsed_key(
    codepoint: i32,
    modifier: u32,
    base_layout_key: Option<i32>,
) -> Option<String> {
    let normalized_codepoint = normalize_kitty_functional_codepoint(codepoint);
    let identity_codepoint =
        normalize_shifted_letter_identity_codepoint(normalized_codepoint, modifier);

    let is_latin_letter = (97..=122).contains(&identity_codepoint);
    let is_digit = (48..=57).contains(&identity_codepoint);
    let is_known_symbol = char::from_u32(identity_codepoint as u32).is_some_and(is_symbol_key);
    let effective_codepoint = if is_latin_letter || is_digit || is_known_symbol {
        identity_codepoint
    } else {
        base_layout_key.unwrap_or(identity_codepoint)
    };

    let key_name = if effective_codepoint == codepoints::ESCAPE {
        "escape".to_string()
    } else if effective_codepoint == codepoints::TAB {
        "tab".to_string()
    } else if effective_codepoint == codepoints::ENTER
        || effective_codepoint == codepoints::KP_ENTER
    {
        "enter".to_string()
    } else if effective_codepoint == codepoints::SPACE {
        "space".to_string()
    } else if effective_codepoint == codepoints::BACKSPACE {
        "backspace".to_string()
    } else if effective_codepoint == functional_codepoints::DELETE {
        "delete".to_string()
    } else if effective_codepoint == functional_codepoints::INSERT {
        "insert".to_string()
    } else if effective_codepoint == functional_codepoints::HOME {
        "home".to_string()
    } else if effective_codepoint == functional_codepoints::END {
        "end".to_string()
    } else if effective_codepoint == functional_codepoints::PAGE_UP {
        "pageUp".to_string()
    } else if effective_codepoint == functional_codepoints::PAGE_DOWN {
        "pageDown".to_string()
    } else if effective_codepoint == arrow_codepoints::UP {
        "up".to_string()
    } else if effective_codepoint == arrow_codepoints::DOWN {
        "down".to_string()
    } else if effective_codepoint == arrow_codepoints::LEFT {
        "left".to_string()
    } else if effective_codepoint == arrow_codepoints::RIGHT {
        "right".to_string()
    } else if (48..=57).contains(&effective_codepoint)
        || (97..=122).contains(&effective_codepoint)
        || char::from_u32(effective_codepoint as u32).is_some_and(is_symbol_key)
    {
        char::from_u32(effective_codepoint as u32)?.to_string()
    } else {
        return None;
    };

    format_key_name_with_modifiers(&key_name, modifier)
}

/// Parse raw terminal input and return the key identifier it represents
/// (e.g. `"ctrl+c"`), or `None` if unrecognized.
pub fn parse_key(data: &str) -> Option<String> {
    if let Some(kitty) = parse_kitty_sequence(data) {
        return format_parsed_key(kitty.codepoint, kitty.modifier, kitty.base_layout_key);
    }

    if let Some(mok) = parse_modify_other_keys_sequence(data) {
        return format_parsed_key(mok.codepoint, mok.modifier, None);
    }

    let kitty_active = is_kitty_protocol_active();

    // Mode-aware legacy sequences: when Kitty protocol is active, these
    // otherwise-ambiguous sequences are custom terminal shift+enter mappings.
    if kitty_active && (data == "\x1b\r" || data == "\n") {
        return Some("shift+enter".to_string());
    }

    if let Some(key_id) = legacy_sequence_key_id(data) {
        return Some(key_id.to_string());
    }

    match data {
        "\x1b" => return Some("escape".to_string()),
        "\x1c" => return Some("ctrl+\\".to_string()),
        "\x1d" => return Some("ctrl+]".to_string()),
        "\x1f" => return Some("ctrl+-".to_string()),
        "\x1b\x1b" => return Some("ctrl+alt+[".to_string()),
        "\x1b\x1c" => return Some("ctrl+alt+\\".to_string()),
        "\x1b\x1d" => return Some("ctrl+alt+]".to_string()),
        "\x1b\x1f" => return Some("ctrl+alt+-".to_string()),
        "\t" => return Some("tab".to_string()),
        "\x00" => return Some("ctrl+space".to_string()),
        " " => return Some("space".to_string()),
        "\x7f" => return Some("backspace".to_string()),
        "\x1b[Z" => return Some("shift+tab".to_string()),
        "\x1b\x7f" | "\x1b\x08" => return Some("alt+backspace".to_string()),
        "\x1b[A" => return Some("up".to_string()),
        "\x1b[B" => return Some("down".to_string()),
        "\x1b[C" => return Some("right".to_string()),
        "\x1b[D" => return Some("left".to_string()),
        "\x1b[3~" => return Some("delete".to_string()),
        "\x1b[5~" => return Some("pageUp".to_string()),
        "\x1b[6~" => return Some("pageDown".to_string()),
        _ => {}
    }

    if data == "\r" || (!kitty_active && data == "\n") || data == "\x1bOM" {
        return Some("enter".to_string());
    }
    if data == "\x08" {
        return Some(if is_windows_terminal_session() {
            "ctrl+backspace".to_string()
        } else {
            "backspace".to_string()
        });
    }
    if !kitty_active && data == "\x1b\r" {
        return Some("alt+enter".to_string());
    }
    if !kitty_active && data == "\x1b " {
        return Some("alt+space".to_string());
    }
    if !kitty_active && data == "\x1bB" {
        return Some("alt+left".to_string());
    }
    if !kitty_active && data == "\x1bF" {
        return Some("alt+right".to_string());
    }
    if data == "\x1b[H" || data == "\x1bOH" {
        return Some("home".to_string());
    }
    if data == "\x1b[F" || data == "\x1bOF" {
        return Some("end".to_string());
    }

    if !kitty_active && data.chars().count() == 2 && data.starts_with('\x1b') {
        let second = data.chars().nth(1).unwrap();
        let code = second as u32;
        if (1..=26).contains(&code) {
            return char::from_u32(code + 96).map(|c| format!("ctrl+alt+{c}"));
        }
        if (97..=122).contains(&code) || (48..=57).contains(&code) {
            return char::from_u32(code).map(|c| format!("alt+{c}"));
        }
    }

    // Raw Ctrl+letter, or a single printable character.
    if data.chars().count() == 1 {
        let code = data.chars().next().unwrap() as u32;
        if (1..=26).contains(&code) {
            return char::from_u32(code + 96).map(|c| format!("ctrl+{c}"));
        }
        if (32..=126).contains(&code) {
            return Some(data.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::set_kitty_protocol_active;

    #[test]
    fn test_parse_key_escape() {
        assert_eq!(parse_key("\x1b").as_deref(), Some("escape"));
    }

    #[test]
    fn test_parse_key_ctrl_letter() {
        assert_eq!(parse_key("\x03").as_deref(), Some("ctrl+c"));
    }

    #[test]
    fn test_parse_key_plain_letter() {
        assert_eq!(parse_key("a").as_deref(), Some("a"));
    }

    #[test]
    fn test_parse_key_arrow() {
        assert_eq!(parse_key("\x1b[A").as_deref(), Some("up"));
    }

    #[test]
    fn test_parse_key_tab() {
        assert_eq!(parse_key("\t").as_deref(), Some("tab"));
    }

    #[test]
    fn test_parse_key_enter() {
        assert_eq!(parse_key("\r").as_deref(), Some("enter"));
    }

    #[test]
    fn test_parse_key_alt_letter_legacy() {
        assert_eq!(parse_key("\x1bx").as_deref(), Some("alt+x"));
    }

    #[test]
    fn test_parse_key_ctrl_alt_letter_legacy() {
        assert_eq!(parse_key("\x1b\x03").as_deref(), Some("ctrl+alt+c"));
    }

    #[test]
    fn test_parse_key_kitty_ctrl_c() {
        set_kitty_protocol_active(true);
        assert_eq!(parse_key("\x1b[99;5u").as_deref(), Some("ctrl+c"));
        set_kitty_protocol_active(false);
    }

    #[test]
    fn test_parse_key_unrecognized() {
        assert_eq!(parse_key("\x1b[999zzz"), None);
    }

    #[test]
    fn test_parse_key_function_key() {
        assert_eq!(parse_key("\x1bOP").as_deref(), Some("f1"));
    }
}
