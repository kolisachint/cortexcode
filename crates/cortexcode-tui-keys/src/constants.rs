//! Shared constants for terminal key parsing/matching.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `keys.ts`.

pub const SYMBOL_KEYS: &[char] = &[
    '`', '-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/', '!', '@', '#', '$', '%', '^', '&',
    '*', '(', ')', '_', '+', '|', '~', '{', '}', ':', '<', '>', '?',
];

pub fn is_symbol_key(c: char) -> bool {
    SYMBOL_KEYS.contains(&c)
}

pub mod modifiers {
    pub const SHIFT: u32 = 1;
    pub const ALT: u32 = 2;
    pub const CTRL: u32 = 4;
    pub const SUPER: u32 = 8;
}

pub const LOCK_MASK: u32 = 64 + 128;

pub mod codepoints {
    pub const ESCAPE: i32 = 27;
    pub const TAB: i32 = 9;
    pub const ENTER: i32 = 13;
    pub const SPACE: i32 = 32;
    pub const BACKSPACE: i32 = 127;
    pub const KP_ENTER: i32 = 57414;
}

pub mod arrow_codepoints {
    pub const UP: i32 = -1;
    pub const DOWN: i32 = -2;
    pub const RIGHT: i32 = -3;
    pub const LEFT: i32 = -4;
}

pub mod functional_codepoints {
    pub const DELETE: i32 = -10;
    pub const INSERT: i32 = -11;
    pub const PAGE_UP: i32 = -12;
    pub const PAGE_DOWN: i32 = -13;
    pub const HOME: i32 = -14;
    pub const END: i32 = -15;
}

/// Kitty keypad/functional codepoints that alias a "normal" key.
pub fn normalize_kitty_functional_codepoint(codepoint: i32) -> i32 {
    use arrow_codepoints::*;
    use functional_codepoints::*;
    match codepoint {
        57399 => 48, // KP_0 -> '0'
        57400 => 49,
        57401 => 50,
        57402 => 51,
        57403 => 52,
        57404 => 53,
        57405 => 54,
        57406 => 55,
        57407 => 56,
        57408 => 57,
        57409 => 46, // KP_DECIMAL -> '.'
        57410 => 47, // KP_DIVIDE -> '/'
        57411 => 42, // KP_MULTIPLY -> '*'
        57412 => 45, // KP_SUBTRACT -> '-'
        57413 => 43, // KP_ADD -> '+'
        57415 => 61, // KP_EQUAL -> '='
        57416 => 44, // KP_SEPARATOR -> ','
        57417 => LEFT,
        57418 => RIGHT,
        57419 => UP,
        57420 => DOWN,
        57421 => PAGE_UP,
        57422 => PAGE_DOWN,
        57423 => HOME,
        57424 => END,
        57425 => INSERT,
        57426 => DELETE,
        other => other,
    }
}

/// Shift on a Latin uppercase codepoint identifies as the lowercase letter
/// (legacy terminals report shift+letter as the uppercase ASCII codepoint).
pub fn normalize_shifted_letter_identity_codepoint(codepoint: i32, modifier: u32) -> i32 {
    let effective_modifier = modifier & !LOCK_MASK;
    if (effective_modifier & modifiers::SHIFT) != 0 && (65..=90).contains(&codepoint) {
        codepoint + 32
    } else {
        codepoint
    }
}

pub struct LegacyKey {
    pub plain: &'static [&'static str],
    pub shift: &'static [&'static str],
    pub ctrl: &'static [&'static str],
}

macro_rules! legacy_key {
    ($plain:expr) => {
        LegacyKey {
            plain: $plain,
            shift: &[],
            ctrl: &[],
        }
    };
    ($plain:expr, shift: $shift:expr) => {
        LegacyKey {
            plain: $plain,
            shift: $shift,
            ctrl: &[],
        }
    };
    ($plain:expr, shift: $shift:expr, ctrl: $ctrl:expr) => {
        LegacyKey {
            plain: $plain,
            shift: $shift,
            ctrl: $ctrl,
        }
    };
}

