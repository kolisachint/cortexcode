//! Port of the pin's `test/theme-export.test.ts`, plus the loader and global
//! theme behavior (custom themes, fallbacks, retired names, registered
//! themes, the custom-theme watcher).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cortexcode_code_tui_theme::*;
use serde_json::{json, Value};

/// A fresh agent dir with an empty `themes/`, pointed at by the env
/// override. Tests here share the process env, so they run one at a time.
struct AgentDir {
    _guard: MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
}

impl AgentDir {
    fn new() -> Self {
        static LOCK: Mutex<()> = Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("agent/themes")).unwrap();
        std::env::set_var("CORTEXCODE_CODING_AGENT_DIR", dir.path().join("agent"));
        set_registered_themes(Vec::new());
        Self { _guard: guard, dir }
    }

    fn theme_file(&self, name: &str) -> PathBuf {
        self.dir
            .path()
            .join("agent/themes")
            .join(format!("{name}.json"))
    }

    fn write(&self, name: &str, theme: &Value) {
        std::fs::write(
            self.theme_file(name),
            serde_json::to_string_pretty(theme).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for AgentDir {
    fn drop(&mut self) {
        stop_theme_watcher();
        set_registered_themes(Vec::new());
    }
}

fn dark_theme() -> Value {
    serde_json::from_str(builtin_theme_source("dark").unwrap()).unwrap()
}

fn with_vars(mut theme: Value, name: &str, vars: Value, export: Value) -> Value {
    theme["name"] = json!(name);
    let merged = theme["vars"].as_object_mut().unwrap();
    for (k, v) in vars.as_object().unwrap() {
        merged.insert(k.clone(), v.clone());
    }
    theme["export"] = export;
    theme
}

#[test]
fn resolves_export_variable_references_using_the_same_syntax_as_colors() {
    let agent = AgentDir::new();
    let theme = with_vars(
        dark_theme(),
        "custom-export-vars",
        json!({"pageBgVar": "#112233", "pageBgAlias": "pageBgVar", "infoBgVar": "#445566", "cardBgVar": "#223344"}),
        json!({"pageBg": "pageBgAlias", "cardBg": "cardBgVar", "infoBg": "infoBgVar"}),
    );
    agent.write("custom-export-vars", &theme);
    assert_eq!(
        get_theme_export_colors(Some("custom-export-vars")),
        ThemeExportColors {
            page_bg: Some("#112233".into()),
            card_bg: Some("#223344".into()),
            info_bg: Some("#445566".into()),
        }
    );
}

#[test]
fn resolves_recursive_vars_and_converts_256_color_export_values_to_hex() {
    let agent = AgentDir::new();
    let theme = with_vars(
        dark_theme(),
        "custom-export-recursive",
        json!({"deepPageBg": "#abcdef", "pageBgAlias": "deepPageBg", "cardBgAnsi": 24}),
        json!({"pageBg": "pageBgAlias", "cardBg": "cardBgAnsi", "infoBg": ""}),
    );
    agent.write("custom-export-recursive", &theme);
    assert_eq!(
        get_theme_export_colors(Some("custom-export-recursive")),
        ThemeExportColors {
            page_bg: Some("#abcdef".into()),
            card_bg: Some("#005f87".into()),
            info_bg: None,
        }
    );
}

#[test]
fn lists_custom_themes_with_their_paths_and_descriptions() {
    let agent = AgentDir::new();
    let mut theme = dark_theme();
    theme["name"] = json!("mine");
    theme["description"] = json!("My own");
    agent.write("mine", &theme);
    let available = get_available_themes();
    assert!(available.contains(&"mine".to_string()));
    assert!(available.windows(2).all(|w| w[0] <= w[1]));
    let infos = get_available_themes_with_paths();
    let mine = infos.iter().find(|i| i.name == "mine").unwrap();
    assert_eq!(
        mine.path.as_deref(),
        Some(agent.theme_file("mine").as_path())
    );
    assert!(infos
        .iter()
        .find(|i| i.name == "dark")
        .unwrap()
        .path
        .is_none());
    assert_eq!(get_theme_description("mine").as_deref(), Some("My own"));
    assert_eq!(get_theme_description("nope"), None);
}

#[test]
fn a_custom_file_named_like_a_retired_theme_wins_over_the_successor() {
    let agent = AgentDir::new();
    let mut theme = dark_theme();
    theme["name"] = json!("warm-light");
    theme["colors"]["accent"] = json!("#123456");
    agent.write("warm-light", &theme);
    assert_eq!(resolve_theme_name("warm-light"), "warm-light");
    assert_eq!(
        get_resolved_theme_colors(Some("warm-light")).unwrap()["accent"],
        "#123456"
    );
}

#[test]
fn init_and_set_fall_back_to_dark_on_an_invalid_theme() {
    let agent = AgentDir::new();
    let mut broken = dark_theme();
    broken["colors"].as_object_mut().unwrap().remove("accent");
    agent.write("broken", &broken);

    init_theme(Some("broken"), false);
    assert_eq!(current_theme_name().as_deref(), Some("dark"));
    assert_eq!(theme().name.as_deref(), Some("dark"));

    init_theme(Some("light"), false);
    let error = set_theme("broken", false).unwrap_err();
    assert!(error.starts_with("Invalid theme \"broken\":"), "{error}");
    assert_eq!(current_theme_name().as_deref(), Some("dark"));

    let error = set_theme("does-not-exist", false).unwrap_err();
    assert_eq!(error, "Theme not found: does-not-exist");
}

#[test]
fn init_records_a_retired_name_as_its_successor() {
    let _agent = AgentDir::new();
    init_theme(Some("vox-light"), false);
    assert_eq!(current_theme_name().as_deref(), Some("vox-cutout-light"));
    assert_eq!(theme().name.as_deref(), Some("vox-cutout-light"));
}

#[test]
fn set_theme_runs_the_change_callback_and_instances_are_in_memory() {
    let _agent = AgentDir::new();
    let calls = Arc::new(Mutex::new(0));
    let sink = calls.clone();
    on_theme_change(move || *sink.lock().unwrap() += 1);
    init_theme(Some("dark"), false);
    set_theme("light", false).unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);

    let instance = load_theme_json("solarized-dark").unwrap();
    let t = create_theme(&instance, Some(ColorMode::Truecolor), None).unwrap();
    set_theme_instance(t);
    assert_eq!(current_theme_name().as_deref(), Some("<in-memory>"));
    assert_eq!(theme().name.as_deref(), Some("solarized-dark"));
    assert_eq!(*calls.lock().unwrap(), 2);
    on_theme_change(|| {});
}

#[test]
fn registered_themes_join_the_roster_and_load_by_name() {
    let agent = AgentDir::new();
    let mut json_theme = dark_theme();
    json_theme["name"] = json!("ext-theme");
    let path = agent.dir.path().join("ext.json");
    std::fs::write(&path, json_theme.to_string()).unwrap();
    let with_path = load_theme_from_path(&path, Some(ColorMode::Truecolor)).unwrap();
    let mut no_path = with_path.clone();
    no_path.name = Some("ext-memory".into());
    no_path.source_path = None;
    set_registered_themes(vec![with_path, no_path]);

    assert!(get_available_themes().contains(&"ext-theme".to_string()));
    assert!(get_theme_by_name("ext-theme").is_some());
    assert_eq!(load_theme_json("ext-theme").unwrap().name, "ext-theme");
    assert_eq!(
        load_theme_json("ext-memory").unwrap_err(),
        "Theme \"ext-memory\" does not have a source path for export"
    );
    let infos = get_available_themes_with_paths();
    assert_eq!(
        infos
            .iter()
            .find(|i| i.name == "ext-theme")
            .unwrap()
            .path
            .as_deref(),
        Some(path.as_path())
    );
}

#[test]
fn the_watcher_reloads_an_edited_custom_theme() {
    let agent = AgentDir::new();
    let mut custom = dark_theme();
    custom["name"] = json!("watched");
    agent.write("watched", &custom);
    let changed = Arc::new(Mutex::new(0));
    let sink = changed.clone();
    on_theme_change(move || *sink.lock().unwrap() += 1);
    init_theme(Some("watched"), true);
    let before = theme().get_fg_ansi("accent").to_string();

    // Let the watcher take its first look, then edit (a size change, so the
    // edit is visible even where mtimes are coarse).
    std::thread::sleep(Duration::from_millis(120));
    custom["colors"]["accent"] = json!("#010203");
    custom["description"] = json!("edited so the file length changes");
    agent.write("watched", &custom);

    let deadline = Instant::now() + Duration::from_secs(5);
    while theme().get_fg_ansi("accent") == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let expected = color::fg_ansi(&RawColor::hex("#010203"), detect_color_mode()).unwrap();
    assert_eq!(theme().get_fg_ansi("accent"), expected);
    assert!(*changed.lock().unwrap() >= 1);
    on_theme_change(|| {});
}
