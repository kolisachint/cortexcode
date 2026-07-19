//! Kitty keyboard protocol (CSI-u) and xterm `modifyOtherKeys` sequence parsing.
//!
//! See <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `keys.ts`.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::constants::{
    arrow_codepoints, functional_codepoints, is_symbol_key, modifiers,
    normalize_kitty_functional_codepoint, normalize_shifted_letter_identity_codepoint, LOCK_MASK,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    Press,
    Repeat,
    Release,
}

pub struct ParsedKittySequence {
    pub codepoint: i32,
    pub shifted_key: Option<i32>,
    pub base_layout_key: Option<i32>,
    pub modifier: u32,
    pub event_type: KeyEventType,
}

pub struct ParsedModifyOtherKeysSequence {
    pub codepoint: i32,
    pub modifier: u32,
}

fn parse_event_type(s: Option<&str>) -> KeyEventType {
    match s.and_then(|s| s.parse::<i32>().ok()) {
        Some(2) => KeyEventType::Repeat,
        Some(3) => KeyEventType::Release,
        _ => KeyEventType::Press,
    }
}

static CSI_U_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\x1b\[(\d+)(?::(\d*))?(?::(\d+))?(?:;(\d+))?(?::(\d+))?u$").unwrap()
});
static ARROW_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\x1b\[1;(\d+)(?::(\d+))?([ABCD])$").unwrap());
static FUNC_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\x1b\[(\d+)(?:;(\d+))?(?::(\d+))?~$").unwrap());
static HOME_END_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\x1b\[1;(\d+)(?::(\d+))?([HF])$").unwrap());
static MODIFY_OTHER_KEYS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\x1b\[27;(\d+);(\d+)~$").unwrap());

fn parse_i32(m: Option<regex::Match>) -> Option<i32> {
    m.and_then(|m| m.as_str().parse().ok())
}

/// Parse a Kitty keyboard-protocol escape sequence (CSI-u, modified arrow
/// keys, modified functional keys, or modified Home/End).
pub fn parse_kitty_sequence(data: &str) -> Option<ParsedKittySequence> {
    if let Some(caps) = CSI_U_RE.captures(data) {
        let codepoint = parse_i32(caps.get(1))?;
        let shifted_key = caps
            .get(2)
            .filter(|m| !m.as_str().is_empty())
            .and_then(|m| m.as_str().parse().ok());
        let base_layout_key = parse_i32(caps.get(3));
        let mod_value = parse_i32(caps.get(4)).unwrap_or(1);
        let event_type = parse_event_type(caps.get(5).map(|m| m.as_str()));
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key,
            base_layout_key,
            modifier: (mod_value - 1).max(0) as u32,
            event_type,
        });
    }

    if let Some(caps) = ARROW_RE.captures(data) {
        let mod_value = parse_i32(caps.get(1))?;
        let event_type = parse_event_type(caps.get(2).map(|m| m.as_str()));
        let letter = caps.get(3)?.as_str();
        let codepoint = match letter {
            "A" => arrow_codepoints::UP,
            "B" => arrow_codepoints::DOWN,
            "C" => arrow_codepoints::RIGHT,
            "D" => arrow_codepoints::LEFT,
            _ => return None,
        };
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: (mod_value - 1).max(0) as u32,
            event_type,
        });
    }

    if let Some(caps) = FUNC_RE.captures(data) {
        let key_num = parse_i32(caps.get(1))?;
        let mod_value = parse_i32(caps.get(2)).unwrap_or(1);
        let event_type = parse_event_type(caps.get(3).map(|m| m.as_str()));
        let codepoint = match key_num {
            2 => functional_codepoints::INSERT,
            3 => functional_codepoints::DELETE,
            5 => functional_codepoints::PAGE_UP,
            6 => functional_codepoints::PAGE_DOWN,
            7 => functional_codepoints::HOME,
            8 => functional_codepoints::END,
            _ => return None,
        };
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: (mod_value - 1).max(0) as u32,
            event_type,
        });
    }

    if let Some(caps) = HOME_END_RE.captures(data) {
        let mod_value = parse_i32(caps.get(1))?;
        let event_type = parse_event_type(caps.get(2).map(|m| m.as_str()));
        let letter = caps.get(3)?.as_str();
        let codepoint = if letter == "H" {
            functional_codepoints::HOME
        } else {
            functional_codepoints::END
        };
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: (mod_value - 1).max(0) as u32,
            event_type,
        });
    }

    None
}

