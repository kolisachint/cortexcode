//! Port of hoocode `test/settings-manager.test.ts` and
//! `test/settings-manager-bug.test.ts` (v0.5.89). Project settings live in
//! `.cortexcode/` (hoocode: `.hoocode/`); the hoocode fallback has its own
//! tests at the end.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cortexcode_code_settings::*;
use serde_json::{json, Value};

struct Dirs {
    root: PathBuf,
    agent: PathBuf,
    project: PathBuf,
}

impl Dirs {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("test-settings-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let agent = root.join("agent");
        let project = root.join("project");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(project.join(".cortexcode")).unwrap();
        Self {
            root,
            agent,
            project,
        }
    }

    fn global_path(&self) -> PathBuf {
        self.agent.join("settings.json")
    }

    fn project_path(&self) -> PathBuf {
        self.project.join(".cortexcode/settings.json")
    }

    fn manager(&self) -> SettingsManager {
        SettingsManager::create(&self.project, &self.agent)
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn edit(path: &Path, f: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    let mut value = read(path);
    f(value.as_object_mut().unwrap());
    write(path, &value);
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

// --- preserves externally added settings ---

#[test]
fn preserves_enabled_models_when_changing_thinking_level() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"theme": "dark", "defaultModel": "claude-sonnet"}),
    );
    let mut manager = d.manager();
    edit(&d.global_path(), |s| {
        s.insert(
            "enabledModels".into(),
            json!(["claude-opus-4-5", "gpt-5.2-codex"]),
        );
    });
    manager.set_default_thinking_level(ThinkingLevelSetting::High);
    manager.flush();

    let saved = read(&d.global_path());
    assert_eq!(
        saved["enabledModels"],
        json!(["claude-opus-4-5", "gpt-5.2-codex"])
    );
    assert_eq!(saved["defaultThinkingLevel"], "high");
    assert_eq!(saved["theme"], "dark");
    assert_eq!(saved["defaultModel"], "claude-sonnet");
}

#[test]
fn preserves_custom_settings_when_changing_theme() {
    let d = Dirs::new();
    write(&d.global_path(), &json!({"defaultModel": "claude-sonnet"}));
    let mut manager = d.manager();
    edit(&d.global_path(), |s| {
        s.insert("shellPath".into(), json!("/bin/zsh"));
        s.insert("extensions".into(), json!(["/path/to/extension.ts"]));
    });
    manager.set_theme("light");

    let saved = read(&d.global_path());
    assert_eq!(saved["shellPath"], "/bin/zsh");
    assert_eq!(saved["extensions"], json!(["/path/to/extension.ts"]));
    assert_eq!(saved["theme"], "light");
}

#[test]
fn in_memory_changes_override_file_changes_for_the_same_key() {
    let d = Dirs::new();
    write(&d.global_path(), &json!({"theme": "dark"}));
    let mut manager = d.manager();
    edit(&d.global_path(), |s| {
        s.insert("defaultThinkingLevel".into(), json!("low"));
    });
    manager.set_default_thinking_level(ThinkingLevelSetting::High);
    assert_eq!(read(&d.global_path())["defaultThinkingLevel"], "high");
}

// --- disabledTools ---

#[test]
fn disabled_tools_default_round_trip_and_preservation() {
    let d = Dirs::new();
    assert!(d.manager().disabled_tools().is_empty());

    let mut manager = d.manager();
    manager.set_disabled_tools(&strings(&["bash", "write"]));
    assert_eq!(
        read(&d.global_path())["disabledTools"],
        json!(["bash", "write"])
    );
    assert_eq!(d.manager().disabled_tools(), strings(&["bash", "write"]));

    write(
        &d.global_path(),
        &json!({"theme": "dark", "defaultModel": "claude-sonnet"}),
    );
    let mut manager = d.manager();
    manager.set_disabled_tools(&strings(&["bash"]));
    let saved = read(&d.global_path());
    assert_eq!(saved["disabledTools"], json!(["bash"]));
    assert_eq!(saved["theme"], "dark");
    assert_eq!(saved["defaultModel"], "claude-sonnet");
}

// --- toolOutputView ---

