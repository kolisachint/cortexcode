//! Shared utilities for the cortex TUI: ANSI-aware string width, wrapping,
//! truncation, and column slicing.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `utils.ts`.

mod ansi;
mod text;
mod width;

pub use ansi::{extract_ansi_code, AnsiCodeTracker};
pub use text::{
    apply_background_to_line, extract_segments, is_punctuation_char, is_whitespace_char,
    normalize_terminal_output, slice_by_column, slice_with_width, truncate_to_width,
    wrap_text_with_ansi,
};
pub use width::visible_width;
