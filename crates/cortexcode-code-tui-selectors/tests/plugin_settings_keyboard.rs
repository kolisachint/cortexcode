//! Port of `plugin-settings-keyboard.test.ts`: the Plugins pane driven by
//! real keystrokes, through a real settings manager, then the next session's
//! startup wiring over the file it wrote.

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_settings::platform_targets::{
    get_workspace_platforms, parse_platforms, resolve_plugin_platforms, set_platforms,
};
use cortexcode_code_settings::{MarketplacePlatform, SettingsManager};
use cortexcode_code_tui_selectors::settings_selector::{
    submenu_list, DoneSlot, SettingsChange, SettingsConfig, SettingsSelectorComponent,
};
use cortexcode_tui_render::Component;

use support::lock;

const DOWN: &str = "\x1b[B";
const ENTER: &str = "\r";
const ESC: &str = "\x1b";

use MarketplacePlatform::{Claude, Github};

/// The pane as the interactive mode builds it, over a real settings manager.
fn open_pane(manager: &Rc<RefCell<SettingsManager>>) -> SettingsSelectorComponent {
    let config = {
        let m = manager.borrow();
        SettingsConfig {
            plugin_install_scope: m.plugin_install_scope(),
            enable_plugin_tools: m.enable_plugin_tools(),
            project_pinned_settings: m.project_settings().keys().cloned().collect(),
            platform: get_workspace_platforms().unwrap_or_default(),
            tips_enabled: false,
            ..SettingsConfig::default()
        }
    };
    let manager = manager.clone();
    SettingsSelectorComponent::new(config, move |change| match change {
        SettingsChange::EnablePluginTools(on) => manager.borrow_mut().set_enable_plugin_tools(on),
        SettingsChange::PluginInstallScope(scope) => {
            manager.borrow_mut().set_plugin_install_scope(scope)
        }
        SettingsChange::Platform(platforms) => {
            let names: Vec<String> = platforms.iter().map(|p| p.as_str().to_string()).collect();
            manager.borrow_mut().set_platform(Some(&names));
            set_platforms(Some(&platforms));
        }
        _ => {}
    })
}

/// Walk to a top-level row with arrow keys, then open it. The keyboard goes
/// to `settings_list()`, as the TUI focuses it.
fn open_category(pane: &mut SettingsSelectorComponent, id: &str) {
    let index = pane
        .settings_list()
        .borrow()
        .items()
        .iter()
        .position(|i| i.id == id)
        .unwrap_or_else(|| panic!("top-level row {id}"));
    for _ in 0..index {
        pane.handle_input(DOWN);
    }
    pane.handle_input(ENTER);
}

fn on_disk(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap()
}

/// The platform targets are process-wide; each test starts and ends unset.
struct ResetPlatforms;
impl Drop for ResetPlatforms {
    fn drop(&mut self) {
        set_platforms(None);
    }
}

fn setup() -> (
    std::sync::MutexGuard<'static, ()>,
    ResetPlatforms,
    tempfile::TempDir,
) {
    let guard = lock();
    set_platforms(None);
    (guard, ResetPlatforms, tempfile::tempdir().unwrap())
}

#[test]
fn writes_the_settings_a_later_session_reads() {
    let (_g, _reset, dir) = setup();
    let manager = Rc::new(RefCell::new(SettingsManager::create(
        dir.path(),
        dir.path(),
    )));
    let mut pane = open_pane(&manager);

    open_category(&mut pane, "cat-plugins");
    // Row 1: the master switch, off -> on.
    pane.handle_input(ENTER);
    // Row 2: the platform targets; toggle claude, then github.
    pane.handle_input(DOWN);
    pane.handle_input(ENTER);
    pane.handle_input(ENTER);
    pane.handle_input(DOWN);
    pane.handle_input(ENTER);
    pane.handle_input(ESC);

    manager.borrow().flush();
    let settings = on_disk(dir.path());
    assert_eq!(settings["enablePluginTools"], true);
    assert_eq!(
        settings["platform"],
        serde_json::json!(["claude", "github"])
    );
}

#[test]
fn hands_the_next_session_targets_its_startup_wiring_can_use() {
    let (_g, _reset, dir) = setup();
    let manager = Rc::new(RefCell::new(SettingsManager::create(
        dir.path(),
        dir.path(),
    )));
    let mut pane = open_pane(&manager);

    open_category(&mut pane, "cat-plugins");
    pane.handle_input(DOWN);
    pane.handle_input(ENTER);
    pane.handle_input(DOWN); // github
    pane.handle_input(ENTER);
    pane.handle_input(ESC);
    manager.borrow().flush();

    // Applied to the running session already, without a restart.
    assert_eq!(get_workspace_platforms(), Some(vec![Github]));
    assert_eq!(resolve_plugin_platforms(None).unwrap(), [Github]);

    // What the next session does with the file: read the setting, parse the
    // tokens, install them as the session targets.
    set_platforms(None);
    let next_session = SettingsManager::create(dir.path(), dir.path());
    let parsed = parse_platforms(&next_session.platform().unwrap_or_default());
    assert!(parsed.invalid.is_empty());
    set_platforms(Some(&parsed.platforms));

    assert_eq!(get_workspace_platforms(), Some(vec![Github]));
    assert_eq!(resolve_plugin_platforms(None).unwrap(), [Github]);
    assert!(!next_session.enable_plugin_tools());
}

#[test]
fn says_so_when_the_project_file_pins_the_key_the_row_writes() {
    let (_g, _reset, dir) = setup();
    // The row writes the user file; the project file is merged over it.
    let project = dir.path().join(cortexcode_code_paths::CONFIG_DIR_NAME);
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("settings.json"),
        r#"{"platform":["claude"],"enablePluginTools":true}"#,
    )
    .unwrap();
    let manager = Rc::new(RefCell::new(SettingsManager::create(
        dir.path(),
        dir.path(),
    )));
    let pane = open_pane(&manager);

    let list = pane.settings_list();
    let list = list.borrow();
    let category = list.items().iter().find(|i| i.id == "cat-plugins").unwrap();
    let slot: DoneSlot = Rc::default();
    let rows = submenu_list(&(category.submenu.as_ref().unwrap())("", slot)).unwrap();
    let rows = rows.borrow();
    let description = |id: &str| {
        rows.items()
            .iter()
            .find(|r| r.id == id)
            .unwrap()
            .description
            .clone()
            .unwrap_or_default()
    };
    assert!(description("platform").contains("overrides this row"));
    assert!(description("plugin-tools").contains("overrides this row"));
    assert!(!description("plugin-install-scope").contains("overrides this row"));
}

#[test]
fn clears_the_key_when_every_target_is_switched_back_off() {
    let (_g, _reset, dir) = setup();
    let manager = Rc::new(RefCell::new(SettingsManager::create(
        dir.path(),
        dir.path(),
    )));
    manager
        .borrow_mut()
        .set_platform(Some(&["claude".to_string()]));
    set_platforms(Some(&[Claude]));

    let mut pane = open_pane(&manager);
    open_category(&mut pane, "cat-plugins");
    pane.handle_input(DOWN);
    pane.handle_input(ENTER);
    pane.handle_input(ENTER); // claude: on -> off
    pane.handle_input(ESC);
    manager.borrow().flush();

    assert!(on_disk(dir.path()).get("platform").is_none());
    assert_eq!(
        SettingsManager::create(dir.path(), dir.path()).platform(),
        None
    );
    // Unset is not "target nothing": the plugin default comes back.
    assert_eq!(resolve_plugin_platforms(None).unwrap(), [Claude]);
}
