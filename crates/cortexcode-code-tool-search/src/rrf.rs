//! `core/search/rrf.ts`: reciprocal rank fusion.

use std::collections::HashMap;

use cortexcode_agent_harness::frontmatter::locale_compare;

use crate::types::{FusedHit, RankedHit, SourceMap};

/// `DEFAULT_RRF_K`.
pub const DEFAULT_RRF_K: f64 = 60.0;

/// `rrfFuse`: sum `1/(k + rank)` per id; ties break by the number of
/// agreeing retrievers, then id. A duplicated `source:id` within one list
/// votes once, at its best rank.
pub fn rrf_fuse(lists: &[Vec<RankedHit>], k: f64) -> Result<Vec<FusedHit>, String> {
    if !k.is_finite() || k < 0.0 {
        return Err(format!(
            "RRF k must be a finite non-negative number; got {}",
            cortexcode_code_tool_api::js_number(k)
        ));
    }
    let mut order: Vec<String> = Vec::new();
    let mut acc: HashMap<String, FusedHit> = HashMap::new();
    for list in lists {
        let mut collapsed: Vec<&RankedHit> = Vec::new();
        for hit in list {
            if hit.rank < 1 {
                return Err(format!(
                    "RRF rank must be a positive integer; got {}",
                    hit.rank
                ));
            }
            match collapsed
                .iter_mut()
                .find(|h| h.source == hit.source && h.id == hit.id)
            {
                Some(existing) => {
                    if hit.rank < existing.rank {
                        *existing = hit;
                    }
                }
                None => collapsed.push(hit),
            }
        }
        for hit in collapsed {
            let current = acc.entry(hit.id.clone()).or_insert_with(|| {
                order.push(hit.id.clone());
                FusedHit {
                    id: hit.id.clone(),
                    rrf_score: 0.0,
                    ranks: SourceMap::default(),
                    raw_scores: SourceMap::default(),
                    merged_from: None,
                }
            });
            current.rrf_score += 1.0 / (k + hit.rank as f64);
            let old = current.ranks.get(hit.source);
            if old.is_none_or(|r| hit.rank < r) {
                current.ranks.set(hit.source, hit.rank);
                if let Some(score) = hit.score {
                    current.raw_scores.set(hit.source, score);
                }
            }
        }
    }
    let mut fused: Vec<FusedHit> = order
        .into_iter()
        .map(|id| acc.remove(&id).expect("accumulated"))
        .collect();
    fused.sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.ranks.len().cmp(&a.ranks.len()))
            .then_with(|| locale_compare(&a.id, &b.id))
    });
    Ok(fused)
}
