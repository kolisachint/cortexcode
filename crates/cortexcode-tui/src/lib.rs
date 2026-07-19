//! Umbrella crate for the cortex TUI namespace.
//!
//! Re-exports every `cortexcode-tui-*` leaf crate so callers can depend on
//! a single `cortexcode-tui` crate instead of naming each leaf individually.

pub use cortexcode_tui_components as components;
pub use cortexcode_tui_editing as editing;
pub use cortexcode_tui_fuzzy as fuzzy;
pub use cortexcode_tui_images as images;
pub use cortexcode_tui_keys as keys;
pub use cortexcode_tui_render as render;
pub use cortexcode_tui_terminal as terminal;
pub use cortexcode_tui_util as util;