#[test]
fn tool_output_view_defaults_round_trips_and_rejects_bogus() {
    let d = Dirs::new();
    assert_eq!(d.manager().tool_output_view(), ToolOutputView::Peek);
    let mut manager = d.manager();
    manager.set_tool_output_view(ToolOutputView::Radar);
    assert_eq!(read(&d.global_path())["toolOutputView"], "radar");
    assert_eq!(d.manager().tool_output_view(), ToolOutputView::Radar);

    write(&d.global_path(), &json!({"toolOutputView": "bogus"}));
    assert_eq!(d.manager().tool_output_view(), ToolOutputView::Peek);
}

#[test]
fn tool_output_view_reads_retired_values_and_prefers_explicit() {
    let d = Dirs::new();
    let read_view = |value: &str| {
        write(&d.global_path(), &json!({"toolOutputDisplay": value}));
        d.manager().tool_output_view()
    };
    assert_eq!(read_view("collapsed"), ToolOutputView::Radar);
    assert_eq!(read_view("glance"), ToolOutputView::Peek);
    assert_eq!(read_view("standard"), ToolOutputView::Full);
    assert_eq!(read_view("peek"), ToolOutputView::Peek);

    write(
        &d.global_path(),
        &json!({"toolOutputDisplay": "standard", "toolOutputView": "radar"}),
    );
    assert_eq!(d.manager().tool_output_view(), ToolOutputView::Radar);
}

// --- tool settings (output caps + context GC) ---

#[test]
fn tool_output_caps_and_context_gc_round_trip_and_clamp() {
    let d = Dirs::new();
    let mut manager = d.manager();
    manager.set_tool_output_max_bytes(65536);
    manager.set_tool_output_max_lines(1600);
    manager.set_context_gc_enabled(false);

    let saved = read(&d.global_path());
    assert_eq!(saved["toolOutput"]["maxBytes"], 65536);
    assert_eq!(saved["toolOutput"]["maxLines"], 1600);
    assert_eq!(saved["contextGc"]["enabled"], false);

    let mut reloaded = d.manager();
    assert_eq!(reloaded.tool_output_max_bytes(), 65536);
    assert_eq!(reloaded.tool_output_max_lines(), 1600);
    assert!(!reloaded.context_gc_enabled());

    reloaded.set_tool_output_max_bytes(10);
    reloaded.set_tool_output_max_lines(0);
    assert_eq!(reloaded.tool_output_max_bytes(), 1024);
    assert_eq!(reloaded.tool_output_max_lines(), 1);
}

#[test]
fn preserves_unrelated_tool_output_keys_when_updating_one_cap() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"toolOutput": {"maxBytes": 32768, "maxLines": 800}}),
    );
    let mut manager = d.manager();
    manager.set_tool_output_max_lines(400);
    let saved = read(&d.global_path());
    assert_eq!(saved["toolOutput"]["maxLines"], 400);
    assert_eq!(saved["toolOutput"]["maxBytes"], 32768);
}

// --- voice silence window ---

#[test]
fn voice_silence_defaults_round_trips_and_clamps() {
    let d = Dirs::new();
    let mut manager = d.manager();
    assert_eq!(manager.voice_silence_ms(), 800);
    manager.set_voice_silence_ms(1500);
    assert_eq!(read(&d.global_path())["voice"]["silenceMs"], 1500);
    assert_eq!(d.manager().voice_silence_ms(), 1500);

    let mut manager = d.manager();
    manager.set_voice_silence_ms(50);
    assert_eq!(manager.voice_silence_ms(), 300);
    manager.set_voice_silence_ms(999999);
    assert_eq!(manager.voice_silence_ms(), 10000);

    write(&d.global_path(), &json!({"voice": {"silenceMs": 100}}));
    assert_eq!(d.manager().voice_silence_ms(), 300);
}

// --- webtools timeout ---

