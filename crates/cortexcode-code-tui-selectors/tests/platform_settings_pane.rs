//! Port of `platform-settings-pane.test.ts`: the artifact platform targets
//! (`--platform` / the `platform` setting) in the `/settings` pane, the
//! plugin system's master switch, `setPlatform`, and the pane's search.

mod support;

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use cortexcode_code_settings::{MarketplacePlatform, SettingsManager};
use cortexcode_code_tui_selectors::settings_selector::{
    submenu_list, DoneSlot, SettingsChange, SettingsConfig, SettingsSelectorComponent,
};
use cortexcode_tui_components::{SettingsList, SubmenuOutcome};

use support::lock;

fn pane(
    platform: &[MarketplacePlatform],
    callback: impl FnMut(SettingsChange) + 'static,
) -> SettingsSelectorComponent {
    SettingsSelectorComponent::new(
        SettingsConfig {
            platform: platform.to_vec(),
            tips_enabled: false,
            ..SettingsConfig::default()
        },
        callback,
    )
}

/// Open the row `id` of `list`; the list inside and the slot it closes into.
fn open(list: &Rc<RefCell<SettingsList>>, id: &str) -> (Rc<RefCell<SettingsList>>, DoneSlot) {
    let list = list.borrow();
    let item = list.items().iter().find(|i| i.id == id).expect(id);
    let slot: DoneSlot = Rc::default();
    let handle = (item.submenu.as_ref().expect("submenu"))(&item.current_value, slot.clone());
    (
        submenu_list(&handle).expect("a settings-list submenu"),
        slot,
    )
}

fn ids(list: &Rc<RefCell<SettingsList>>) -> Vec<String> {
    list.borrow().items().iter().map(|i| i.id.clone()).collect()
}

fn value(list: &Rc<RefCell<SettingsList>>, id: &str) -> String {
    list.borrow()
        .items()
        .iter()
        .find(|i| i.id == id)
        .unwrap()
        .current_value
        .clone()
}

/// Every id reachable by opening rows from the top level down. Select-list
/// submenus (theme, thinking) hold no settings list; their row is counted.
fn reachable_ids(pane: &SettingsSelectorComponent) -> HashSet<String> {
    fn walk(list: &Rc<RefCell<SettingsList>>, ids: &mut HashSet<String>) {
        let nested: Vec<Rc<RefCell<SettingsList>>> = {
            let list = list.borrow();
            list.items()
                .iter()
                .filter_map(|item| {
                    ids.insert(item.id.clone());
                    let factory = item.submenu.as_ref()?;
                    submenu_list(&factory(&item.current_value, Rc::default()))
                })
                .collect()
        };
        for list in &nested {
            walk(list, ids);
        }
    }
    let mut ids = HashSet::new();
    walk(&pane.settings_list(), &mut ids);
    ids
}

fn platforms(changes: &Rc<RefCell<Vec<Vec<MarketplacePlatform>>>>) -> impl FnMut(SettingsChange) {
    let changes = changes.clone();
    move |change| {
        if let SettingsChange::Platform(p) = change {
            changes.borrow_mut().push(p);
        }
    }
}

use MarketplacePlatform::{Agents, Claude, Github};

#[test]
fn gives_the_platform_and_plugin_scope_rows_a_category_to_be_reached_from() {
    let _g = lock();
    let pane = pane(&[], |_| {});
    let list = pane.settings_list();
    let label = list
        .borrow()
        .items()
        .iter()
        .find(|i| i.id == "cat-plugins")
        .map(|i| i.label.clone());
    assert_eq!(label.as_deref(), Some("Plugins"));
    let (plugins, _) = open(&list, "cat-plugins");
    assert_eq!(
        ids(&plugins),
        ["plugin-tools", "platform", "plugin-install-scope"]
    );
}

#[test]
fn leaves_no_setting_stranded_outside_every_category() {
    let _g = lock();
    let ids = reachable_ids(&pane(&[], |_| {}));
    for id in [
        "autocompact",
        "context-gc",
        "light",
        "tools",
        "tool-output",
        "tool-output-view",
        "output-max-bytes",
        "output-max-lines",
        "platform",
        "plugin-tools",
        "plugin-install-scope",
        "steering-mode",
        "follow-up-mode",
        "thinking",
        "double-escape-action",
        "tree-filter-mode",
        "transport",
        "theme",
        "hide-thinking",
        "show-hardware-cursor",
        "editor-border",
        "editor-padding",
        "autocomplete-max-visible",
        "clear-on-shrink",
        "terminal-progress",
        "auto-resize-images",
        "block-images",
        "quiet-startup",
        "collapse-changelog",
        "install-telemetry",
        "skill-commands",
        "warnings",
        "voice-silence-ms",
        "webtools-timeout-secs",
    ] {
        assert!(ids.contains(id), "\"{id}\" reachable from the pane");
    }
}

