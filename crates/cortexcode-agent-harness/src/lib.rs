//! Agent harness: message conversion, system prompts, and prompt templates.
//!
//! Mirrors the `harness/` directory from the TypeScript
//! `@kolisachint/hoocode-agent-core` package.

pub mod messages;
pub mod prompt_templates;
pub mod system_prompt;

pub use messages::*;
pub use prompt_templates::*;
pub use system_prompt::*;
