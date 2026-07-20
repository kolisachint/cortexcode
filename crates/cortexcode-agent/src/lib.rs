//! Umbrella crate for the cortex agent namespace.
//!
//! Re-exports every `cortexcode-agent-*` leaf crate so consumers can depend on
//! a single crate and qualify imports as `cortexcode::agent::*`.

pub use cortexcode_agent_compaction as compaction;
pub use cortexcode_agent_core as core;
pub use cortexcode_agent_harness as harness;
pub use cortexcode_agent_loop as agent_loop;
pub use cortexcode_agent_mcp as mcp;
pub use cortexcode_agent_session as session;
pub use cortexcode_agent_tools as tools;
pub use cortexcode_agent_types as types;
