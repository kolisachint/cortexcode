//! OAuth flows for cortex AI providers.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `utils/oauth/*`.
//!
//! Each provider module exposes the pure request/response logic needed to
//! drive an OAuth login and to refresh tokens. Interactive concerns —
//! opening a browser, running a local HTTP callback server, prompting the
//! user — are left to the caller (an interactive CLI/TUI layer), since they
//! don't belong in provider logic and can't be meaningfully unit-tested here.

pub mod anthropic;
pub mod github_copilot;
pub mod pkce;
pub mod types;

pub use types::OAuthCredentials;
