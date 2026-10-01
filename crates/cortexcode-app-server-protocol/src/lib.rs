//! Wire types for `hoocode app-server`.
//!
//! A subset of the Codex app-server protocol, written for this project. Field
//! names and shapes match Codex's wire format so its clients can connect; see
//! `docs/design/app-server.md` for what is covered.

pub mod jsonrpc;
pub mod messages;
pub mod types;

pub use jsonrpc::{
    error_response, notification, request, response, ErrorObject, Message, RequestId,
};
pub use messages::*;
pub use types::*;
