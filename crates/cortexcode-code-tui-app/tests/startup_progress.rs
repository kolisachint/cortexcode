//! Port of the pin's `test/startup-progress.test.ts`: the store and the
//! embsearch state mapping (the footer rendering cases are in `footer.rs`).

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cortexcode_code_tui_app::embsearch_progress::*;
use cortexcode_code_tui_app::startup_progress::{self, StartupProgress};
use support::lock;

fn dl(key: &str, label: &str, received: u64, total: u64) -> StartupProgress {
    StartupProgress::Download {
        key: key.into(),
        label: label.into(),
        received_bytes: received,
        total_bytes: Some(total),
    }
}

fn keys() -> Vec<String> {
    startup_progress::list()
        .iter()
        .map(|e| e.key().to_string())
        .collect()
}

#[test]
fn set_list_keeps_entries_in_insertion_order() {
    let _g = lock(None);
    startup_progress::set(dl("fd", "fd", 1, 10));
    startup_progress::set(dl("rg", "ripgrep", 2, 20));
    assert_eq!(keys(), ["fd", "rg"]);
}

#[test]
fn updating_an_existing_key_replaces_in_place_without_reordering() {
    let _g = lock(None);
    startup_progress::set(dl("fd", "fd", 1, 10));
    startup_progress::set(dl("rg", "ripgrep", 2, 20));
    startup_progress::set(dl("fd", "fd", 9, 10));
    assert_eq!(keys(), ["fd", "rg"]);
    assert_eq!(startup_progress::list()[0], dl("fd", "fd", 9, 10));
}

#[test]
fn remove_drops_one_key_clear_wipes_all() {
    let _g = lock(None);
    startup_progress::set(dl("fd", "fd", 1, 10));
    startup_progress::set(dl("rg", "ripgrep", 2, 20));
    startup_progress::remove("fd");
    assert_eq!(keys(), ["rg"]);
    startup_progress::clear();
    assert!(startup_progress::list().is_empty());
}

#[test]
fn subscribe_fires_on_set_remove_clear_and_stops_after_unsubscribe() {
    let _g = lock(None);
    let n = Arc::new(AtomicUsize::new(0));
    let sink = n.clone();
    let sub = startup_progress::subscribe(move || {
        sink.fetch_add(1, Ordering::SeqCst);
    });
    startup_progress::set(dl("fd", "fd", 1, 10));
    startup_progress::remove("fd");
    startup_progress::clear(); // already empty: no notification
    assert_eq!(n.load(Ordering::SeqCst), 2);
    sub.unsubscribe();
    startup_progress::set(dl("rg", "ripgrep", 1, 2));
    assert_eq!(n.load(Ordering::SeqCst), 2);
}

#[test]
fn removing_an_unknown_key_does_not_notify() {
    let _g = lock(None);
    let n = Arc::new(AtomicUsize::new(0));
    let sink = n.clone();
    let sub = startup_progress::subscribe(move || {
        sink.fetch_add(1, Ordering::SeqCst);
    });
    startup_progress::remove("nope");
    assert_eq!(n.load(Ordering::SeqCst), 0);
    sub.unsubscribe();
}

fn set(state: EmbsearchState) {
    report_embsearch_progress(&state, true, &mut |_| {});
}

fn index_entry() -> Option<StartupProgress> {
    startup_progress::list()
        .into_iter()
        .find(|e| e.key() == SEMANTIC_INDEX_PROGRESS_KEY)
}

#[test]
fn downloading_then_indexing_share_one_evolving_key() {
    let _g = lock(None);
    set(EmbsearchState::Downloading {
        received_bytes: 1024,
        total_bytes: Some(4096),
    });
    assert_eq!(startup_progress::list().len(), 1);
    assert!(matches!(
        index_entry(),
        Some(StartupProgress::Download {
            received_bytes: 1024,
            total_bytes: Some(4096),
            ..
        })
    ));
    set(EmbsearchState::Indexing { done: 3, total: 12 });
    assert_eq!(startup_progress::list().len(), 1);
    match index_entry() {
        Some(StartupProgress::Work {
            label,
            done: 3,
            total: 12,
            ..
        }) => assert_eq!(label, "Building semantic search index"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn ready_and_skipped_drop_the_line() {
    let _g = lock(None);
    set(EmbsearchState::Indexing { done: 1, total: 2 });
    set(EmbsearchState::Ready { chunk_count: 99 });
    assert!(index_entry().is_none());
    set(EmbsearchState::Indexing { done: 1, total: 2 });
    set(EmbsearchState::Skipped {
        reason: "under threshold".into(),
    });
    assert!(index_entry().is_none());
}

#[test]
fn unavailable_shows_a_transient_error_entry_and_idle_nothing() {
    let _g = lock(None);
    set(EmbsearchState::Idle);
    assert!(startup_progress::list().is_empty());
    set(EmbsearchState::Unavailable {
        reason: "binary not found".into(),
    });
    assert!(
        matches!(index_entry(), Some(StartupProgress::Error { message, .. }) if message == "binary not found")
    );
}

fn collect(state: EmbsearchState) -> Vec<String> {
    let mut logs = Vec::new();
    report_embsearch_progress(&state, false, &mut |m| logs.push(m.to_string()));
    logs
}

#[test]
fn non_interactive_logs_plain_language_lines_and_never_touches_the_store() {
    let _g = lock(None);
    assert_eq!(
        collect(EmbsearchState::Indexing { done: 3, total: 12 }),
        ["Building semantic search index – 3/12 files (25%)"]
    );
    assert_eq!(
        collect(EmbsearchState::Ready { chunk_count: 42 }),
        ["Semantic search index ready (42 chunks)"]
    );
    assert_eq!(
        collect(EmbsearchState::Skipped {
            reason: "too small".into()
        }),
        ["Semantic search index skipped (too small)"]
    );
    assert_eq!(
        collect(EmbsearchState::Unavailable {
            reason: "offline".into()
        }),
        ["Semantic search index unavailable (offline)"]
    );
    assert!(collect(EmbsearchState::Downloading {
        received_bytes: 1,
        total_bytes: Some(2)
    })
    .is_empty());
    assert!(collect(EmbsearchState::Idle).is_empty());
    assert!(startup_progress::list().is_empty());
}
