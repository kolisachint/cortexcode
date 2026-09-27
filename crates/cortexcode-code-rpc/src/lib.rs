//! RPC mode for the cortex coding agent (`modes/rpc/` in hoocode's
//! `packages/coding-agent`): strict JSONL framing and the command loop that
//! drives an [`AgentSession`](cortexcode_code_agent_session::AgentSession)
//! from stdin and streams responses and session events to stdout.
//!
//! The wire format is hoocode's (`docs/rpc.md`); it is also what subagents
//! speak (`--mode rpc`).

pub mod client;
pub mod jsonl;
pub mod mode;

pub use jsonl::{serialize_json_line, JsonlLineReader};
pub use mode::{
    error, run_rpc_mode, success, ForkChange, HostFuture, RpcHost, RpcMode, RpcOutput, RuntimeHost,
    SessionChange, SingleSessionHost,
};
