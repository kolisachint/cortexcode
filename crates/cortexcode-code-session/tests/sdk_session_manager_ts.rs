//! Port of the pin's `coding-agent/test/sdk-session-manager.test.ts`.
//!
//! `createAgentSession` there picks a default `SessionManager` and cwd; the
//! Rust `create_agent_session` always takes both explicitly (the caller builds
//! the manager), so "keeps an explicit sessionManager override" holds by the
//! signature and "derives cwd when omitted" has no omitted cwd to derive. The
//! default-path case ports onto the default manager, `SessionManager::create`.
//! (Its own test binary: it sets the agent-dir env var.)

use cortexcode_code_session::SessionManager;

#[test]
fn uses_the_agent_dir_for_the_default_persisted_session_path() {
    let root = std::env::temp_dir().join(format!("sdk-session-test-{}", std::process::id()));
    let (cwd, agent_dir) = (root.join("project"), root.join("agent"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::env::set_var("CORTEXCODE_CODING_AGENT_DIR", &agent_dir);

    let cwd_s = cwd.to_string_lossy().into_owned();
    let mut manager = SessionManager::create(cwd_s.clone(), None);
    let safe = format!(
        "--{}--",
        cwd_s
            .trim_start_matches(['/', '\\'])
            .replace(['/', '\\', ':'], "-")
    );
    let expected = agent_dir.join("sessions").join(safe);
    assert_eq!(manager.session_dir(), expected);
    assert!(manager.is_persisted());
    let file = manager
        .new_session(Default::default())
        .expect("a session file");
    assert!(file.starts_with(&expected), "{}", file.display());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_in_memory_manager_is_not_persisted() {
    assert!(!SessionManager::in_memory("/tmp/x").is_persisted());
}
