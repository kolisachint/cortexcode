//! Shared test setup: the process-wide theme, one test at a time.

#![allow(dead_code)]

use std::sync::{Mutex, MutexGuard};

use cortexcode_ai_types::{AssistantMessage, Content, StopReason};

pub fn lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("COLORTERM", "truecolor");
    cortexcode_code_tui_theme::init_theme(Some("dark"), false);
    guard
}

pub fn assistant(content: Vec<Content>) -> AssistantMessage {
    AssistantMessage {
        content,
        api: "openai-responses".into(),
        provider: "openai".into(),
        model: "gpt-4o-mini".into(),
        stop_reason: StopReason::Stop,
        ..Default::default()
    }
}