/// Whether `data` matches a Kitty-protocol key press with the given
/// codepoint and modifier bitmask (also matches via a non-Latin base layout
/// key when the codepoint isn't a recognized Latin letter/symbol).
pub fn matches_kitty_sequence(data: &str, expected_codepoint: i32, expected_modifier: u32) -> bool {
    let Some(parsed) = parse_kitty_sequence(data) else {
        return false;
    };
    let actual_mod = parsed.modifier & !LOCK_MASK;
    let expected_mod = expected_modifier & !LOCK_MASK;
    if actual_mod != expected_mod {
        return false;
    }

    let normalized_codepoint = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(parsed.codepoint),
        parsed.modifier,
    );
    let normalized_expected_codepoint = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(expected_codepoint),
        expected_modifier,
    );

    if normalized_codepoint == normalized_expected_codepoint {
        return true;
    }

    if let Some(base) = parsed.base_layout_key {
        if base == expected_codepoint {
            let cp = normalized_codepoint;
            let is_latin_letter = (97..=122).contains(&cp);
            let is_known_symbol = char::from_u32(cp as u32).is_some_and(is_symbol_key);
            if !is_latin_letter && !is_known_symbol {
                return true;
            }
        }
    }

    false
}

/// Parse xterm's `modifyOtherKeys` sequence: `CSI 27 ; modifiers ; keycode ~`.
pub fn parse_modify_other_keys_sequence(data: &str) -> Option<ParsedModifyOtherKeysSequence> {
    let caps = MODIFY_OTHER_KEYS_RE.captures(data)?;
    let mod_value = parse_i32(caps.get(1))?;
    let codepoint = parse_i32(caps.get(2))?;
    Some(ParsedModifyOtherKeysSequence {
        codepoint,
        modifier: (mod_value - 1).max(0) as u32,
    })
}

pub fn matches_modify_other_keys(
    data: &str,
    expected_keycode: i32,
    expected_modifier: u32,
) -> bool {
    let Some(parsed) = parse_modify_other_keys_sequence(data) else {
        return false;
    };
    parsed.codepoint == expected_keycode && parsed.modifier == expected_modifier
}

pub fn matches_printable_modify_other_keys(
    data: &str,
    expected_keycode: i32,
    expected_modifier: u32,
) -> bool {
    if expected_modifier == 0 {
        return false;
    }
    let Some(parsed) = parse_modify_other_keys_sequence(data) else {
        return false;
    };
    if parsed.modifier != expected_modifier {
        return false;
    }
    normalize_shifted_letter_identity_codepoint(parsed.codepoint, parsed.modifier)
        == normalize_shifted_letter_identity_codepoint(expected_keycode, expected_modifier)
}

const KITTY_PRINTABLE_ALLOWED_MODIFIERS: u32 = modifiers::SHIFT | LOCK_MASK;

/// Decode a Kitty CSI-u sequence into a printable character, if it carries
/// only a plain-or-Shift printable key (Ctrl/Alt/unsupported modifiers are
/// rejected — those are handled by keybinding matching instead).
pub fn decode_kitty_printable(data: &str) -> Option<char> {
    let caps = CSI_U_RE.captures(data)?;
    let codepoint = parse_i32(caps.get(1))?;
    let shifted_key = caps
        .get(2)
        .filter(|m| !m.as_str().is_empty())
        .and_then(|m| m.as_str().parse::<i32>().ok());
    let mod_value = parse_i32(caps.get(4)).unwrap_or(1);
    let modifier = (mod_value - 1).max(0) as u32;

    if (modifier & !KITTY_PRINTABLE_ALLOWED_MODIFIERS) != 0 {
        return None;
    }
    if (modifier & (modifiers::ALT | modifiers::CTRL)) != 0 {
        return None;
    }

    let mut effective_codepoint = codepoint;
    if modifier & modifiers::SHIFT != 0 {
        if let Some(shifted) = shifted_key {
            effective_codepoint = shifted;
        }
    }
    effective_codepoint = normalize_kitty_functional_codepoint(effective_codepoint);
    if effective_codepoint < 32 {
        return None;
    }

    char::from_u32(effective_codepoint as u32)
}

pub fn decode_modify_other_keys_printable(data: &str) -> Option<char> {
    let parsed = parse_modify_other_keys_sequence(data)?;
    let modifier = parsed.modifier & !LOCK_MASK;
    if (modifier & !modifiers::SHIFT) != 0 {
        return None;
    }
    if parsed.codepoint < 32 {
        return None;
    }
    char::from_u32(parsed.codepoint as u32)
}

