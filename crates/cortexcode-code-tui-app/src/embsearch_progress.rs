//! Semantic-index progress routing (`core/embsearch/embsearch-progress.ts`):
//! interactive mode shows it as a transient footer line; other modes log a
//! dim stderr line.

use crate::startup_progress::{self, StartupProgress};

/// Footer key for the semantic index (download and build share one line).
pub const SEMANTIC_INDEX_PROGRESS_KEY: &str = "semantic-index";

/// `EmbsearchState`.
#[derive(Debug, Clone, PartialEq)]
pub enum EmbsearchState {
    Idle,
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
    Skipped {
        reason: String,
    },
    Unavailable {
        reason: String,
    },
}

/// `reportEmbsearchProgress`. `log` receives the non-interactive lines
/// (unstyled; the caller dims them).
pub fn report_embsearch_progress(
    state: &EmbsearchState,
    interactive: bool,
    log: &mut dyn FnMut(&str),
) {
    let key = SEMANTIC_INDEX_PROGRESS_KEY.to_string();
    if interactive {
        match state {
            EmbsearchState::Downloading {
                received_bytes,
                total_bytes,
            } => startup_progress::set(StartupProgress::Download {
                key,
                label: "Semantic search index".into(),
                received_bytes: *received_bytes,
                total_bytes: *total_bytes,
            }),
            EmbsearchState::Indexing { done, total } => {
                startup_progress::set(StartupProgress::Work {
                    key,
                    label: "Building semantic search index".into(),
                    done: *done,
                    total: *total,
                    unit: "files".into(),
                })
            }
            EmbsearchState::Ready { .. } | EmbsearchState::Skipped { .. } => {
                startup_progress::remove(SEMANTIC_INDEX_PROGRESS_KEY)
            }
            EmbsearchState::Unavailable { reason } => {
                startup_progress::set(StartupProgress::Error {
                    key,
                    label: "Semantic search index unavailable".into(),
                    message: reason.clone(),
                })
            }
            EmbsearchState::Idle => {}
        }
        return;
    }
    match state {
        EmbsearchState::Indexing { done, total } => {
            let pct = ((*done as f64 / *total as f64) * 100.0 + 0.5).floor() as i64;
            log(&format!(
                "Building semantic search index – {done}/{total} files ({pct}%)"
            ));
        }
        EmbsearchState::Ready { chunk_count } => log(&format!(
            "Semantic search index ready ({chunk_count} chunks)"
        )),
        EmbsearchState::Skipped { reason } => {
            log(&format!("Semantic search index skipped ({reason})"))
        }
        EmbsearchState::Unavailable { reason } => {
            log(&format!("Semantic search index unavailable ({reason})"))
        }
        _ => {}
    }
}
