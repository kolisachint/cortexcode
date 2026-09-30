//! Port of the pin's `suite/regressions/3616-settings-inmemory-reload.test.ts`.
//! The `DefaultResourceLoader.reload()` case re-reads settings through the same
//! `SettingsManager::reload`, so it is covered by the direct-reload case.

use cortexcode_code_settings::SettingsManager;
use serde_json::{json, Value};

fn in_memory(v: Value) -> SettingsManager {
    SettingsManager::in_memory(v.as_object().unwrap().clone())
}

#[test]
fn preserves_initial_settings_after_direct_reload() {
    let initial = json!({
        "defaultThinkingLevel": "high",
        "images": {"autoResize": false},
        "compaction": {"enabled": false},
    });
    let mut m = in_memory(initial.clone());
    m.reload();
    assert_eq!(
        m.default_thinking_level()
            .map(cortexcode_ai_types::ThinkingLevel::from),
        Some(cortexcode_ai_types::ThinkingLevel::High)
    );
    assert!(!m.image_auto_resize());
    assert!(!m.compaction_enabled());
    assert_eq!(Value::Object(m.global_settings()), initial);
}

#[test]
fn preserves_initial_settings_after_an_unrelated_setter_flush_and_reload() {
    let mut m =
        in_memory(json!({"images": {"autoResize": false}, "compaction": {"enabled": false}}));
    m.set_theme("dark");
    m.flush();
    m.reload();
    assert_eq!(m.theme().as_deref(), Some("dark"));
    assert!(!m.image_auto_resize());
    assert!(!m.compaction_enabled());
    assert_eq!(
        Value::Object(m.global_settings()),
        json!({"images": {"autoResize": false}, "compaction": {"enabled": false}, "theme": "dark"})
    );
}
