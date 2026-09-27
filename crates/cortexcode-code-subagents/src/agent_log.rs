//! `core/agent-log.ts`: the agent's operational log lines (subagent dispatch,
//! warm-worker fallback, lifeguard kills).
//!
//! They go to stderr, except while an interactive TUI owns the terminal: a
//! write the differential renderer did not make shifts its frame. Then they
//! are withheld (everything is also in `dispatch-log.json` and the task
//! panel); `<prefix>DEBUG_AGENT_LOG=1` tees them to the debug log (`<agent dir>/<app>-debug.log`).

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static OWNED_BY_TUI: AtomicBool = AtomicBool::new(false);

/// Declare whether a TUI currently owns the terminal.
pub fn set_terminal_owned_by_tui(owned: bool) {
    OWNED_BY_TUI.store(owned, Ordering::SeqCst);
}

/// Whether operational logging is being withheld from the terminal.
pub fn is_terminal_owned_by_tui() -> bool {
    OWNED_BY_TUI.load(Ordering::SeqCst)
}

/// Emit one operational log line, unless a TUI owns the terminal.
pub fn agent_log(message: &str) {
    if is_terminal_owned_by_tui() {
        debug_tee(message);
        return;
    }
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Best-effort file tee for suppressed lines, off unless enabled.
fn debug_tee(message: &str) {
    if cortexcode_code_paths::env_override("DEBUG_AGENT_LOG").as_deref() != Some("1") {
        return;
    }
    let path = cortexcode_code_paths::debug_log_path();
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{now}] {message}");
    }
}
