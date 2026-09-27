//! `core/search/hybrid-search.ts`: resolve the mode, run the retrievers,
//! fuse by rank, rerank, hoist stale files, merge spans, then assemble the
//! text and write a trace.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use cortexcode_ai_types::AbortSignal;

use crate::adapter::{adapt_grep_hits, ChunkRef};
use crate::assembler::assemble_context;
use crate::lexical::{normalize_search_glob, run_lexical_retriever, RunLexicalOptions};
use crate::mode::resolve_search_mode;
use crate::rerank::{read_candidate_windows, rerank_candidates};
use crate::rrf::{rrf_fuse, DEFAULT_RRF_K};
use crate::service::{ChunkRetriever, EmbsearchService, EmbsearchState, RerankPassage};
use crate::trace::{write_search_trace, RerankStats, RetrieverStats, SearchTrace};
use crate::types::*;

/// Raw grep line hits fetched per query.
const LEXICAL_MATCH_LIMIT: usize = 200;
/// Adapted lexical candidates entering fusion in hybrid mode.
const LEXICAL_FUSION_CAP: usize = 20;
const EMBED_TOP_K: usize = 50;
const FUSION_POOL_TOP_K: usize = 200;
const BM25_TOP_K: usize = FUSION_POOL_TOP_K;
const STALE_HOIST_CAP: usize = 5;
const FUSED_WINDOW: usize = 50;
/// Candidates sent to the cross-encoder per query.
pub const CROSS_ENCODER_DEPTH: usize = 30;

/// `RetrieveOptions`.
#[derive(Clone)]
pub struct RetrieveOptions<'a> {
    pub cwd: &'a Path,
    pub query: &'a str,
    pub mode: SearchMode,
    pub glob: Option<&'a str>,
    /// Maximum candidates returned (default 10).
    pub limit: Option<usize>,
    pub rrf_k: Option<f64>,
    /// Rerank the fused window (default true).
    pub rerank: Option<bool>,
    pub daemon_hybrid: bool,
    pub bm25_leg: Option<bool>,
    pub cross_encoder: bool,
    pub service: Option<Arc<dyn EmbsearchService>>,
    pub signal: Option<AbortSignal>,
}

impl<'a> RetrieveOptions<'a> {
    /// Defaults: auto mode, no glob, limit 10, k = 60, rerank on, no service.
    pub fn new(cwd: &'a Path, query: &'a str) -> Self {
        Self {
            cwd,
            query,
            mode: SearchMode::Auto,
            glob: None,
            limit: None,
            rrf_k: None,
            rerank: None,
            daemon_hybrid: false,
            bm25_leg: None,
            cross_encoder: false,
            service: None,
            signal: None,
        }
    }
}

/// `RetrieveResult`.
#[derive(Debug, Clone)]
pub struct RetrieveResult {
    pub candidates: Vec<FusedCandidate>,
    pub resolved_mode: ResolvedSearchMode,
    pub degraded_reason: Option<String>,
    /// `ready`, `indexing` or `unavailable`.
    pub index_phase: &'static str,
    pub retrievers: SourceMap<RetrieverStats>,
    /// `(done, total)` while the index is still building.
    pub indexing: Option<(u64, u64)>,
    pub rrf_k: f64,
    pub rerank: Option<RerankStats>,
}

/// `RunSearchResult`.
#[derive(Debug, Clone)]
pub struct RunSearchResult {
    pub text: String,
    pub resolved_mode: ResolvedSearchMode,
    pub degraded_reason: Option<String>,
    pub result_count: usize,
    pub indexing: Option<(u64, u64)>,
}

/// `hoistStaleCandidates`: up to five candidates from files the index has
/// not read move to the front, in order.
pub fn hoist_stale_candidates(
    candidates: &[FusedCandidate],
    stale: &HashSet<String>,
) -> Vec<FusedCandidate> {
    if stale.is_empty() {
        return candidates.to_vec();
    }
    let (mut hoisted, mut rest) = (Vec::new(), Vec::new());
    for candidate in candidates {
        if stale.contains(&candidate.path) && hoisted.len() < STALE_HOIST_CAP {
            hoisted.push(candidate.clone());
        } else {
            rest.push(candidate.clone());
        }
    }
    hoisted.extend(rest);
    hoisted
}

