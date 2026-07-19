//! Matching raw terminal input against a key identifier string (`"ctrl+c"`,
//! `"escape"`, `"shift+ctrl+p"`, ...).
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `keys.ts` (`matchesKey`).

use crate::constants::{
    arrow_codepoints, codepoints, functional_codepoints, is_symbol_key, legacy_key,
    matches_legacy_modifier_sequence, matches_legacy_sequence, modifiers,
};
use crate::kitty::{
    matches_kitty_sequence, matches_modify_other_keys, matches_printable_modify_other_keys,
};
use crate::state::is_kitty_protocol_active;

struct ParsedKeyId {
    key: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
    super_mod: bool,
}

fn parse_key_id(key_id: &str) -> Option<ParsedKeyId> {
    let lower = key_id.to_lowercase();
    let parts: Vec<&str> = lower.split('+').collect();
    let key = parts.last()?.to_string();
    if key.is_empty() {
        return None;
    }
    Some(ParsedKeyId {
        key,
        ctrl: parts.contains(&"ctrl"),
        shift: parts.contains(&"shift"),
        alt: parts.contains(&"alt"),
        super_mod: parts.contains(&"super"),
    })
}

/// Raw 0x08 (BS) is ambiguous: Windows Terminal uses it for Ctrl+Backspace,
/// other terminals/tmux setups use it for plain Backspace.
fn matches_raw_backspace(data: &str, expected_modifier: u32) -> bool {
    if data == "\x7f" {
        return expected_modifier == 0;
    }
    if data != "\x08" {
        return false;
    }
    if crate::constants::is_windows_terminal_session() {
        expected_modifier == modifiers::CTRL
    } else {
        expected_modifier == 0
    }
}

/// The control character produced by Ctrl+`key` (`code & 0x1f`).
fn raw_ctrl_char(key: &str) -> Option<char> {
    let c = key.chars().next()?.to_ascii_lowercase();
    let code = c as u32;
    if (97..=122).contains(&code) || matches!(c, '[' | '\\' | ']' | '_') {
        return char::from_u32(code & 0x1f);
    }
    if c == '-' {
        return char::from_u32(31); // same physical key as '_' on US keyboards
    }
    None
}

