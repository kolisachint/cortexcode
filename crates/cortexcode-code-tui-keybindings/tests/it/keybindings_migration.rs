//! Port of the pin's `test/keybindings-migration.test.ts` (the file rewrite
//! is `migrations.ts`'s keybindings step, [`migrate_keybindings_config_file`]).

use std::collections::HashMap;
use std::path::PathBuf;

use cortexcode_code_tui_keybindings::*;
use serde_json::{json, Value};

fn create_agent_dir(config: Value) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    std::fs::write(
        path.join("keybindings.json"),
        format!("{}\n", serde_json::to_string_pretty(&config).unwrap()),
    )
    .unwrap();
    (dir, path)
}

fn read(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path.join("keybindings.json")).unwrap()).unwrap()
}

#[test]
fn rewrites_old_key_names_to_namespaced_ids() {
    let (_dir, agent_dir) =
        create_agent_dir(json!({"cursorUp": ["up", "ctrl+p"], "expandTools": "ctrl+x"}));
    migrate_keybindings_config_file(&agent_dir);
    assert_eq!(
        read(&agent_dir),
        json!({"tui.editor.cursorUp": ["up", "ctrl+p"], "app.tools.expand": "ctrl+x"})
    );
}

#[test]
fn keeps_the_namespaced_value_when_old_and_new_names_both_exist() {
    let (_dir, agent_dir) =
        create_agent_dir(json!({"expandTools": "ctrl+x", "app.tools.expand": "ctrl+y"}));
    migrate_keybindings_config_file(&agent_dir);
    assert_eq!(read(&agent_dir), json!({"app.tools.expand": "ctrl+y"}));
}

#[test]
fn loads_old_key_names_in_memory_before_the_file_is_rewritten() {
    let (_dir, agent_dir) =
        create_agent_dir(json!({"selectConfirm": "enter", "interrupt": "ctrl+x"}));
    let keybindings = AppKeybindingsManager::create(Some(&agent_dir));
    let expected: HashMap<String, Vec<String>> = [
        ("tui.select.confirm".to_string(), vec!["enter".to_string()]),
        ("app.interrupt".to_string(), vec!["ctrl+x".to_string()]),
    ]
    .into_iter()
    .collect();
    assert_eq!(keybindings.get_user_bindings(), &expected);
    let effective = keybindings.get_effective_config();
    assert_eq!(effective["tui.select.confirm"], vec!["enter"]);
    assert_eq!(effective["app.interrupt"], vec!["ctrl+x"]);
}

#[test]
fn leaves_an_unmigrated_or_malformed_file_alone() {
    let (_dir, agent_dir) = create_agent_dir(json!({"app.tools.expand": "ctrl+y"}));
    let before = std::fs::read_to_string(agent_dir.join("keybindings.json")).unwrap();
    migrate_keybindings_config_file(&agent_dir);
    assert_eq!(
        std::fs::read_to_string(agent_dir.join("keybindings.json")).unwrap(),
        before
    );

    std::fs::write(agent_dir.join("keybindings.json"), "{ nope").unwrap();
    migrate_keybindings_config_file(&agent_dir);
    assert_eq!(
        std::fs::read_to_string(agent_dir.join("keybindings.json")).unwrap(),
        "{ nope"
    );
    // And a malformed file loads as no user bindings.
    assert!(AppKeybindingsManager::create(Some(&agent_dir))
        .get_user_bindings()
        .is_empty());
}

#[test]
fn reload_picks_up_an_edited_file() {
    let (_dir, agent_dir) = create_agent_dir(json!({"app.interrupt": "ctrl+x"}));
    let mut keybindings = AppKeybindingsManager::create(Some(&agent_dir));
    assert_eq!(keybindings.get_keys("app.interrupt"), vec!["ctrl+x"]);
    std::fs::write(
        agent_dir.join("keybindings.json"),
        r#"{"interrupt": ["ctrl+q"]}"#,
    )
    .unwrap();
    keybindings.reload();
    assert_eq!(keybindings.get_keys("app.interrupt"), vec!["ctrl+q"]);
}
