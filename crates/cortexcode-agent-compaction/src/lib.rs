//! Context compaction and branch summarization for long sessions: port of
//! hoocode `packages/agent/src/harness/compaction/` (v0.5.89).
//!
//! [`prepare_compaction`] picks what to summarize from a session path,
//! [`compact`] asks the model for the summary, and the session layer writes
//! the compaction entry. [`generate_branch_summary`] does the same for a
//! branch being left during tree navigation.

pub mod branch_summarization;
pub mod compaction;
pub mod utils;

pub use branch_summarization::{
    collect_entries_for_branch_summary, generate_branch_summary, prepare_branch_entries,
    BranchEntrySource, BranchPreparation, BranchSummaryDetails, BranchSummaryResult,
    CollectEntriesResult, GenerateBranchSummaryOptions,
};
pub use compaction::{
    calculate_context_tokens, compact, estimate_context_tokens, estimate_tokens, find_cut_point,
    find_turn_start_index, generate_summary, get_last_assistant_usage, prepare_compaction,
    should_compact, CompactionDetails, CompactionPreparation, CompactionResult, CompactionSettings,
    ContextUsageEstimate, CutPointResult, SummarizeOptions, DEFAULT_COMPACTION_SETTINGS,
};
pub use utils::{
    compute_file_lists, create_file_ops, extract_file_ops_from_message, format_file_operations,
    serialize_conversation, FileOperations, SUMMARIZATION_SYSTEM_PROMPT,
};