fn with_webtools_env(value: Option<&str>, f: impl FnOnce()) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let names = [
        "CORTEXCODE_WEBTOOLS_TIMEOUT",
        "CORTEX_WEBTOOLS_TIMEOUT",
        "HOOCODE_WEBTOOLS_TIMEOUT",
    ];
    let saved: Vec<_> = names.iter().map(std::env::var_os).collect();
    for n in names {
        std::env::remove_var(n);
    }
    if let Some(value) = value {
        std::env::set_var("HOOCODE_WEBTOOLS_TIMEOUT", value);
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    for (n, v) in names.iter().zip(saved) {
        match v {
            Some(v) => std::env::set_var(n, v),
            None => std::env::remove_var(n),
        }
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn webtools_timeout_defaults_round_trips_and_clamps() {
    with_webtools_env(None, || {
        let d = Dirs::new();
        let mut manager = d.manager();
        assert_eq!(manager.webtools_timeout_secs(), 15);
        manager.set_webtools_timeout_secs(45);
        assert_eq!(read(&d.global_path())["webtools"]["timeoutSecs"], 45);
        assert_eq!(d.manager().webtools_timeout_secs(), 45);

        let mut manager = d.manager();
        manager.set_webtools_timeout_secs(0);
        assert_eq!(manager.webtools_timeout_secs(), 1);
        manager.set_webtools_timeout_secs(999);
        assert_eq!(manager.webtools_timeout_secs(), 120);

        write(
            &d.global_path(),
            &json!({"webtools": {"timeoutSecs": "nope"}}),
        );
        assert_eq!(d.manager().webtools_timeout_secs(), 15);
    });
}

#[test]
fn webtools_timeout_env_is_used_when_unset_but_the_setting_wins() {
    let d = Dirs::new();
    with_webtools_env(Some("999"), || {
        assert_eq!(d.manager().webtools_timeout_secs(), 120);
        write(&d.global_path(), &json!({"webtools": {"timeoutSecs": 30}}));
        assert_eq!(d.manager().webtools_timeout_secs(), 30);
    });
    std::fs::remove_file(d.global_path()).unwrap();
    with_webtools_env(Some("not-a-number"), || {
        assert_eq!(d.manager().webtools_timeout_secs(), 15);
    });
}

// --- enableEmbsearchTools -> enableSemanticIndex migration ---

#[test]
fn semantic_index_migration() {
    let d = Dirs::new();
    write(&d.global_path(), &json!({"enableEmbsearchTools": false}));
    assert!(!d.manager().enable_semantic_index());
    write(
        &d.global_path(),
        &json!({"enableEmbsearchTools": false, "enableSemanticIndex": true}),
    );
    assert!(d.manager().enable_semantic_index());
    write(&d.global_path(), &json!({}));
    assert!(d.manager().enable_semantic_index());
}

// --- webtools search ---

#[test]
fn webtools_search_reads_the_user_level_block_only() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"webtools": {"search": {"provider": "brave", "providers": {"brave": {"api_key": "k"}}}}}),
    );
    let search = d.manager().webtools_search().unwrap();
    assert_eq!(search["provider"], "brave");
    assert_eq!(search["providers"]["brave"]["api_key"], "k");

    std::fs::remove_file(d.global_path()).unwrap();
    write(
        &d.project_path(),
        &json!({"webtools": {"search": {"providers": {"tavily": {"api_key": "k"}}}}}),
    );
    assert_eq!(d.manager().webtools_search(), None);
}

// --- flag overrides ---

#[test]
fn flag_overrides_round_trip_and_clear() {
    let d = Dirs::new();
    let mut manager = d.manager();
    manager.set_flag_override("plan", FlagValue::Bool(true));
    manager.set_flag_override("endpoint", FlagValue::String("https://example.test".into()));
    assert_eq!(
        read(&d.global_path())["flags"],
        json!({"plan": true, "endpoint": "https://example.test"})
    );

    let mut reloaded = d.manager();
    assert_eq!(
        reloaded.flag_overrides(),
        vec![
            ("plan".to_string(), FlagValue::Bool(true)),
            (
                "endpoint".to_string(),
                FlagValue::String("https://example.test".into())
            ),
        ]
    );
    reloaded.clear_flag_override("plan");
    assert_eq!(
        read(&d.global_path())["flags"],
        json!({"endpoint": "https://example.test"})
    );
}

