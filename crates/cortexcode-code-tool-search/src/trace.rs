//! `core/search/trace.ts`: per-call diagnostics appended to a jsonl file in
//! the embsearch store dir (never into model context). Best effort.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::types::{FusedHit, ResolvedSearchMode};

const TRACE_FILE: &str = "search-trace.jsonl";
const TRACE_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// Per-retriever latency and hit count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrieverStats {
    pub latency_ms: u64,
    pub hit_count: usize,
}

/// Reranker diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RerankStats {
    pub applied: bool,
    pub candidate_count: usize,
    pub latency_ms: u64,
}

/// `SearchTrace`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchTrace {
    pub timestamp_ms: u64,
    pub query: String,
    pub requested_mode: &'static str,
    pub resolved_mode: ResolvedSearchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    pub index_phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rrf_k: Option<f64>,
    pub retrievers: crate::types::SourceMap<RetrieverStats>,
    pub fused: Vec<FusedHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank: Option<RerankStats>,
}

/// `path.resolve`.
fn resolve(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `getEmbsearchStoreDir`: `<agentDir>/embsearch/<sha256(repo)[..16]>`.
pub fn embsearch_store_dir(repo_root: &Path) -> PathBuf {
    let resolved = resolve(repo_root);
    let digest = Sha256::digest(resolved.to_string_lossy().as_bytes());
    let hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    cortexcode_code_paths::agent_dir()
        .join("embsearch")
        .join(hash)
}

/// `getSearchTracePath`.
pub fn search_trace_path(cwd: &Path) -> PathBuf {
    embsearch_store_dir(cwd).join(TRACE_FILE)
}

/// `writeSearchTrace`: append one record, rotating past 5 MiB; failures are
/// ignored.
pub fn write_search_trace(cwd: &Path, trace: &SearchTrace) {
    let _ = (|| -> std::io::Result<()> {
        let dir = embsearch_store_dir(cwd);
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(TRACE_FILE);
        if std::fs::metadata(&file).is_ok_and(|m| m.len() >= TRACE_ROTATE_BYTES) {
            let mut rotated = file.clone().into_os_string();
            rotated.push(".1");
            let _ = std::fs::rename(&file, rotated);
        }
        let line = serde_json::to_string(trace)?;
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)?;
        writeln!(out, "{line}")
    })();
}
