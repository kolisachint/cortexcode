//! Port of the pin's `coding-agent/test/session-cwd.test.ts`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cortexcode_code_agent_session::runtime::{
    create_agent_session_runtime, get_missing_session_cwd_issue, RuntimeError, RuntimeRequest,
    SessionCwdIssue,
};
use cortexcode_code_session::SessionManager;

fn write_session_file(path: &Path, cwd: &Path) {
    let header = serde_json::json!({
        "type": "session",
        "version": 3,
        "id": "session-id",
        "timestamp": "2026-01-01T00:00:00.000Z",
        "cwd": cwd,
    });
    std::fs::write(path, format!("{header}\n")).unwrap();
}

/// (fallback cwd, the session file whose stored cwd is missing, temp root).
fn fixture() -> (PathBuf, PathBuf, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let fallback = root.path().join("fallback");
    std::fs::create_dir_all(&fallback).unwrap();
    let file = root.path().join("session.jsonl");
    write_session_file(&file, &fallback.join("does-not-exist"));
    (fallback, file, root)
}

#[test]
fn detects_missing_session_cwd_from_persisted_sessions() {
    let (fallback, file, _root) = fixture();
    let manager = SessionManager::open(&file, None, None);
    assert_eq!(
        get_missing_session_cwd_issue(&manager, &fallback),
        Some(SessionCwdIssue {
            session_file: Some(file.to_string_lossy().into_owned()),
            session_cwd: fallback
                .join("does-not-exist")
                .to_string_lossy()
                .into_owned(),
            fallback_cwd: fallback.to_string_lossy().into_owned(),
        })
    );
}

#[test]
fn supports_overriding_the_effective_cwd_when_opening_a_session() {
    let (fallback, file, _root) = fixture();
    let fallback_s = fallback.to_string_lossy().into_owned();
    let manager = SessionManager::open(&file, None, Some(fallback_s.clone()));
    assert_eq!(manager.cwd(), fallback_s);
    assert_eq!(get_missing_session_cwd_issue(&manager, &fallback), None);
}

#[tokio::test]
async fn errors_before_runtime_creation_when_the_stored_cwd_is_missing() {
    let (fallback, file, _root) = fixture();
    let called = Arc::new(AtomicBool::new(false));
    let flag = called.clone();
    let result = create_agent_session_runtime(
        Arc::new(move |_| {
            flag.store(true, Ordering::SeqCst);
            Box::pin(async { Err("should not be called".to_string()) })
        }),
        RuntimeRequest {
            cwd: fallback.clone(),
            agent_dir: fallback,
            session_manager: SessionManager::open(&file, None, None),
            session_start_event: None,
        },
    )
    .await;
    assert!(matches!(result, Err(RuntimeError::MissingSessionCwd(_))));
    assert!(!called.load(Ordering::SeqCst));
}