#[test]
fn preserves_externally_added_flag_keys() {
    let d = Dirs::new();
    write(&d.global_path(), &json!({"flags": {"external": "keep-me"}}));
    let mut manager = d.manager();
    manager.set_flag_override("plan", FlagValue::Bool(true));
    assert_eq!(
        read(&d.global_path())["flags"],
        json!({"external": "keep-me", "plan": true})
    );
}

// --- packages migration ---

#[test]
fn keeps_local_only_extensions_in_the_extensions_array() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"extensions": ["/local/ext.ts", "./relative/ext.ts"]}),
    );
    let manager = d.manager();
    assert!(manager.packages().is_empty());
    assert_eq!(
        manager.extension_paths(),
        strings(&["/local/ext.ts", "./relative/ext.ts"])
    );
}

#[test]
fn handles_packages_with_filtering_objects() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"packages": [
            "npm:simple-pkg",
            {"source": "npm:shitty-extensions", "extensions": ["extensions/oracle.ts"], "skills": []}
        ]}),
    );
    let packages = d.manager().packages();
    assert_eq!(packages.len(), 2);
    assert_eq!(packages[0], PackageSource::Source("npm:simple-pkg".into()));
    assert_eq!(
        serde_json::to_value(&packages[1]).unwrap(),
        json!({"source": "npm:shitty-extensions", "extensions": ["extensions/oracle.ts"], "skills": []})
    );
}

// --- reload ---

#[test]
fn reloads_global_settings_from_disk() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"theme": "dark", "extensions": ["/before.ts"]}),
    );
    let mut manager = d.manager();
    write(
        &d.global_path(),
        &json!({"theme": "light", "extensions": ["/after.ts"], "defaultModel": "claude-sonnet"}),
    );
    manager.reload();
    assert_eq!(manager.theme().as_deref(), Some("light"));
    assert_eq!(manager.extension_paths(), strings(&["/after.ts"]));
    assert_eq!(manager.default_model().as_deref(), Some("claude-sonnet"));
}

#[test]
fn keeps_previous_settings_when_the_file_is_invalid() {
    let d = Dirs::new();
    write(&d.global_path(), &json!({"theme": "dark"}));
    let mut manager = d.manager();
    std::fs::write(d.global_path(), "{ invalid json").unwrap();
    manager.reload();
    assert_eq!(manager.theme().as_deref(), Some("dark"));
}

// --- error tracking ---

#[test]
fn collects_and_clears_load_errors() {
    let d = Dirs::new();
    std::fs::write(d.global_path(), "{ invalid global json").unwrap();
    std::fs::write(d.project_path(), "{ invalid project json").unwrap();
    let mut manager = d.manager();
    let errors = manager.drain_errors();
    let mut scopes: Vec<_> = errors.iter().map(|e| e.scope.as_str()).collect();
    scopes.sort();
    assert_eq!(scopes, ["global", "project"]);
    assert!(manager.drain_errors().is_empty());
}

// --- project settings directory creation ---

#[test]
fn does_not_create_the_project_dir_when_only_reading() {
    let d = Dirs::new();
    write(&d.global_path(), &json!({"theme": "dark"}));
    std::fs::remove_dir_all(d.project.join(".cortexcode")).unwrap();
    let manager = d.manager();
    assert!(!d.project.join(".cortexcode").exists());
    assert_eq!(manager.theme().as_deref(), Some("dark"));
}

#[test]
fn creates_the_project_dir_when_writing_project_settings() {
    let d = Dirs::new();
    write(&d.global_path(), &json!({"theme": "dark"}));
    std::fs::remove_dir_all(d.project.join(".cortexcode")).unwrap();
    let mut manager = d.manager();
    assert!(!d.project.join(".cortexcode").exists());
    manager.set_project_packages(&[PackageSource::Filtered(PackageFilter {
        source: "npm:test-pkg".into(),
        ..Default::default()
    })]);
    manager.flush();
    assert!(d.project.join(".cortexcode").exists());
    assert!(d.project_path().exists());
    assert_eq!(
        read(&d.project_path()),
        json!({"packages": [{"source": "npm:test-pkg"}]})
    );
}

// --- shellCommandPrefix ---