fn is_digit_key(key: &str) -> bool {
    key.len() == 1 && key.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Match raw terminal input `data` against a key identifier like `"ctrl+c"`,
/// `"escape"`, or `"shift+ctrl+p"`.
pub fn matches_key(data: &str, key_id: &str) -> bool {
    let Some(parsed) = parse_key_id(key_id) else {
        return false;
    };
    let ParsedKeyId {
        key,
        ctrl,
        shift,
        alt,
        super_mod,
    } = parsed;

    let mut modifier = 0u32;
    if shift {
        modifier |= modifiers::SHIFT;
    }
    if alt {
        modifier |= modifiers::ALT;
    }
    if ctrl {
        modifier |= modifiers::CTRL;
    }
    if super_mod {
        modifier |= modifiers::SUPER;
    }

    let kitty_active = is_kitty_protocol_active();

    match key.as_str() {
        "escape" | "esc" => {
            if modifier != 0 {
                return false;
            }
            data == "\x1b"
                || matches_kitty_sequence(data, codepoints::ESCAPE, 0)
                || matches_modify_other_keys(data, codepoints::ESCAPE, 0)
        }
        "space" => {
            if !kitty_active {
                if modifier == modifiers::CTRL && data == "\x00" {
                    return true;
                }
                if modifier == modifiers::ALT && data == "\x1b " {
                    return true;
                }
            }
            if modifier == 0 {
                return data == " "
                    || matches_kitty_sequence(data, codepoints::SPACE, 0)
                    || matches_modify_other_keys(data, codepoints::SPACE, 0);
            }
            matches_kitty_sequence(data, codepoints::SPACE, modifier)
                || matches_modify_other_keys(data, codepoints::SPACE, modifier)
        }
        "tab" => {
            if modifier == modifiers::SHIFT {
                return data == "\x1b[Z"
                    || matches_kitty_sequence(data, codepoints::TAB, modifiers::SHIFT)
                    || matches_modify_other_keys(data, codepoints::TAB, modifiers::SHIFT);
            }
            if modifier == 0 {
                return data == "\t" || matches_kitty_sequence(data, codepoints::TAB, 0);
            }
            matches_kitty_sequence(data, codepoints::TAB, modifier)
                || matches_modify_other_keys(data, codepoints::TAB, modifier)
        }
        "enter" | "return" => matches_enter(data, modifier, kitty_active),
        "backspace" => matches_backspace(data, modifier),
        "insert" => matches_legacy_or_kitty_functional(
            data,
            modifier,
            "insert",
            functional_codepoints::INSERT,
        ),
        "delete" => matches_legacy_or_kitty_functional(
            data,
            modifier,
            "delete",
            functional_codepoints::DELETE,
        ),
        "clear" => {
            if modifier == 0 {
                matches_legacy_sequence(data, legacy_key("clear").unwrap().plain)
            } else {
                matches_legacy_modifier_sequence(data, "clear", modifier)
            }
        }
        "home" => {
            matches_legacy_or_kitty_functional(data, modifier, "home", functional_codepoints::HOME)
        }
        "end" => {
            matches_legacy_or_kitty_functional(data, modifier, "end", functional_codepoints::END)
        }
        "pageup" => matches_legacy_or_kitty_functional(
            data,
            modifier,
            "pageUp",
            functional_codepoints::PAGE_UP,
        ),
        "pagedown" => matches_legacy_or_kitty_functional(
            data,
            modifier,
            "pageDown",
            functional_codepoints::PAGE_DOWN,
        ),
        "up" => matches_arrow(
            data,
            modifier,
            kitty_active,
            "up",
            arrow_codepoints::UP,
            "\x1bp",
        ),
        "down" => matches_arrow(
            data,
            modifier,
            kitty_active,
            "down",
            arrow_codepoints::DOWN,
            "\x1bn",
        ),
        "left" => matches_left_right(
            data,
            modifier,
            kitty_active,
            "left",
            arrow_codepoints::LEFT,
            "\x1b[1;3D",
            "\x1bB",
            "\x1bb",
            "\x1b[1;5D",
        ),
        "right" => matches_left_right(
            data,
            modifier,
            kitty_active,
            "right",
            arrow_codepoints::RIGHT,
            "\x1b[1;3C",
            "\x1bF",
            "\x1bf",
            "\x1b[1;5C",
        ),
        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            if modifier != 0 {
                return false;
            }
            legacy_key(&key).is_some_and(|k| matches_legacy_sequence(data, k.plain))
        }
        _ => matches_printable(data, &key, modifier, kitty_active),
    }
}

#[allow(clippy::too_many_arguments)]
fn matches_left_right(
    data: &str,
    modifier: u32,
    kitty_active: bool,
    name: &str,
    codepoint: i32,
    alt_csi: &str,
    alt_legacy_ss3: &str,
    alt_legacy_ansi: &str,
    ctrl_csi: &str,
) -> bool {
    if modifier == modifiers::ALT {
        return data == alt_csi
            || (!kitty_active && data == alt_legacy_ss3)
            || data == alt_legacy_ansi
            || matches_kitty_sequence(data, codepoint, modifiers::ALT);
    }
    if modifier == modifiers::CTRL {
        return data == ctrl_csi
            || matches_legacy_modifier_sequence(data, name, modifiers::CTRL)
            || matches_kitty_sequence(data, codepoint, modifiers::CTRL);
    }
    if modifier == 0 {
        return matches_legacy_sequence(data, legacy_key(name).unwrap().plain)
            || matches_kitty_sequence(data, codepoint, 0);
    }
    if matches_legacy_modifier_sequence(data, name, modifier) {
        return true;
    }
    matches_kitty_sequence(data, codepoint, modifier)
}

