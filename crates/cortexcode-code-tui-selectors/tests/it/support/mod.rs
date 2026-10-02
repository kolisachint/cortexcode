//! Shared test setup: the process-wide theme and the default keybindings.

#![allow(dead_code)]

use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use cortexcode_code_tui_keybindings::AppKeybindingsManager;

pub fn lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("COLORTERM", "truecolor");
    cortexcode_code_tui_theme::init_theme(Some("dark"), false);
    AppKeybindingsManager::create(Some(agent_dir())).install();
    guard
}

pub fn agent_dir() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

pub fn strip(lines: &[String]) -> String {
    cortexcode_tui_util::strip_vt_control_characters(&lines.join("\n"))
}
