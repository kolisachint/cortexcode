//! Keyboard handling for the cortex TUI: terminal key-sequence parsing
//! (legacy escape codes, Kitty keyboard protocol, xterm `modifyOtherKeys`)
//! and a keybinding registry.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `keys.ts`,
//! `keybindings.ts`.

mod constants;
mod keybindings;
mod kitty;
mod matching;
mod parse;
mod state;

pub use keybindings::{
    default_tui_keybindings, KeybindingConflict, KeybindingDefinition, KeybindingsManager,
};
pub use kitty::{
    decode_kitty_printable, decode_modify_other_keys_printable, decode_printable_key,
    is_key_release, is_key_repeat, parse_kitty_sequence, KeyEventType, ParsedKittySequence,
};
pub use matching::matches_key;
pub use parse::parse_key;
pub use state::{is_kitty_protocol_active, set_kitty_protocol_active};