#[test]
fn shell_command_prefix_load_unset_and_preserve() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"shellCommandPrefix": "shopt -s expand_aliases"}),
    );
    assert_eq!(
        d.manager().shell_command_prefix().as_deref(),
        Some("shopt -s expand_aliases")
    );
    let mut manager = d.manager();
    manager.set_theme("light");
    let saved = read(&d.global_path());
    assert_eq!(saved["shellCommandPrefix"], "shopt -s expand_aliases");
    assert_eq!(saved["theme"], "light");

    write(&d.global_path(), &json!({"theme": "dark"}));
    assert_eq!(d.manager().shell_command_prefix(), None);
}

// --- getSessionDir ---

#[test]
fn session_dir_unset_global_project_and_tilde() {
    let d = Dirs::new();
    write(&d.global_path(), &json!({"theme": "dark"}));
    assert_eq!(d.manager().session_dir(), None);

    write(&d.global_path(), &json!({"sessionDir": "/tmp/sessions"}));
    assert_eq!(
        d.manager().session_dir(),
        Some(PathBuf::from("/tmp/sessions"))
    );

    write(&d.global_path(), &json!({"sessionDir": "/global/sessions"}));
    write(&d.project_path(), &json!({"sessionDir": "./sessions"}));
    assert_eq!(d.manager().session_dir(), Some(PathBuf::from("./sessions")));

    std::fs::remove_file(d.project_path()).unwrap();
    write(&d.global_path(), &json!({"sessionDir": "~/sessions"}));
    let home = std::env::var("HOME").unwrap();
    assert_eq!(
        d.manager().session_dir(),
        Some(Path::new(&home).join("sessions"))
    );
}

// --- getPluginInstallScope ---

#[test]
fn plugin_install_scope() {
    let d = Dirs::new();
    write(&d.global_path(), &json!({"theme": "dark"}));
    assert_eq!(d.manager().plugin_install_scope(), PluginInstallScope::User);
    write(&d.global_path(), &json!({"pluginInstallScope": "project"}));
    assert_eq!(
        d.manager().plugin_install_scope(),
        PluginInstallScope::Project
    );
    write(&d.global_path(), &json!({"pluginInstallScope": "global"}));
    assert_eq!(d.manager().plugin_install_scope(), PluginInstallScope::User);
    write(&d.global_path(), &json!({"pluginInstallScope": "user"}));
    write(&d.project_path(), &json!({"pluginInstallScope": "project"}));
    assert_eq!(
        d.manager().plugin_install_scope(),
        PluginInstallScope::Project
    );
}

// --- settings-manager-bug.test.ts: external edit preservation ---

#[test]
fn preserves_file_changes_to_packages_when_changing_an_unrelated_setting() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"theme": "dark", "packages": ["npm:pi-mcp-adapter"]}),
    );
    let mut manager = d.manager();
    assert_eq!(
        manager.packages(),
        [PackageSource::Source("npm:pi-mcp-adapter".into())]
    );
    edit(&d.global_path(), |s| {
        s.insert("packages".into(), json!([]));
    });
    assert_eq!(read(&d.global_path())["packages"], json!([]));
    manager.set_theme("light");
    let saved = read(&d.global_path());
    assert_eq!(saved["packages"], json!([]));
    assert_eq!(saved["theme"], "light");
}

#[test]
fn preserves_file_changes_to_extensions_when_changing_an_unrelated_setting() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"theme": "dark", "extensions": ["/old/extension.ts"]}),
    );
    let mut manager = d.manager();
    edit(&d.global_path(), |s| {
        s.insert("extensions".into(), json!(["/new/extension.ts"]));
    });
    manager.set_default_thinking_level(ThinkingLevelSetting::High);
    assert_eq!(
        read(&d.global_path())["extensions"],
        json!(["/new/extension.ts"])
    );
}

