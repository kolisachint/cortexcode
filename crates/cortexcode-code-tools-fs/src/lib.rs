//! File tools for the cortex coding agent.
//!
//! Ports hoocode's `core/tools/read.ts`, `read-dedup.ts`, `edit.ts`,
//! `edit-diff.ts`, `write.ts`, `file-mutation-queue.ts` and `core/context-gc.ts`.

mod access;
pub mod context_gc;
pub mod edit;
pub mod edit_diff;
pub mod mutation_queue;
pub mod read;
pub mod read_dedup;
pub mod write;

pub use context_gc::{evict_superseded_reads, BudgetPressureLatch, ContextGcOptions};
pub use edit::{
    create_edit_tool, create_edit_tool_definition, edit_parameters_schema, prepare_edit_arguments,
    EditOperations, EditToolOptions, LocalEditOperations,
};
pub use edit_diff::{
    apply_edits_to_normalized_content, compute_edits_diff, generate_diff_string, AppliedEdits,
    Edit, EditDiff,
};
pub use mutation_queue::with_file_mutation_queue;
pub use write::{
    create_write_tool, create_write_tool_definition, write_parameters_schema, LocalWriteOperations,
    WriteOperations, WriteToolOptions,
};

pub use read::{
    create_read_tool, create_read_tool_definition, read_parameters_schema, LocalReadOperations,
    ReadOperations, ReadToolOptions,
};
