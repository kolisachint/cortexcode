//! The coding agent's keyboard map: port of hoocode `core/keybindings.ts`
//! and `modes/interactive/components/keybinding-hints.ts`.

pub mod hints;
pub mod keybindings;

pub use hints::{
    app_key_label, format_key_text, key_display_label, key_display_text, key_hint, key_text,
    matches_app_key, raw_key_hint,
};
pub use keybindings::{
    app_keybindings, keybinding, keybinding_definitions, keybindings, migrate_keybindings_config,
    migrate_keybindings_config_file, order_keybindings_config, to_keybindings_config,
    AppKeybindingsManager, KeybindingEntry, KEYBINDING_NAME_MIGRATIONS,
};