#[test]
fn preserves_external_project_changes_when_updating_an_unrelated_project_field() {
    let d = Dirs::new();
    write(
        &d.project_path(),
        &json!({"extensions": ["./old-extension.ts"], "prompts": ["./old-prompt.md"]}),
    );
    let mut manager = d.manager();
    edit(&d.project_path(), |s| {
        s.insert("prompts".into(), json!(["./new-prompt.md"]));
    });
    manager.set_project_extension_paths(&strings(&["./updated-extension.ts"]));
    let saved = read(&d.project_path());
    assert_eq!(saved["prompts"], json!(["./new-prompt.md"]));
    assert_eq!(saved["extensions"], json!(["./updated-extension.ts"]));
}

#[test]
fn in_memory_project_changes_override_external_changes_for_the_same_field() {
    let d = Dirs::new();
    write(
        &d.project_path(),
        &json!({"extensions": ["./initial-extension.ts"]}),
    );
    let mut manager = d.manager();
    edit(&d.project_path(), |s| {
        s.insert("extensions".into(), json!(["./external-extension.ts"]));
    });
    manager.set_project_extension_paths(&strings(&["./in-memory-extension.ts"]));
    assert_eq!(
        read(&d.project_path())["extensions"],
        json!(["./in-memory-extension.ts"])
    );
}

// --- beyond the TS files ---

#[test]
fn reads_hoocode_settings_until_the_first_write_creates_the_cortex_file() {
    let d = Dirs::new();
    let hoocode_agent = d.root.join(".hoocode");
    let cortex_agent = d.root.join(".cortexcode");
    std::fs::create_dir_all(&hoocode_agent).unwrap();
    write(
        &hoocode_agent.join("settings.json"),
        &json!({"theme": "dark", "unknownFutureKey": {"a": 1}}),
    );
    std::fs::create_dir_all(d.project.join(".hoocode")).unwrap();
    write(
        &d.project.join(".hoocode/settings.json"),
        &json!({"defaultModel": "from-hoocode-project"}),
    );
    std::fs::remove_dir_all(d.project.join(".cortexcode")).unwrap();

    let mut manager = SettingsManager::create(&d.project, &cortex_agent);
    assert_eq!(manager.theme().as_deref(), Some("dark"));
    assert_eq!(
        manager.default_model().as_deref(),
        Some("from-hoocode-project")
    );
    assert!(!cortex_agent.exists());

    manager.set_quiet_startup(true);
    assert_eq!(
        read(&cortex_agent.join("settings.json")),
        json!({"theme": "dark", "unknownFutureKey": {"a": 1}, "quietStartup": true})
    );
    // The hoocode file is left alone.
    assert_eq!(
        read(&hoocode_agent.join("settings.json")),
        json!({"theme": "dark", "unknownFutureKey": {"a": 1}})
    );
    assert!(!hoocode_agent.join("settings.json.lock").exists());
    assert!(!d.project.join(".cortexcode").exists());
}