fn matches_arrow(
    data: &str,
    modifier: u32,
    kitty_active: bool,
    name: &str,
    codepoint: i32,
    alt_legacy: &str,
) -> bool {
    if modifier == modifiers::ALT {
        let _ = kitty_active;
        return data == alt_legacy || matches_kitty_sequence(data, codepoint, modifiers::ALT);
    }
    if modifier == 0 {
        return matches_legacy_sequence(data, legacy_key(name).unwrap().plain)
            || matches_kitty_sequence(data, codepoint, 0);
    }
    if matches_legacy_modifier_sequence(data, name, modifier) {
        return true;
    }
    matches_kitty_sequence(data, codepoint, modifier)
}

fn matches_legacy_or_kitty_functional(
    data: &str,
    modifier: u32,
    name: &str,
    codepoint: i32,
) -> bool {
    if modifier == 0 {
        return matches_legacy_sequence(data, legacy_key(name).unwrap().plain)
            || matches_kitty_sequence(data, codepoint, 0);
    }
    if matches_legacy_modifier_sequence(data, name, modifier) {
        return true;
    }
    matches_kitty_sequence(data, codepoint, modifier)
}

fn matches_enter(data: &str, modifier: u32, kitty_active: bool) -> bool {
    if modifier == modifiers::SHIFT {
        if matches_kitty_sequence(data, codepoints::ENTER, modifiers::SHIFT)
            || matches_kitty_sequence(data, codepoints::KP_ENTER, modifiers::SHIFT)
        {
            return true;
        }
        if matches_modify_other_keys(data, codepoints::ENTER, modifiers::SHIFT) {
            return true;
        }
        if kitty_active {
            return data == "\x1b\r" || data == "\n";
        }
        return false;
    }
    if modifier == modifiers::ALT {
        if matches_kitty_sequence(data, codepoints::ENTER, modifiers::ALT)
            || matches_kitty_sequence(data, codepoints::KP_ENTER, modifiers::ALT)
        {
            return true;
        }
        if matches_modify_other_keys(data, codepoints::ENTER, modifiers::ALT) {
            return true;
        }
        if !kitty_active {
            return data == "\x1b\r";
        }
        return false;
    }
    if modifier == 0 {
        return data == "\r"
            || (!kitty_active && data == "\n")
            || data == "\x1bOM"
            || matches_kitty_sequence(data, codepoints::ENTER, 0)
            || matches_kitty_sequence(data, codepoints::KP_ENTER, 0);
    }
    matches_kitty_sequence(data, codepoints::ENTER, modifier)
        || matches_kitty_sequence(data, codepoints::KP_ENTER, modifier)
        || matches_modify_other_keys(data, codepoints::ENTER, modifier)
}

fn matches_backspace(data: &str, modifier: u32) -> bool {
    if modifier == modifiers::ALT {
        if data == "\x1b\x7f" || data == "\x1b\x08" {
            return true;
        }
        return matches_kitty_sequence(data, codepoints::BACKSPACE, modifiers::ALT)
            || matches_modify_other_keys(data, codepoints::BACKSPACE, modifiers::ALT);
    }
    if modifier == modifiers::CTRL {
        if matches_raw_backspace(data, modifiers::CTRL) {
            return true;
        }
        return matches_kitty_sequence(data, codepoints::BACKSPACE, modifiers::CTRL)
            || matches_modify_other_keys(data, codepoints::BACKSPACE, modifiers::CTRL);
    }
    if modifier == 0 {
        return matches_raw_backspace(data, 0)
            || matches_kitty_sequence(data, codepoints::BACKSPACE, 0)
            || matches_modify_other_keys(data, codepoints::BACKSPACE, 0);
    }
    matches_kitty_sequence(data, codepoints::BACKSPACE, modifier)
        || matches_modify_other_keys(data, codepoints::BACKSPACE, modifier)
}

