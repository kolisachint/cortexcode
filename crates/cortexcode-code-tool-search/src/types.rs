//! `core/search/types.ts`.

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

/// `RetrieverSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RetrieverSource {
    Grep,
    Embed,
    Bm25,
}

impl RetrieverSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grep => "grep",
            Self::Embed => "embed",
            Self::Bm25 => "bm25",
        }
    }
}

/// `SearchMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    #[default]
    Auto,
    Lexical,
    Semantic,
    Hybrid,
}

impl SearchMode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "auto" => Self::Auto,
            "lexical" => Self::Lexical,
            "semantic" => Self::Semantic,
            "hybrid" => Self::Hybrid,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }
}

/// `ResolvedSearchMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSearchMode {
    Lexical,
    Semantic,
    Hybrid,
}

impl ResolvedSearchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }
}

impl Serialize for ResolvedSearchMode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Per-retriever values in insertion order (a JS object keyed by source).
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMap<T>(pub Vec<(RetrieverSource, T)>);

impl<T> Default for SourceMap<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T: Copy> SourceMap<T> {
    pub fn get(&self, source: RetrieverSource) -> Option<T> {
        self.0.iter().find(|(s, _)| *s == source).map(|(_, v)| *v)
    }

    pub fn set(&mut self, source: RetrieverSource, value: T) {
        match self.0.iter_mut().find(|(s, _)| *s == source) {
            Some(slot) => slot.1 = value,
            None => self.0.push((source, value)),
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (RetrieverSource, T)> + '_ {
        self.0.iter().copied()
    }
}

impl<T: Serialize> Serialize for SourceMap<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (source, value) in &self.0 {
            map.serialize_entry(source.as_str(), value)?;
        }
        map.end()
    }
}

/// `RankedHit`.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedHit {
    pub id: String,
    /// 1-indexed, gap-free rank within its retriever's list.
    pub rank: usize,
    /// Retriever-local score; diagnostics only.
    pub score: Option<f64>,
    pub source: RetrieverSource,
}

/// `FusedHit`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FusedHit {
    pub id: String,
    pub rrf_score: f64,
    /// Best rank per contributing retriever.
    pub ranks: SourceMap<usize>,
    /// Raw score at the best rank per retriever; diagnostics only.
    pub raw_scores: SourceMap<f64>,
    /// Ids folded into this one by span merging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_from: Option<Vec<String>>,
}

/// `CandidateSpan`: 1-based inclusive lines (`end_line` may pass the end of
/// the file; readers clamp).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateSpan {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// `FusedCandidate`.
#[derive(Debug, Clone, PartialEq)]
pub struct FusedCandidate {
    pub hit: FusedHit,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
}

impl FusedCandidate {
    pub fn new(hit: FusedHit, span: CandidateSpan) -> Self {
        Self {
            hit,
            path: span.path,
            start_line: span.start_line,
            end_line: span.end_line,
        }
    }

    pub fn id(&self) -> &str {
        &self.hit.id
    }
}