#[test]
fn shows_the_fallback_when_nothing_is_configured_and_the_targets_when_they_are() {
    let _g = lock();
    let (unset, _) = open(&pane(&[], |_| {}).settings_list(), "cat-plugins");
    assert_eq!(value(&unset, "platform"), "default (claude)");

    let (set, _) = open(
        &pane(&[Github, Agents], |_| {}).settings_list(),
        "cat-plugins",
    );
    assert_eq!(value(&set, "platform"), "github, agents");
}

#[test]
fn toggles_each_target_independently_since_the_setting_is_a_list() {
    let _g = lock();
    let changes = Rc::default();
    let pane = pane(&[Claude], platforms(&changes));
    let (plugins, _) = open(&pane.settings_list(), "cat-plugins");
    let (platform_list, _) = open(&plugins, "platform");

    let rows: Vec<(String, String)> = platform_list
        .borrow()
        .items()
        .iter()
        .map(|i| (i.id.clone(), i.current_value.clone()))
        .collect();
    assert_eq!(
        rows,
        [
            ("claude".into(), "on".into()),
            ("github".into(), "off".into()),
            ("agents".into(), "off".into())
        ]
    );

    platform_list.borrow_mut().emit_change("github", "on");
    platform_list.borrow_mut().emit_change("claude", "off");

    // Pane order, not toggle order, so the persisted list is stable.
    assert_eq!(*changes.borrow(), [vec![Claude, Github], vec![Github]]);
}

#[test]
fn reports_an_empty_selection_as_unset_rather_than_as_targeting_nothing() {
    let _g = lock();
    let changes = Rc::default();
    let pane = pane(&[Claude], platforms(&changes));
    let (plugins, _) = open(&pane.settings_list(), "cat-plugins");
    let (platform_list, slot) = open(&plugins, "platform");

    platform_list.borrow_mut().emit_change("claude", "off");
    platform_list.borrow_mut().emit_cancel();

    assert_eq!(*changes.borrow(), [Vec::<MarketplacePlatform>::new()]);
    let summary = match slot.borrow_mut().take() {
        Some(SubmenuOutcome::Selected(summary)) => summary,
        _ => panic!("the platform submenu closes with its summary"),
    };
    assert_eq!(summary, "default (claude)");
}

#[test]
fn routes_the_master_switch_to_the_setting_that_gates_the_tools_and_the_nudge() {
    let _g = lock();
    let changes = Rc::new(RefCell::new(Vec::new()));
    let seen = changes.clone();
    let pane = pane(&[], move |change| {
        if let SettingsChange::EnablePluginTools(on) = change {
            seen.borrow_mut().push(on);
        }
    });
    let (plugins, _) = open(&pane.settings_list(), "cat-plugins");
    assert_eq!(value(&plugins, "plugin-tools"), "false");
    plugins.borrow_mut().emit_change("plugin-tools", "true");
    assert_eq!(*changes.borrow(), [true]);
}

fn read_settings(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

#[test]
fn set_platform_persists_the_targets_to_the_user_settings_json_the_next_session_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SettingsManager::create(dir.path(), dir.path());
    manager.set_platform(Some(&strings(&["github", "claude"])));
    assert_eq!(manager.platform(), Some(strings(&["github", "claude"])));

    manager.flush();
    assert_eq!(
        read_settings(dir.path())["platform"],
        serde_json::json!(["github", "claude"])
    );
    assert_eq!(
        SettingsManager::create(dir.path(), dir.path()).platform(),
        Some(strings(&["github", "claude"]))
    );
}

#[test]
fn set_platform_removes_the_key_on_an_empty_selection() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SettingsManager::create(dir.path(), dir.path());
    manager.set_platform(Some(&strings(&["github"])));
    manager.set_platform(Some(&[]));

    assert_eq!(manager.platform(), None);
    manager.flush();
    assert!(read_settings(dir.path()).get("platform").is_none());
}

#[test]
fn finds_a_setting_through_the_category_that_holds_it() {
    let _g = lock();
    let pane = pane(&[], |_| {});
    let list = pane.settings_list();
    let filtered = |query: &str| -> Vec<String> {
        list.borrow_mut().apply_filter(query);
        list.borrow()
            .filtered_items()
            .iter()
            .map(|i| i.id.clone())
            .collect()
    };
    assert!(filtered("theme").contains(&"cat-interface".to_string()));
    assert!(filtered("platform").contains(&"cat-plugins".to_string()));
}