fn matches_printable(data: &str, key: &str, modifier: u32, kitty_active: bool) -> bool {
    let Some(k) = key.chars().next() else {
        return false;
    };
    if key.chars().count() != 1
        || !(k.is_ascii_lowercase() || is_digit_key(key) || is_symbol_key(k))
    {
        return false;
    }

    let codepoint = k as i32;
    let raw_ctrl = raw_ctrl_char(key);
    let is_letter = k.is_ascii_lowercase();
    let is_digit = is_digit_key(key);

    if modifier == modifiers::CTRL + modifiers::ALT && !kitty_active {
        if let Some(rc) = raw_ctrl {
            if data == format!("\x1b{rc}") {
                return true;
            }
        }
    }

    if modifier == modifiers::ALT
        && !kitty_active
        && (is_letter || is_digit)
        && data == format!("\x1b{key}")
    {
        return true;
    }

    if modifier == modifiers::CTRL {
        if let Some(rc) = raw_ctrl {
            if data.chars().eq([rc]) {
                return true;
            }
        }
        return matches_kitty_sequence(data, codepoint, modifiers::CTRL)
            || matches_printable_modify_other_keys(data, codepoint, modifiers::CTRL);
    }

    if modifier == modifiers::SHIFT + modifiers::CTRL {
        return matches_kitty_sequence(data, codepoint, modifiers::SHIFT + modifiers::CTRL)
            || matches_printable_modify_other_keys(
                data,
                codepoint,
                modifiers::SHIFT + modifiers::CTRL,
            );
    }

    if modifier == modifiers::SHIFT {
        if is_letter && data == key.to_uppercase() {
            return true;
        }
        return matches_kitty_sequence(data, codepoint, modifiers::SHIFT)
            || matches_printable_modify_other_keys(data, codepoint, modifiers::SHIFT);
    }

    if modifier != 0 {
        return matches_kitty_sequence(data, codepoint, modifier)
            || matches_printable_modify_other_keys(data, codepoint, modifier);
    }

    data == key || matches_kitty_sequence(data, codepoint, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::set_kitty_protocol_active;

    #[test]
    fn test_matches_key_escape() {
        assert!(matches_key("\x1b", "escape"));
        assert!(!matches_key("a", "escape"));
    }

    #[test]
    fn test_matches_key_plain_letter() {
        assert!(matches_key("a", "a"));
        assert!(!matches_key("b", "a"));
    }

    #[test]
    fn test_matches_key_ctrl_c() {
        assert!(matches_key("\x03", "ctrl+c"));
    }

    #[test]
    fn test_matches_key_ctrl_minus_maps_to_unit_separator() {
        assert!(matches_key("\x1f", "ctrl+-"));
    }

    #[test]
    fn test_matches_key_shift_letter_is_uppercase() {
        assert!(matches_key("A", "shift+a"));
    }

    #[test]
    fn test_matches_key_tab() {
        assert!(matches_key("\t", "tab"));
        assert!(matches_key("\x1b[Z", "shift+tab"));
    }

    #[test]
    fn test_matches_key_enter() {
        assert!(matches_key("\r", "enter"));
    }

    #[test]
    fn test_matches_key_backspace() {
        assert!(matches_key("\x7f", "backspace"));
    }

    #[test]
    fn test_matches_key_arrow_up() {
        assert!(matches_key("\x1b[A", "up"));
        assert!(matches_key("\x1bOA", "up"));
    }

    #[test]
    fn test_matches_key_ctrl_arrow_via_kitty() {
        assert!(matches_key("\x1b[1;5A", "ctrl+up"));
    }

    #[test]
    fn test_matches_key_alt_left_legacy() {
        assert!(matches_key("\x1bb", "alt+left"));
    }

    #[test]
    fn test_matches_key_function_key() {
        assert!(matches_key("\x1bOP", "f1"));
    }

    #[test]
    fn test_matches_key_kitty_ctrl_c_when_protocol_active() {
        set_kitty_protocol_active(true);
        assert!(matches_key("\x1b[99;5u", "ctrl+c"));
        set_kitty_protocol_active(false);
    }

    #[test]
    fn test_matches_key_invalid_key_id() {
        assert!(!matches_key("a", ""));
    }

    #[test]
    fn test_matches_key_combined_modifiers() {
        assert!(matches_key("\x1b[99;7u", "ctrl+alt+c"));
    }
}
