//! `core/search/adapter.ts`: grep line hits become a ranked candidate list
//! (indexed chunks, or per-file clusters with `rel#L<line>` ids).

use std::collections::{HashMap, HashSet};

use cortexcode_agent_harness::frontmatter::locale_compare;

use crate::types::{CandidateSpan, RankedHit, RetrieverSource};

/// `GrepLineHit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepLineHit {
    /// Repo-relative POSIX path.
    pub rel: String,
    /// 1-based line number.
    pub line: usize,
    /// Lowercased query terms on this line.
    pub terms: Option<Vec<String>>,
}

impl GrepLineHit {
    pub fn new(rel: impl Into<String>, line: usize) -> Self {
        Self {
            rel: rel.into(),
            line,
            terms: None,
        }
    }
}

/// An indexed chunk's identity and span (`ChunkLookup`'s result).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRef {
    pub id: String,
    pub span: CandidateSpan,
}

/// `ChunkLookup`: the indexed chunk enclosing a line, if any.
pub type ChunkLookup<'a> = &'a dyn Fn(&str, usize) -> Option<ChunkRef>;

const FALLBACK_PAD_LINES: usize = 5;
const FALLBACK_MERGE_GAP: usize = 10;
const FALLBACK_MAX_CLUSTER_LINES: usize = 40;
const PER_FILE_CANDIDATE_CAP: usize = 8;

/// `AdaptedGrepHits`.
#[derive(Debug, Clone, Default)]
pub struct AdaptedGrepHits {
    pub hits: Vec<RankedHit>,
    /// The span of every emitted id.
    pub spans: HashMap<String, CandidateSpan>,
}

/// An unindexed hit: line, terms, index in the hit list.
type UnmappedHit<'a> = (usize, &'a [String], usize);

struct Candidate {
    id: String,
    span: CandidateSpan,
    terms: HashSet<String>,
    hit_count: usize,
    first_seen: usize,
}

/// `adaptGrepHits`.
pub fn adapt_grep_hits(
    line_hits: &[GrepLineHit],
    lookup: Option<ChunkLookup<'_>>,
) -> AdaptedGrepHits {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut unmapped: Vec<(String, Vec<UnmappedHit<'_>>)> = Vec::new();

    for (index, hit) in line_hits.iter().enumerate() {
        let terms: &[String] = hit.terms.as_deref().unwrap_or(&[]);
        let Some(chunk) = lookup.and_then(|f| f(&hit.rel, hit.line)) else {
            match unmapped.iter_mut().find(|(rel, _)| *rel == hit.rel) {
                Some((_, list)) => list.push((hit.line, terms, index)),
                None => unmapped.push((hit.rel.clone(), vec![(hit.line, terms, index)])),
            }
            continue;
        };
        match by_id.get(&chunk.id) {
            Some(&i) => {
                candidates[i].hit_count += 1;
                candidates[i].terms.extend(terms.iter().cloned());
            }
            None => {
                by_id.insert(chunk.id.clone(), candidates.len());
                candidates.push(Candidate {
                    id: chunk.id,
                    span: chunk.span,
                    terms: terms.iter().cloned().collect(),
                    hit_count: 1,
                    first_seen: index,
                });
            }
        }
    }

    for (rel, mut hits) in unmapped {
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
        let mut cluster: Vec<UnmappedHit<'_>> = Vec::new();
        let mut flush = |cluster: &mut Vec<UnmappedHit<'_>>| {
            if cluster.is_empty() {
                return;
            }
            let first = cluster[0].0;
            let last = cluster[cluster.len() - 1].0;
            let id = format!("{rel}#L{first}");
            let candidate = Candidate {
                id: id.clone(),
                span: CandidateSpan {
                    path: rel.clone(),
                    start_line: first.saturating_sub(FALLBACK_PAD_LINES).max(1),
                    end_line: last + FALLBACK_PAD_LINES,
                },
                terms: cluster.iter().flat_map(|h| h.1.iter().cloned()).collect(),
                hit_count: cluster.len(),
                first_seen: cluster.iter().map(|h| h.2).min().unwrap_or(0),
            };
            match by_id.get(&id) {
                Some(&i) => candidates[i] = candidate,
                None => {
                    by_id.insert(id, candidates.len());
                    candidates.push(candidate);
                }
            }
            cluster.clear();
        };
        for hit in hits {
            if let (Some(start), Some(prev)) = (cluster.first(), cluster.last()) {
                if hit.0 - prev.0 > FALLBACK_MERGE_GAP
                    || hit.0 - start.0 > FALLBACK_MAX_CLUSTER_LINES
                {
                    flush(&mut cluster);
                }
            }
            cluster.push(hit);
        }
        flush(&mut cluster);
    }

    candidates.sort_by(|a, b| {
        b.terms
            .len()
            .cmp(&a.terms.len())
            .then(b.hit_count.cmp(&a.hit_count))
            .then(a.first_seen.cmp(&b.first_seen))
            .then_with(|| locale_compare(&a.id, &b.id))
    });

    let mut out = AdaptedGrepHits::default();
    let mut per_file: HashMap<String, usize> = HashMap::new();
    for candidate in candidates {
        let count = per_file.entry(candidate.span.path.clone()).or_insert(0);
        if *count >= PER_FILE_CANDIDATE_CAP {
            continue;
        }
        *count += 1;
        out.hits.push(RankedHit {
            id: candidate.id.clone(),
            rank: out.hits.len() + 1,
            score: None,
            source: RetrieverSource::Grep,
        });
        out.spans.insert(candidate.id, candidate.span);
    }
    out
}
