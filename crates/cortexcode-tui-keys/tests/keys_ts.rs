//! Case-for-case port of the pin's `packages/tui/test/keys.test.ts`.
//!
//! The Kitty protocol flag and the Windows Terminal env vars are process
//! globals, so every test holds `GLOBALS` (the TS tests run serially).

use cortexcode_tui_keys::{
    decode_kitty_printable, decode_printable_key, matches_key, parse_key, set_kitty_protocol_active,
};
use std::sync::{Mutex, MutexGuard};

static GLOBALS: Mutex<()> = Mutex::new(());

fn kitty(active: bool) -> MutexGuard<'static, ()> {
    let guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
    set_kitty_protocol_active(active);
    guard
}

#[track_caller]
fn yes(data: &str, key: &str) {
    assert!(matches_key(data, key), "{data:?} should match {key}");
}

#[track_caller]
fn no(data: &str, key: &str) {
    assert!(!matches_key(data, key), "{data:?} should not match {key}");
}

#[track_caller]
fn parses(data: &str, key: &str) {
    assert_eq!(parse_key(data).as_deref(), Some(key), "parseKey({data:?})");
}

#[track_caller]
fn unparsed(data: &str) {
    assert_eq!(parse_key(data), None, "parseKey({data:?})");
}

/// `withEnvVars`: set (Some) or remove (None) env vars for the closure.
fn with_env(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
    let previous: Vec<_> = vars
        .iter()
        .map(|(k, _)| (*k, std::env::var(k).ok()))
        .collect();
    let apply = |k: &str, v: Option<&str>| match v {
        Some(v) => std::env::set_var(k, v),
        None => std::env::remove_var(k),
    };
    for (k, v) in vars {
        apply(k, *v);
    }
    f();
    for (k, v) in previous {
        apply(k, v.as_deref());
    }
}

// --- matchesKey: Kitty protocol with alternate keys (non-Latin layouts)

#[test]
fn cyrillic_ctrl_c_matches_via_base_layout_key() {
    let _g = kitty(true);
    yes("\x1b[1089::99;5u", "ctrl+c");
}

#[test]
fn cyrillic_ctrl_d_matches_via_base_layout_key() {
    let _g = kitty(true);
    yes("\x1b[1074::100;5u", "ctrl+d");
}

#[test]
fn cyrillic_ctrl_z_matches_via_base_layout_key() {
    let _g = kitty(true);
    yes("\x1b[1103::122;5u", "ctrl+z");
}

#[test]
fn ctrl_shift_p_matches_via_base_layout_key() {
    let _g = kitty(true);
    yes("\x1b[1079::112;6u", "ctrl+shift+p");
}

#[test]
fn direct_codepoint_matches_without_base_layout_key() {
    let _g = kitty(true);
    yes("\x1b[99;5u", "ctrl+c");
}

#[test]
fn super_modified_kitty_bindings() {
    let _g = kitty(true);
    yes("\x1b[107;9u", "super+k");
    yes("\x1b[13;9u", "super+enter");
    // `Key.ctrlSuper("k")` builds the id "ctrl+super+k".
    yes("\x1b[107;13u", "ctrl+super+k");
    yes("\x1b[107;14u", "ctrl+shift+super+k");
    no("\x1b[107;13u", "super+k");
    parses("\x1b[107;9u", "super+k");
    parses("\x1b[13;9u", "super+enter");
    parses("\x1b[107;13u", "ctrl+super+k");
    parses("\x1b[107;14u", "shift+ctrl+super+k");
}

#[test]
fn digit_bindings_via_kitty_csi_u() {
    let _g = kitty(true);
    yes("\x1b[49u", "1");
    yes("\x1b[49;5u", "ctrl+1");
    no("\x1b[49;5u", "ctrl+2");
    parses("\x1b[49u", "1");
    parses("\x1b[49;5u", "ctrl+1");
}

#[test]
fn kitty_keypad_keys_normalize_to_logical_keys() {
    let _g = kitty(true);
    yes("\x1b[57400u", "1");
    yes("\x1b[57410u", "/");
    yes("\x1b[57417u", "left");
    yes("\x1b[57426u", "delete");
    for (seq, key) in [
        ("\x1b[57399u", "0"),
        ("\x1b[57409u", "."),
        ("\x1b[57413u", "+"),
        ("\x1b[57416u", ","),
        ("\x1b[57417u", "left"),
        ("\x1b[57418u", "right"),
        ("\x1b[57419u", "up"),
        ("\x1b[57420u", "down"),
        ("\x1b[57421u", "pageUp"),
        ("\x1b[57422u", "pageDown"),
        ("\x1b[57423u", "home"),
        ("\x1b[57424u", "end"),
        ("\x1b[57425u", "insert"),
        ("\x1b[57426u", "delete"),
    ] {
        parses(seq, key);
    }
}

