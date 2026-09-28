//! Shared utilities for the cortex TUI: ANSI-aware string width, wrapping,
//! truncation, and column slicing.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `utils.ts`.

mod ansi;
pub mod js_regex;
mod text;
mod width;

pub use ansi::{bare_url_at, extract_ansi_code, hyperlink_at, AnsiCodeTracker};
pub use text::{
    apply_background_to_line, extract_segments, is_punctuation_char, is_whitespace_char,
    normalize_terminal_output, slice_by_column, slice_with_width, strip_vt_control_characters,
    truncate_to_width, wrap_text_with_ansi,
};
pub use width::visible_width;
