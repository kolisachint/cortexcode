//! Port of the pin's `suite/regressions/2753-reload-stale-resource-settings.test.ts`.
//! `AgentSession::reload` is `settings.reload()` then `resource_loader.reload()`
//! over one shared `SettingsManager`; the regression was the loader reading a
//! stale copy, so the case runs on that pair directly (the TS registers its
//! faux provider through an extension only to get a session up).

use std::sync::{Arc, Mutex};

use cortexcode_code_resources::{DefaultResourceLoader, DefaultResourceLoaderOptions};
use cortexcode_code_settings::SettingsManager;

fn prompt_names(loader: &DefaultResourceLoader) -> Vec<String> {
    loader
        .prompts()
        .prompts
        .into_iter()
        .map(|p| p.name)
        .collect()
}

#[test]
fn applies_updated_top_level_prompt_settings_on_reload() {
    let root = tempfile::tempdir().unwrap();
    let agent_dir = root.path().join("agent");
    std::fs::create_dir_all(agent_dir.join("prompts")).unwrap();
    std::fs::write(agent_dir.join("prompts/test.md"), "Echo test prompt\n").unwrap();

    let settings = Arc::new(Mutex::new(SettingsManager::create(root.path(), &agent_dir)));
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.path().to_string_lossy().into_owned(),
        agent_dir: agent_dir.to_string_lossy().into_owned(),
        settings: Some(settings.clone()),
        no_skills: true,
        home: Some(root.path().join("home").to_string_lossy().into_owned()),
        user_agents_dir: Some(root.path().join("agents").to_string_lossy().into_owned()),
        ..Default::default()
    });
    loader.reload();
    assert!(prompt_names(&loader).contains(&"test".to_string()));

    std::fs::write(
        agent_dir.join("settings.json"),
        "{\n  \"prompts\": [\n    \"-prompts/test.md\"\n  ]\n}\n",
    )
    .unwrap();
    settings.lock().unwrap().reload();
    loader.reload();

    assert_eq!(
        settings.lock().unwrap().global_settings()["prompts"],
        serde_json::json!(["-prompts/test.md"])
    );
    assert!(!prompt_names(&loader).contains(&"test".to_string()));
}