/// `mergeOverlappingSpans`: overlapping or abutting spans of one file fold
/// into the better-ranked one (ranks: best per source; the id is kept in
/// `merged_from`).
pub fn merge_overlapping_spans(candidates: &[FusedCandidate]) -> Vec<FusedCandidate> {
    let mut merged: Vec<FusedCandidate> = Vec::new();
    for candidate in candidates {
        let into = merged.iter_mut().find(|m| {
            m.path == candidate.path
                && candidate.start_line <= m.end_line + 1
                && m.start_line <= candidate.end_line + 1
        });
        let Some(into) = into else {
            merged.push(candidate.clone());
            continue;
        };
        into.start_line = into.start_line.min(candidate.start_line);
        into.end_line = into.end_line.max(candidate.end_line);
        into.hit
            .merged_from
            .get_or_insert_with(Vec::new)
            .push(candidate.hit.id.clone());
        for (source, rank) in candidate.hit.ranks.iter() {
            if into.hit.ranks.get(source).is_none_or(|held| rank < held) {
                into.hit.ranks.set(source, rank);
            }
        }
        for (source, score) in candidate.hit.raw_scores.iter() {
            if into
                .hit
                .raw_scores
                .get(source)
                .is_none_or(|held| score > held)
            {
                into.hit.raw_scores.set(source, score);
            }
        }
    }
    merged
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

fn aborted(signal: &Option<AbortSignal>) -> bool {
    signal.as_ref().is_some_and(AbortSignal::aborted)
}

/// Cross-encoder rerank of the first [`CROSS_ENCODER_DEPTH`] candidates
/// (`crossEncoderRerank`); returns the order and how many were scored.
fn cross_encoder_rerank(
    query: &str,
    candidates: &[FusedCandidate],
    cwd: &Path,
    service: &dyn EmbsearchService,
) -> Result<(Vec<FusedCandidate>, usize), String> {
    if candidates.len() < 2 {
        return Ok((candidates.to_vec(), 0));
    }
    let split = candidates.len().min(CROSS_ENCODER_DEPTH);
    let (head, tail) = candidates.split_at(split);
    let windows = read_candidate_windows(head, cwd);
    let mut passages = Vec::new();
    let mut by_id: HashMap<&str, &FusedCandidate> = HashMap::new();
    for (candidate, window) in head.iter().zip(windows) {
        if let Some(text) = window {
            passages.push(RerankPassage {
                id: candidate.hit.id.clone(),
                text,
            });
            by_id.insert(&candidate.hit.id, candidate);
        }
    }
    if passages.is_empty() {
        return Ok((candidates.to_vec(), 0));
    }
    let scored = service.rerank(query, &passages, passages.len())?;
    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    for result in scored {
        if let Some(candidate) = by_id.get(result.id.as_str()) {
            if seen.insert(result.id.clone()) {
                ordered.push((*candidate).clone());
            }
        }
    }
    ordered.extend(head.iter().filter(|c| !seen.contains(&c.hit.id)).cloned());
    ordered.extend(tail.iter().cloned());
    Ok((ordered, passages.len()))
}

/// `retrieveCandidates`.
pub fn retrieve_candidates(options: &RetrieveOptions<'_>) -> Result<RetrieveResult, String> {
    let cwd = options.cwd;
    let query = options.query;
    let service = options.service.as_deref();
    let glob = normalize_search_glob(options.glob);
    let limit = options.limit.unwrap_or(10).max(1);
    let rrf_k = options.rrf_k.unwrap_or(DEFAULT_RRF_K);
    let state = service.map(EmbsearchService::state);
    let embed_available = service.is_some_and(EmbsearchService::is_available);
    let unavailable_reason: Option<String> = match &state {
        None => Some("semantic index is not enabled".into()),
        Some(EmbsearchState::Unavailable { reason } | EmbsearchState::Skipped { reason }) => {
            Some(reason.clone())
        }
        Some(EmbsearchState::Idle) => Some("semantic index has not started".into()),
        Some(_) => None,
    };
    let resolution = resolve_search_mode(
        query,
        options.mode,
        embed_available,
        unavailable_reason.as_deref(),
    );
    let mode = resolution.mode;

    let lookup = |rel: &str, line: usize| -> Option<ChunkRef> {
        service.and_then(|s| s.find_enclosing_chunk(rel, line))
    };

    let mut spans: HashMap<String, CandidateSpan> = HashMap::new();
    let mut stale: HashSet<String> = HashSet::new();
    let mut lists: Vec<Vec<RankedHit>> = Vec::new();
    let mut stats: SourceMap<RetrieverStats> = SourceMap::default();
    let mut errors: Vec<String> = Vec::new();

    let bm25_as_lexical_leg = options
        .bm25_leg
        .unwrap_or_else(|| service.is_some_and(EmbsearchService::supports_lexical_retriever))
        && embed_available
        && mode != ResolvedSearchMode::Lexical;

    if matches!(
        mode,
        ResolvedSearchMode::Lexical | ResolvedSearchMode::Hybrid
    ) {
        let started = Instant::now();
        let scoped: Option<Vec<String>> = if bm25_as_lexical_leg {
            service.map(EmbsearchService::stale_files)
        } else {
            None
        };
        if let Some(paths) = &scoped {
            stale.extend(paths.iter().cloned());
        }
        let run = run_lexical_retriever(RunLexicalOptions {
            cwd,
            query,
            limit: LEXICAL_MATCH_LIMIT,
            glob: glob.as_deref(),
            signal: options.signal.clone(),
            paths: scoped.as_deref(),
        });
        match run {
            Ok(line_hits) => {
                let adapted = adapt_grep_hits(
                    &line_hits,
                    embed_available.then_some(&lookup as &dyn Fn(&str, usize) -> Option<ChunkRef>),
                );
                let mut hits = adapted.hits;
                if mode == ResolvedSearchMode::Hybrid {
                    hits.truncate(LEXICAL_FUSION_CAP);
                }
                for (id, span) in adapted.spans {
                    spans.entry(id).or_insert(span);
                }
                stats.set(
                    RetrieverSource::Grep,
                    RetrieverStats {
                        latency_ms: elapsed_ms(started),
                        hit_count: hits.len(),
                    },
                );
                lists.push(hits);
            }
            Err(e) => {
                errors.push(e);
                stats.set(
                    RetrieverSource::Grep,
                    RetrieverStats {
                        latency_ms: elapsed_ms(started),
                        hit_count: 0,
                    },
                );
            }
        }
    }

    if let (true, Some(service)) = (
        matches!(
            mode,
            ResolvedSearchMode::Semantic | ResolvedSearchMode::Hybrid
        ),
        service,
    ) {
        let started = Instant::now();
        let top_k = if options.bm25_leg == Some(true) {
            FUSION_POOL_TOP_K
        } else {
            EMBED_TOP_K
        };
        let retriever = if options.daemon_hybrid {
            ChunkRetriever::Hybrid
        } else {
            ChunkRetriever::Dense
        };
        match service.search_chunks(query, top_k, glob.as_deref(), retriever) {
            Ok(chunks) => {
                let chunks: Vec<_> = chunks.into_iter().filter(|h| h.score > 0.0).collect();
                let hits: Vec<RankedHit> = chunks
                    .iter()
                    .enumerate()
                    .map(|(i, h)| RankedHit {
                        id: h.id.clone(),
                        rank: i + 1,
                        score: Some(h.score),
                        source: RetrieverSource::Embed,
                    })
                    .collect();
                for h in chunks {
                    spans.insert(
                        h.id,
                        CandidateSpan {
                            path: h.path,
                            start_line: h.start_line,
                            end_line: h.end_line,
                        },
                    );
                }
                stats.set(
                    RetrieverSource::Embed,
                    RetrieverStats {
                        latency_ms: elapsed_ms(started),
                        hit_count: hits.len(),
                    },
                );
                lists.push(hits);
            }
            Err(e) => {
                errors.push(e);
                stats.set(
                    RetrieverSource::Embed,
                    RetrieverStats {
                        latency_ms: elapsed_ms(started),
                        hit_count: 0,
                    },
                );
            }
        }
    }

    if let (true, Some(service)) = (bm25_as_lexical_leg, service) {
        let started = Instant::now();
        match service.search_chunks(query, BM25_TOP_K, glob.as_deref(), ChunkRetriever::Lexical) {
            Ok(chunks) => {
                let hits: Vec<RankedHit> = chunks
                    .iter()
                    .enumerate()
                    .map(|(i, h)| RankedHit {
                        id: h.id.clone(),
                        rank: i + 1,
                        score: Some(h.score),
                        source: RetrieverSource::Bm25,
                    })
                    .collect();
                for h in chunks {
                    spans.entry(h.id).or_insert(CandidateSpan {
                        path: h.path,
                        start_line: h.start_line,
                        end_line: h.end_line,
                    });
                }
                stats.set(
                    RetrieverSource::Bm25,
                    RetrieverStats {
                        latency_ms: elapsed_ms(started),
                        hit_count: hits.len(),
                    },
                );
                lists.push(hits);
            }
            Err(e) => {
                errors.push(e);
                stats.set(
                    RetrieverSource::Bm25,
                    RetrieverStats {
                        latency_ms: elapsed_ms(started),
                        hit_count: 0,
                    },
                );
            }
        }
    }

    if aborted(&options.signal) {
        return Err("Operation aborted".into());
    }
    if lists.is_empty() {
        return Err(errors
            .into_iter()
            .next()
            .unwrap_or_else(|| "search produced no retriever results".into()));
    }

    let fused = rrf_fuse(&lists, rrf_k)?;
    let mut candidates: Vec<FusedCandidate> = fused
        .into_iter()
        .take(FUSED_WINDOW)
        .filter_map(|hit| {
            let span = spans.get(&hit.id)?.clone();
            Some(FusedCandidate::new(hit, span))
        })
        .collect();

    let mut rerank = None;
    if let (true, Some(service)) = (
        options.cross_encoder && service.is_some_and(EmbsearchService::supports_cross_encoder),
        service,
    ) {
        let started = Instant::now();
        let (reranked, scored) = cross_encoder_rerank(query, &candidates, cwd, service)?;
        rerank = Some(RerankStats {
            applied: true,
            candidate_count: scored,
            latency_ms: elapsed_ms(started),
        });
        candidates = reranked;
    } else if options.rerank != Some(false) {
        let started = Instant::now();
        let count = candidates.len();
        candidates = rerank_candidates(query, &candidates, cwd);
        rerank = Some(RerankStats {
            applied: true,
            candidate_count: count,
            latency_ms: elapsed_ms(started),
        });
    }

    let mut candidates = merge_overlapping_spans(&hoist_stale_candidates(&candidates, &stale));
    candidates.truncate(limit);
    let (index_phase, indexing) = match state {
        Some(EmbsearchState::Ready { .. }) => ("ready", None),
        Some(EmbsearchState::Indexing { done, total }) => ("indexing", Some((done, total))),
        _ => ("unavailable", None),
    };
    Ok(RetrieveResult {
        candidates,
        resolved_mode: mode,
        degraded_reason: resolution.degraded_reason,
        index_phase,
        retrievers: stats,
        indexing,
        rrf_k,
        rerank,
    })
}

/// `runSearch`: retrieve, assemble within `token_budget`, trace.
pub fn run_search(
    options: &RetrieveOptions<'_>,
    token_budget: Option<usize>,
) -> Result<RunSearchResult, String> {
    let retrieved = retrieve_candidates(options)?;
    let assembled = assemble_context(&retrieved.candidates, options.cwd, token_budget);
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    write_search_trace(
        options.cwd,
        &SearchTrace {
            timestamp_ms,
            query: options.query.to_owned(),
            requested_mode: options.mode.as_str(),
            resolved_mode: retrieved.resolved_mode,
            degraded_reason: retrieved.degraded_reason.clone(),
            index_phase: retrieved.index_phase,
            rrf_k: (retrieved.resolved_mode == ResolvedSearchMode::Hybrid)
                .then_some(retrieved.rrf_k),
            retrievers: retrieved.retrievers.clone(),
            fused: retrieved.candidates.iter().map(|c| c.hit.clone()).collect(),
            rerank: retrieved.rerank,
        },
    );
    Ok(RunSearchResult {
        text: assembled.text,
        resolved_mode: retrieved.resolved_mode,
        degraded_reason: retrieved.degraded_reason,
        result_count: retrieved.candidates.len(),
        indexing: retrieved.indexing,
    })
}
