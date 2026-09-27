//! mode-subagent-appendix.test.ts: a spawned subagent (depth env set) gets no
//! mode appendix; a top-level session keeps it. One test: it sets process env.

use cortexcode_code_modes::config::HooConfig;
use cortexcode_code_modes::extension::{append_mode_prompt, resolve_active_mode, ModeSession};

fn prompt_with_mode() -> Option<String> {
    let cwd = tempfile::tempdir().unwrap();
    let session = ModeSession {
        cwd: cwd.path().to_path_buf(),
        session_id: "s1".into(),
        light: false,
        mode_search_paths: Vec::new(),
    };
    let active = resolve_active_mode(&session, &HooConfig::new());
    append_mode_prompt("BASE", &active)
}

#[test]
fn mode_appendix_only_in_top_level_sessions() {
    for prefix in cortexcode_code_paths::ENV_PREFIXES {
        std::env::remove_var(format!("{prefix}SUBAGENT_DEPTH"));
    }
    let top = prompt_with_mode().unwrap();
    assert!(top.contains("<!-- hoo-core: mode="));

    std::env::set_var("HOOCODE_SUBAGENT_DEPTH", "1");
    let child = prompt_with_mode();
    std::env::remove_var("HOOCODE_SUBAGENT_DEPTH");
    assert!(!child.unwrap_or_default().contains("<!-- hoo-core: mode="));
}
