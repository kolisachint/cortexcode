//! Session CRUD, directory layout, and lifecycle for the cortex coding agent.
//!
//! This crate ports the JSONL session tree from the TypeScript
//! `packages/coding-agent/src/core/session-manager.ts`. Sessions are stored as
//! append-only `.jsonl` files where each line is a typed entry. Entries form a
//! tree via `id`/`parent_id`, and the leaf pointer tracks the current branch.

pub mod context;
pub mod entry;
pub mod manager;

pub use context::{build_session_context, ModelRef, SessionContext};
pub use entry::{CustomMessageContent, FileEntry, Header, CURRENT_SESSION_VERSION};
pub use manager::{
    build_session_info, create_session_id, default_session_dir, default_sessions_root, encode_cwd,
    find_most_recent_session, generate_id, list_all_sessions, list_sessions,
    load_entries_from_file, NewSessionOptions, SessionError, SessionInfo, SessionListProgress,
    SessionManager, SessionTreeNode,
};