#[test]
fn shifted_key_in_format() {
    let _g = kitty(true);
    yes("\x1b[99:67:99;2u", "shift+c");
}

#[test]
fn event_type_in_format() {
    let _g = kitty(true);
    yes("\x1b[1089::99;5:3u", "ctrl+c");
}

#[test]
fn full_format_with_shifted_base_and_event_type() {
    let _g = kitty(true);
    yes("\x1b[1089:1057:99;6:2u", "ctrl+shift+c");
}

#[test]
fn prefers_codepoint_for_latin_letters_over_base_layout() {
    let _g = kitty(true);
    yes("\x1b[107::118;5u", "ctrl+k");
    no("\x1b[107::118;5u", "ctrl+v");
}

#[test]
fn prefers_codepoint_for_symbol_keys_over_base_layout() {
    let _g = kitty(true);
    yes("\x1b[47::91;5u", "ctrl+/");
    no("\x1b[47::91;5u", "ctrl+[");
}

#[test]
fn wrong_key_does_not_match_even_with_base_layout() {
    let _g = kitty(true);
    no("\x1b[1089::99;5u", "ctrl+d");
}

#[test]
fn wrong_modifiers_do_not_match_even_with_base_layout() {
    let _g = kitty(true);
    no("\x1b[1089::99;5u", "ctrl+shift+c");
}

// --- matchesKey: modifyOtherKeys

#[test]
fn modify_other_keys_ctrl_letters() {
    let _g = kitty(false);
    for (seq, key) in [
        ("\x1b[27;5;99~", "ctrl+c"),
        ("\x1b[27;5;100~", "ctrl+d"),
        ("\x1b[27;5;122~", "ctrl+z"),
    ] {
        yes(seq, key);
        parses(seq, key);
    }
}

#[test]
fn modify_other_keys_enter_tab_backspace_escape_space_variants() {
    let _g = kitty(false);
    for (seq, key) in [
        ("\x1b[27;5;13~", "ctrl+enter"),
        ("\x1b[27;2;13~", "shift+enter"),
        ("\x1b[27;3;13~", "alt+enter"),
        ("\x1b[27;2;9~", "shift+tab"),
        ("\x1b[27;5;9~", "ctrl+tab"),
        ("\x1b[27;3;9~", "alt+tab"),
        ("\x1b[27;1;127~", "backspace"),
        ("\x1b[27;5;127~", "ctrl+backspace"),
        ("\x1b[27;3;127~", "alt+backspace"),
        ("\x1b[27;1;27~", "escape"),
        ("\x1b[27;1;32~", "space"),
        ("\x1b[27;5;32~", "ctrl+space"),
        ("\x1b[27;5;47~", "ctrl+/"),
        ("\x1b[27;5;49~", "ctrl+1"),
        ("\x1b[27;2;49~", "shift+1"),
    ] {
        yes(seq, key);
        parses(seq, key);
    }
}

#[test]
fn modify_other_keys_shifted_uppercase_letters() {
    let _g = kitty(false);
    yes("\x1b[27;2;69~", "shift+e");
    yes("\x1b[27;6;69~", "ctrl+shift+e");
    parses("\x1b[27;2;69~", "shift+e");
    parses("\x1b[27;6;69~", "shift+ctrl+e");
}

#[test]
fn ctrl_alt_letter_via_csi_u_when_kitty_inactive() {
    let _g = kitty(false);
    yes("\x1b[104;7u", "ctrl+alt+h");
    parses("\x1b[104;7u", "ctrl+alt+h");
}

#[test]
fn ctrl_alt_letter_via_modify_other_keys() {
    let _g = kitty(false);
    yes("\x1b[27;7;104~", "ctrl+alt+h");
    parses("\x1b[27;7;104~", "ctrl+alt+h");
}

// --- matchesKey: legacy

#[test]
fn legacy_ctrl_c_and_ctrl_d() {
    let _g = kitty(false);
    yes("\x03", "ctrl+c");
    yes("\x04", "ctrl+d");
}

#[test]
fn escape_key() {
    let _g = kitty(false);
    yes("\x1b", "escape");
}

#[test]
fn legacy_linefeed_is_enter() {
    let _g = kitty(false);
    yes("\n", "enter");
    parses("\n", "enter");
}

#[test]
fn linefeed_is_shift_enter_when_kitty_active() {
    let _g = kitty(true);
    yes("\n", "shift+enter");
    no("\n", "enter");
    parses("\n", "shift+enter");
}

