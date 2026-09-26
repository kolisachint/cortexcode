//! The slice of hoocode's `EmbsearchService` that search uses. The daemon
//! client that implements it is ledger 12.4.

use crate::adapter::ChunkRef;

/// `EmbsearchState`.
#[derive(Debug, Clone, PartialEq)]
pub enum EmbsearchState {
    Idle,
    Skipped {
        reason: String,
    },
    Downloading {
        received_bytes: u64,
        total_bytes: Option<u64>,
    },
    Indexing {
        done: u64,
        total: u64,
    },
    Ready {
        chunk_count: u64,
    },
    Unavailable {
        reason: String,
    },
}

/// Which index a chunk query uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkRetriever {
    Dense,
    /// The daemon fuses BM25 and vectors itself.
    Hybrid,
    /// BM25 only.
    Lexical,
}

/// A chunk returned by the index.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkHit {
    pub id: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
}

/// A passage for the cross-encoder, and its score.
#[derive(Debug, Clone, PartialEq)]
pub struct RerankPassage {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RerankScore {
    pub id: String,
    pub score: f64,
}

/// `EmbsearchService` (the calls `hybrid-search.ts` makes).
pub trait EmbsearchService: Send + Sync {
    fn state(&self) -> EmbsearchState;
    fn is_available(&self) -> bool;
    fn supports_lexical_retriever(&self) -> bool {
        false
    }
    fn supports_cross_encoder(&self) -> bool {
        false
    }
    /// Repo-relative files the index has no current content for.
    fn stale_files(&self) -> Vec<String> {
        Vec::new()
    }
    fn search_chunks(
        &self,
        query: &str,
        top_k: usize,
        glob: Option<&str>,
        retriever: ChunkRetriever,
    ) -> Result<Vec<ChunkHit>, String>;
    fn find_enclosing_chunk(&self, rel: &str, line: usize) -> Option<ChunkRef>;
    fn rerank(
        &self,
        _query: &str,
        _passages: &[RerankPassage],
        _k: usize,
    ) -> Result<Vec<RerankScore>, String> {
        Ok(Vec::new())
    }
}
