//! File tools for the cortex coding agent.
//!
//! Ports hoocode's `core/tools/read.ts` and `core/tools/read-dedup.ts`. The
//! `edit`/`write` tools and the file mutation queue join this crate in 10.2c.

pub mod read;
pub mod read_dedup;

pub use read::{
    create_read_tool, create_read_tool_definition, read_parameters_schema, LocalReadOperations,
    ReadOperations, ReadToolOptions,
};