#[test]
fn ctrl_space() {
    let _g = kitty(false);
    yes("\x00", "ctrl+space");
    parses("\x00", "ctrl+space");
}

#[test]
fn legacy_ctrl_symbol() {
    let _g = kitty(false);
    yes("\x1c", "ctrl+\\");
    parses("\x1c", "ctrl+\\");
    yes("\x1d", "ctrl+]");
    parses("\x1d", "ctrl+]");
    yes("\x1f", "ctrl+_");
    yes("\x1f", "ctrl+-");
    parses("\x1f", "ctrl+-");
}

#[test]
fn legacy_ctrl_alt_symbol() {
    let _g = kitty(false);
    yes("\x1b\x1b", "ctrl+alt+[");
    parses("\x1b\x1b", "ctrl+alt+[");
    yes("\x1b\x1c", "ctrl+alt+\\");
    parses("\x1b\x1c", "ctrl+alt+\\");
    yes("\x1b\x1d", "ctrl+alt+]");
    parses("\x1b\x1d", "ctrl+alt+]");
    yes("\x1b\x1f", "ctrl+alt+_");
    yes("\x1b\x1f", "ctrl+alt+-");
    parses("\x1b\x1f", "ctrl+alt+-");
}

#[test]
fn raw_0x08_is_plain_backspace_outside_windows_terminal() {
    let _g = kitty(false);
    with_env(&[("WT_SESSION", None)], || {
        yes("\x7f", "backspace");
        no("\x7f", "ctrl+backspace");
        parses("\x7f", "backspace");
        yes("\x08", "backspace");
        no("\x08", "ctrl+backspace");
        parses("\x08", "backspace");
        yes("\x08", "ctrl+h");
    });
}

#[test]
fn raw_0x08_is_ctrl_backspace_in_local_windows_terminal() {
    let _g = kitty(false);
    with_env(
        &[
            ("WT_SESSION", Some("test-session")),
            ("SSH_CONNECTION", None),
            ("SSH_CLIENT", None),
            ("SSH_TTY", None),
        ],
        || {
            yes("\x08", "ctrl+backspace");
            no("\x08", "backspace");
            parses("\x08", "ctrl+backspace");
            yes("\x08", "ctrl+h");
        },
    );
}

#[test]
fn raw_0x08_is_plain_backspace_in_windows_terminal_over_ssh() {
    let _g = kitty(false);
    with_env(
        &[
            ("WT_SESSION", Some("test-session")),
            ("SSH_CONNECTION", Some("1 2 3 4")),
            ("SSH_CLIENT", Some("1 2 3")),
            ("SSH_TTY", Some("/dev/pts/1")),
        ],
        || {
            no("\x08", "ctrl+backspace");
            yes("\x08", "backspace");
            parses("\x08", "backspace");
            yes("\x08", "ctrl+h");
        },
    );
}

#[test]
fn legacy_alt_prefixed_sequences_only_when_kitty_inactive() {
    let g = kitty(false);
    for (seq, key) in [
        ("\x1b ", "alt+space"),
        ("\x1b\x08", "alt+backspace"),
        ("\x1b\x03", "ctrl+alt+c"),
        ("\x1bB", "alt+left"),
        ("\x1bF", "alt+right"),
        ("\x1ba", "alt+a"),
        ("\x1b1", "alt+1"),
        ("\x1by", "alt+y"),
        ("\x1bz", "alt+z"),
    ] {
        yes(seq, key);
        parses(seq, key);
    }
    drop(g);

    let _g = kitty(true);
    yes("\x1b\x08", "alt+backspace");
    parses("\x1b\x08", "alt+backspace");
    for (seq, key) in [
        ("\x1b ", "alt+space"),
        ("\x1b\x03", "ctrl+alt+c"),
        ("\x1bB", "alt+left"),
        ("\x1bF", "alt+right"),
        ("\x1ba", "alt+a"),
        ("\x1b1", "alt+1"),
        ("\x1by", "alt+y"),
    ] {
        no(seq, key);
        unparsed(seq);
    }
}

#[test]
fn arrow_keys() {
    let _g = kitty(false);
    yes("\x1b[A", "up");
    yes("\x1b[B", "down");
    yes("\x1b[C", "right");
    yes("\x1b[D", "left");
}

#[test]
fn ss3_arrows_and_home_end() {
    let _g = kitty(false);
    yes("\x1bOA", "up");
    yes("\x1bOB", "down");
    yes("\x1bOC", "right");
    yes("\x1bOD", "left");
    yes("\x1bOH", "home");
    yes("\x1bOF", "end");
}

