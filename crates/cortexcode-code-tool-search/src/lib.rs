//! `SearchCodebase`: hoocode `core/tools/search.ts` and the runtime half of
//! `core/search/` (v0.5.89). The eval harness (`search/eval*.ts`) and the
//! embsearch daemon client belong to ledger 12.4.

pub mod adapter;
pub mod assembler;
pub mod hybrid;
pub mod lexical;
pub mod mode;
pub mod rerank;
pub mod rrf;
pub mod service;
pub mod tool;
pub mod trace;
pub mod types;

pub use adapter::{adapt_grep_hits, AdaptedGrepHits, ChunkLookup, ChunkRef, GrepLineHit};
pub use assembler::{assemble_context, AssembledContext};
pub use hybrid::{
    hoist_stale_candidates, merge_overlapping_spans, retrieve_candidates, run_search,
    RetrieveOptions, RetrieveResult, RunSearchResult,
};
pub use lexical::{
    build_lexical_pattern, build_lexical_query_plan, run_lexical_retriever, LexicalQueryPlan,
    RunLexicalOptions,
};
pub use mode::{has_strong_lexical_signals, resolve_search_mode, ModeResolution};
pub use rerank::{query_is_prose, read_candidate_windows, rerank_candidates};
pub use rrf::{rrf_fuse, DEFAULT_RRF_K};
pub use service::{
    ChunkHit, ChunkRetriever, EmbsearchService, EmbsearchState, RerankPassage, RerankScore,
};
pub use tool::{
    create_search_tool, create_search_tool_definition, search_parameters_schema, SearchToolOptions,
    ServiceProvider,
};
pub use trace::{embsearch_store_dir, search_trace_path, SearchTrace};
pub use types::*;