/// Decode a printable character from either a Kitty CSI-u or
/// `modifyOtherKeys` sequence.
pub fn decode_printable_key(data: &str) -> Option<char> {
    decode_kitty_printable(data).or_else(|| decode_modify_other_keys_printable(data))
}

/// Don't treat bracketed-paste content as a key release/repeat, even if it
/// contains a substring that would otherwise match (e.g. `:3F` inside a
/// pasted MAC address like `90:62:3F:A5`).
fn is_bracketed_paste(data: &str) -> bool {
    data.contains("\x1b[200~")
}

/// Whether `data` is a Kitty-protocol key-release event (flag 2).
pub fn is_key_release(data: &str) -> bool {
    if is_bracketed_paste(data) {
        return false;
    }
    [":3u", ":3~", ":3A", ":3B", ":3C", ":3D", ":3H", ":3F"]
        .iter()
        .any(|marker| data.contains(marker))
}

/// Whether `data` is a Kitty-protocol key-repeat event (flag 2).
pub fn is_key_repeat(data: &str) -> bool {
    if is_bracketed_paste(data) {
        return false;
    }
    [":2u", ":2~", ":2A", ":2B", ":2C", ":2D", ":2H", ":2F"]
        .iter()
        .any(|marker| data.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_csi_u_basic() {
        let parsed = parse_kitty_sequence("\x1b[99u").unwrap();
        assert_eq!(parsed.codepoint, 99); // 'c'
        assert_eq!(parsed.modifier, 0);
        assert_eq!(parsed.event_type, KeyEventType::Press);
    }

    #[test]
    fn test_parse_csi_u_with_modifier() {
        let parsed = parse_kitty_sequence("\x1b[99;5u").unwrap();
        assert_eq!(parsed.codepoint, 99);
        assert_eq!(parsed.modifier, 4); // ctrl = mod 5 - 1
    }

    #[test]
    fn test_parse_csi_u_with_event_type() {
        let parsed = parse_kitty_sequence("\x1b[99;1:3u").unwrap();
        assert_eq!(parsed.event_type, KeyEventType::Release);
    }

    #[test]
    fn test_parse_arrow_with_modifier() {
        let parsed = parse_kitty_sequence("\x1b[1;5A").unwrap();
        assert_eq!(parsed.codepoint, arrow_codepoints::UP);
        assert_eq!(parsed.modifier, 4);
    }

    #[test]
    fn test_parse_functional_key() {
        let parsed = parse_kitty_sequence("\x1b[3;5~").unwrap();
        assert_eq!(parsed.codepoint, functional_codepoints::DELETE);
        assert_eq!(parsed.modifier, 4);
    }

    #[test]
    fn test_parse_home_end() {
        let parsed = parse_kitty_sequence("\x1b[1;2H").unwrap();
        assert_eq!(parsed.codepoint, functional_codepoints::HOME);
        assert_eq!(parsed.modifier, 1); // shift
    }

    #[test]
    fn test_matches_kitty_sequence_ctrl_c() {
        assert!(matches_kitty_sequence("\x1b[99;5u", 99, modifiers::CTRL));
    }

    #[test]
    fn test_matches_kitty_sequence_wrong_modifier() {
        assert!(!matches_kitty_sequence("\x1b[99;5u", 99, modifiers::ALT));
    }

    #[test]
    fn test_parse_modify_other_keys() {
        let parsed = parse_modify_other_keys_sequence("\x1b[27;5;99~").unwrap();
        assert_eq!(parsed.codepoint, 99);
        assert_eq!(parsed.modifier, 4);
    }

    #[test]
    fn test_decode_kitty_printable_plain() {
        assert_eq!(decode_kitty_printable("\x1b[97u"), Some('a'));
    }

    #[test]
    fn test_decode_kitty_printable_rejects_ctrl() {
        assert_eq!(decode_kitty_printable("\x1b[97;5u"), None);
    }

    #[test]
    fn test_decode_kitty_printable_shifted() {
        // codepoint 97 ('a'), shifted key 65 ('A'), shift modifier (mod value 2).
        assert_eq!(decode_kitty_printable("\x1b[97:65;2u"), Some('A'));
    }

    #[test]
    fn test_is_key_release() {
        assert!(is_key_release("\x1b[99;1:3u"));
        assert!(!is_key_release("\x1b[99u"));
    }

    #[test]
    fn test_is_key_release_ignores_bracketed_paste() {
        assert!(!is_key_release("\x1b[200~90:62:3F:A5\x1b[201~"));
    }

    #[test]
    fn test_is_key_repeat() {
        assert!(is_key_repeat("\x1b[99;1:2u"));
        assert!(!is_key_repeat("\x1b[99u"));
    }
}
