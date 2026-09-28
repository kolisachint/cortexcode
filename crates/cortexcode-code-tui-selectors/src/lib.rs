//! The coding agent's pickers and dialogs (`modes/interactive/components/`).
//!
//! - [`session_selector`]: the session picker (`/resume`, `alt+h`, `--resume`).
//! - [`session_selector_search`]: its search (fuzzy tokens, phrases, `re:`).
//! - [`small_selectors`]: thinking level, theme, show images, session colour.
//! - [`user_message_selector`]: the message to fork from.
//! - [`ask_options`]: the options pane (`ask_options`).
//! - [`settings_selector`]: the `/settings` pane.

pub mod ask_options;
pub mod framed_list;
pub mod session_selector;
pub mod session_selector_search;
pub mod settings_selector;
pub mod small_selectors;
pub mod user_message_selector;