#[test]
fn legacy_function_keys_and_clear() {
    let _g = kitty(false);
    yes("\x1bOP", "f1");
    yes("\x1b[24~", "f12");
    yes("\x1b[E", "clear");
}

#[test]
fn alt_arrows() {
    let _g = kitty(false);
    yes("\x1bp", "alt+up");
    no("\x1bp", "up");
}

#[test]
fn rxvt_modifier_sequences() {
    let _g = kitty(false);
    yes("\x1b[a", "shift+up");
    yes("\x1bOa", "ctrl+up");
    yes("\x1b[2$", "shift+insert");
    yes("\x1b[2^", "ctrl+insert");
    yes("\x1b[7$", "shift+home");
}

// --- decodeKittyPrintable / decodePrintableKey

#[test]
fn decode_kitty_keypad_keys_to_printable_characters() {
    let _g = kitty(false);
    for (seq, c) in [
        ("\x1b[57399u", '0'),
        ("\x1b[57400u", '1'),
        ("\x1b[57409u", '.'),
        ("\x1b[57410u", '/'),
        ("\x1b[57411u", '*'),
        ("\x1b[57412u", '-'),
        ("\x1b[57413u", '+'),
        ("\x1b[57415u", '='),
        ("\x1b[57416u", ','),
    ] {
        assert_eq!(decode_kitty_printable(seq), Some(c), "{seq:?}");
    }
    assert_eq!(decode_kitty_printable("\x1b[57417u"), None);
}

#[test]
fn decode_printable_modify_other_keys_sequences() {
    let _g = kitty(false);
    assert_eq!(decode_printable_key("\x1b[27;2;69~"), Some('E'));
    assert_eq!(decode_printable_key("\x1b[27;2;196~"), Some('Ä'));
    assert_eq!(decode_printable_key("\x1b[27;2;32~"), Some(' '));
    assert_eq!(decode_printable_key("\x1b[27;2;13~"), None);
    assert_eq!(decode_printable_key("\x1b[27;6;69~"), None);
}

// --- parseKey

#[test]
fn parse_returns_latin_name_when_base_layout_key_present() {
    let _g = kitty(true);
    parses("\x1b[1089::99;5u", "ctrl+c");
}

#[test]
fn parse_prefers_codepoint_for_latin_letters() {
    let _g = kitty(true);
    parses("\x1b[107::118;5u", "ctrl+k");
}

#[test]
fn parse_prefers_codepoint_for_symbol_keys() {
    let _g = kitty(true);
    parses("\x1b[47::91;5u", "ctrl+/");
}

#[test]
fn parse_uses_codepoint_without_base_layout() {
    let _g = kitty(true);
    parses("\x1b[99;5u", "ctrl+c");
}

#[test]
fn parse_shifted_uppercase_csi_u_letters_as_shift_letter() {
    let _g = kitty(true);
    yes("\x1b[69;2u", "shift+e");
    parses("\x1b[69;2u", "shift+e");
}

#[test]
fn parse_ignores_kitty_csi_u_with_unsupported_modifiers() {
    let _g = kitty(true);
    unparsed("\x1b[99;17u");
}

#[test]
fn parse_legacy_ctrl_letter() {
    let _g = kitty(false);
    parses("\x03", "ctrl+c");
    parses("\x04", "ctrl+d");
}

#[test]
fn parse_special_keys() {
    let _g = kitty(false);
    parses("\x1b", "escape");
    parses("\t", "tab");
    parses("\r", "enter");
    parses("\n", "enter");
    parses("\x00", "ctrl+space");
    parses(" ", "space");
    parses("1", "1");
    yes("1", "1");
}

#[test]
fn parse_arrow_keys() {
    let _g = kitty(false);
    parses("\x1b[A", "up");
    parses("\x1b[B", "down");
    parses("\x1b[C", "right");
    parses("\x1b[D", "left");
}

#[test]
fn parse_ss3_arrows_and_home_end() {
    let _g = kitty(false);
    parses("\x1bOA", "up");
    parses("\x1bOB", "down");
    parses("\x1bOC", "right");
    parses("\x1bOD", "left");
    parses("\x1bOH", "home");
    parses("\x1bOF", "end");
}

#[test]
fn parse_legacy_function_and_modifier_sequences() {
    let _g = kitty(false);
    parses("\x1bOP", "f1");
    parses("\x1b[24~", "f12");
    parses("\x1b[E", "clear");
    parses("\x1b[2^", "ctrl+insert");
    parses("\x1bp", "alt+up");
}

#[test]
fn parse_double_bracket_page_up() {
    let _g = kitty(false);
    parses("\x1b[[5~", "pageUp");
}