#[test]
fn migrates_retired_keys() {
    let migrated = migrate_settings(
        json!({
            "queueMode": "all",
            "websockets": true,
            "skills": {"enableSkillCommands": false, "customDirectories": ["~/skills"]},
            "retry": {"maxDelayMs": 5000, "maxRetries": 2}
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    assert_eq!(
        Value::Object(migrated),
        json!({
            "skills": ["~/skills"],
            "retry": {"maxRetries": 2, "provider": {"maxRetryDelayMs": 5000}},
            "steeringMode": "all",
            "transport": "websocket",
            "enableSkillCommands": false
        })
    );
    let kept = migrate_settings(
        json!({
            "queueMode": "all",
            "steeringMode": "one-at-a-time",
            "skills": {"customDirectories": []},
            "retry": {"maxDelayMs": 5000, "provider": {"maxRetryDelayMs": 1}}
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    assert_eq!(
        Value::Object(kept),
        json!({
            "queueMode": "all",
            "steeringMode": "one-at-a-time",
            "retry": {"provider": {"maxRetryDelayMs": 1}}
        })
    );
}

#[test]
fn project_overrides_merge_one_level_deep_and_defaults_fill_in() {
    let d = Dirs::new();
    write(
        &d.global_path(),
        &json!({"compaction": {"enabled": false, "reserveTokens": 1000}, "theme": "dark"}),
    );
    write(
        &d.project_path(),
        &json!({"compaction": {"reserveTokens": 2000}, "theme": "light"}),
    );
    let manager = d.manager();
    assert_eq!(
        manager.compaction_settings(),
        CompactionSettings {
            enabled: false,
            reserve_tokens: 2000,
            keep_recent_tokens: 20000,
            max_context_ratio: 0.75
        }
    );
    assert_eq!(manager.theme().as_deref(), Some("light"));
    assert_eq!(manager.steering_mode(), QueueMode::OneAtATime);
    assert_eq!(manager.transport(), cortexcode_ai_types::Transport::Auto);
    assert_eq!(manager.double_escape_action(), DoubleEscapeAction::Tree);
    assert_eq!(manager.editor_border(), EditorBorder::Box);
    assert_eq!(manager.code_block_indent(), "  ");
    assert_eq!(
        manager.warnings(),
        WarningSettings {
            anthropic_extra_usage: Some(true),
            websearch_api_key: Some(true)
        }
    );
    assert_eq!(
        manager.learn_settings(),
        LearnSettings {
            max_sessions: 20,
            max_age_days: 30,
            min_repeats: 2,
            min_request_repeats: 3,
            max_proposals: 8
        }
    );
    assert_eq!(manager.default_settings()["toolOutput"]["maxBytes"], 32768);
}

#[test]
fn in_memory_manager_and_misc_setters() {
    let mut manager = SettingsManager::in_memory(
        json!({"queueMode": "all", "maxSubagentDepth": 0, "learnMaxSessions": -3})
            .as_object()
            .unwrap()
            .clone(),
    );
    assert_eq!(manager.steering_mode(), QueueMode::All);
    assert_eq!(manager.max_subagent_depth(), 1);
    assert_eq!(manager.learn_settings().max_sessions, 20);

    manager.set_learn_setting(LearnSettingKey::MaxSessions, 0);
    assert_eq!(manager.learn_settings().max_sessions, 1);
    manager.set_editor_padding_x(9);
    assert_eq!(manager.editor_padding_x(), 3);
    manager.set_autocomplete_max_visible(1);
    assert_eq!(manager.autocomplete_max_visible(), 3);
    manager.mark_tip_seen("a");
    manager.mark_tip_seen("a");
    manager.record_star_nudge();
    manager.record_star_nudge();
    assert_eq!(manager.seen_tips(), strings(&["a"]));
    assert_eq!(manager.star_nudge_count(), 2);
    manager.set_platform(Some(&strings(&["claude"])));
    assert_eq!(manager.platform(), Some(strings(&["claude"])));
    manager.set_platform(Some(&[]));
    assert_eq!(manager.platform(), None);
    manager.set_shell_path(Some("/bin/zsh"));
    manager.set_shell_path(None);
    assert_eq!(manager.shell_path(), None);
    manager.set_default_model_and_provider("anthropic", "claude");
    assert_eq!(manager.default_provider().as_deref(), Some("anthropic"));
    manager.set_transport(cortexcode_ai_types::Transport::WebSocket);
    assert_eq!(manager.global_settings()["transport"], json!("websocket"));

    let mut overrides = serde_json::Map::new();
    overrides.insert("theme".into(), json!("override"));
    manager.apply_overrides(&overrides);
    assert_eq!(manager.theme().as_deref(), Some("override"));
    assert!(manager.drain_errors().is_empty());
}

#[test]
fn a_scope_that_failed_to_load_is_never_written() {
    let d = Dirs::new();
    std::fs::write(d.global_path(), "{ broken").unwrap();
    let mut manager = d.manager();
    manager.set_theme("light");
    assert_eq!(
        std::fs::read_to_string(d.global_path()).unwrap(),
        "{ broken"
    );
    assert_eq!(manager.theme().as_deref(), Some("light"));
}

#[test]
fn writes_the_same_bytes_as_json_stringify_with_two_spaces() {
    let d = Dirs::new();
    let mut manager = d.manager();
    manager.set_disabled_tools(&[]);
    manager.set_compaction_enabled(false);
    assert_eq!(
        std::fs::read_to_string(d.global_path()).unwrap(),
        "{\n  \"disabledTools\": [],\n  \"compaction\": {\n    \"enabled\": false\n  }\n}"
    );
}