pub fn legacy_key(name: &str) -> Option<LegacyKey> {
    Some(match name {
        "up" => legacy_key!(&["\x1b[A", "\x1bOA"], shift: &["\x1b[a"], ctrl: &["\x1bOa"]),
        "down" => legacy_key!(&["\x1b[B", "\x1bOB"], shift: &["\x1b[b"], ctrl: &["\x1bOb"]),
        "right" => legacy_key!(&["\x1b[C", "\x1bOC"], shift: &["\x1b[c"], ctrl: &["\x1bOc"]),
        "left" => legacy_key!(&["\x1b[D", "\x1bOD"], shift: &["\x1b[d"], ctrl: &["\x1bOd"]),
        "home" => {
            legacy_key!(&["\x1b[H", "\x1bOH", "\x1b[1~", "\x1b[7~"], shift: &["\x1b[7$"], ctrl: &["\x1b[7^"])
        }
        "end" => {
            legacy_key!(&["\x1b[F", "\x1bOF", "\x1b[4~", "\x1b[8~"], shift: &["\x1b[8$"], ctrl: &["\x1b[8^"])
        }
        "insert" => legacy_key!(&["\x1b[2~"], shift: &["\x1b[2$"], ctrl: &["\x1b[2^"]),
        "delete" => legacy_key!(&["\x1b[3~"], shift: &["\x1b[3$"], ctrl: &["\x1b[3^"]),
        "pageUp" => legacy_key!(&["\x1b[5~", "\x1b[[5~"], shift: &["\x1b[5$"], ctrl: &["\x1b[5^"]),
        "pageDown" => {
            legacy_key!(&["\x1b[6~", "\x1b[[6~"], shift: &["\x1b[6$"], ctrl: &["\x1b[6^"])
        }
        "clear" => legacy_key!(&["\x1b[E", "\x1bOE"], shift: &["\x1b[e"], ctrl: &["\x1bOe"]),
        "f1" => legacy_key!(&["\x1bOP", "\x1b[11~", "\x1b[[A"]),
        "f2" => legacy_key!(&["\x1bOQ", "\x1b[12~", "\x1b[[B"]),
        "f3" => legacy_key!(&["\x1bOR", "\x1b[13~", "\x1b[[C"]),
        "f4" => legacy_key!(&["\x1bOS", "\x1b[14~", "\x1b[[D"]),
        "f5" => legacy_key!(&["\x1b[15~", "\x1b[[E"]),
        "f6" => legacy_key!(&["\x1b[17~"]),
        "f7" => legacy_key!(&["\x1b[18~"]),
        "f8" => legacy_key!(&["\x1b[19~"]),
        "f9" => legacy_key!(&["\x1b[20~"]),
        "f10" => legacy_key!(&["\x1b[21~"]),
        "f11" => legacy_key!(&["\x1b[23~"]),
        "f12" => legacy_key!(&["\x1b[24~"]),
        _ => return None,
    })
}

/// Maps an unambiguous legacy escape sequence directly to its key identifier.
pub fn legacy_sequence_key_id(data: &str) -> Option<&'static str> {
    Some(match data {
        "\x1bOA" => "up",
        "\x1bOB" => "down",
        "\x1bOC" => "right",
        "\x1bOD" => "left",
        "\x1bOH" => "home",
        "\x1bOF" => "end",
        "\x1b[E" => "clear",
        "\x1bOE" => "clear",
        "\x1bOe" => "ctrl+clear",
        "\x1b[e" => "shift+clear",
        "\x1b[2~" => "insert",
        "\x1b[2$" => "shift+insert",
        "\x1b[2^" => "ctrl+insert",
        "\x1b[3$" => "shift+delete",
        "\x1b[3^" => "ctrl+delete",
        "\x1b[[5~" => "pageUp",
        "\x1b[[6~" => "pageDown",
        "\x1b[a" => "shift+up",
        "\x1b[b" => "shift+down",
        "\x1b[c" => "shift+right",
        "\x1b[d" => "shift+left",
        "\x1bOa" => "ctrl+up",
        "\x1bOb" => "ctrl+down",
        "\x1bOc" => "ctrl+right",
        "\x1bOd" => "ctrl+left",
        "\x1b[5$" => "shift+pageUp",
        "\x1b[6$" => "shift+pageDown",
        "\x1b[7$" => "shift+home",
        "\x1b[8$" => "shift+end",
        "\x1b[5^" => "ctrl+pageUp",
        "\x1b[6^" => "ctrl+pageDown",
        "\x1b[7^" => "ctrl+home",
        "\x1b[8^" => "ctrl+end",
        "\x1bOP" => "f1",
        "\x1bOQ" => "f2",
        "\x1bOR" => "f3",
        "\x1bOS" => "f4",
        "\x1b[11~" => "f1",
        "\x1b[12~" => "f2",
        "\x1b[13~" => "f3",
        "\x1b[14~" => "f4",
        "\x1b[[A" => "f1",
        "\x1b[[B" => "f2",
        "\x1b[[C" => "f3",
        "\x1b[[D" => "f4",
        "\x1b[[E" => "f5",
        "\x1b[15~" => "f5",
        "\x1b[17~" => "f6",
        "\x1b[18~" => "f7",
        "\x1b[19~" => "f8",
        "\x1b[20~" => "f9",
        "\x1b[21~" => "f10",
        "\x1b[23~" => "f11",
        "\x1b[24~" => "f12",
        "\x1bb" => "alt+left",
        "\x1bf" => "alt+right",
        "\x1bp" => "alt+up",
        "\x1bn" => "alt+down",
        _ => return None,
    })
}

pub fn matches_legacy_sequence(data: &str, sequences: &[&str]) -> bool {
    sequences.contains(&data)
}

pub fn matches_legacy_modifier_sequence(data: &str, key: &str, modifier: u32) -> bool {
    let Some(legacy) = legacy_key(key) else {
        return false;
    };
    if modifier == modifiers::SHIFT {
        matches_legacy_sequence(data, legacy.shift)
    } else if modifier == modifiers::CTRL {
        matches_legacy_sequence(data, legacy.ctrl)
    } else {
        false
    }
}

/// Whether this process is running in Windows Terminal, outside an SSH session.
pub fn is_windows_terminal_session() -> bool {
    std::env::var("WT_SESSION").is_ok_and(|v| !v.is_empty())
        && std::env::var("SSH_CONNECTION").is_err()
        && std::env::var("SSH_CLIENT").is_err()
        && std::env::var("SSH_TTY").is_err()
}
