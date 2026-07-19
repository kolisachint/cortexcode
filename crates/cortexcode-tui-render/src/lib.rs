//! Differential rendering for the cortex TUI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` -> `tui.ts`: the
//! component tree (`Component`/`Container`), overlay positioning, and the
//! `TUI` differential terminal writer. See [`tui`] module docs for the
//! (documented, deliberate) simplifications made relative to the original.

mod component;
mod overlay;
mod tui;

pub use component::{Component, ComponentHandle, Container};
pub use overlay::{
    parse_size_value, resolve_overlay_layout, OverlayAnchor, OverlayLayout, OverlayMargin,
    OverlayOptions, SizeValue,
};
pub use tui::{InputListener, InputListenerResult, OverlayHandle, Tui, TuiEvent, CURSOR_MARKER};
