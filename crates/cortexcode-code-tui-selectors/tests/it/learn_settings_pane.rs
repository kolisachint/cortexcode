//! Port of `learn-settings-pane.test.ts`: the `/learn` thresholds in the
//! `/settings` pane, and `setLearnSetting` persisting them.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_settings::{LearnSettingKey, LearnSettings, SettingsManager};
use cortexcode_code_tui_selectors::settings_selector::{
    submenu_list, DoneSlot, SettingsChange, SettingsConfig, SettingsSelectorComponent,
};
use cortexcode_tui_components::SettingsList;

use crate::support::lock;

const DEFAULT_LEARN: LearnSettings = LearnSettings {
    max_sessions: 20,
    max_age_days: 30,
    min_repeats: 2,
    min_request_repeats: 3,
    max_proposals: 8,
};

fn pane(
    learn: LearnSettings,
    callback: impl FnMut(SettingsChange) + 'static,
) -> SettingsSelectorComponent {
    SettingsSelectorComponent::new(
        SettingsConfig {
            learn,
            tips_enabled: false,
            ..SettingsConfig::default()
        },
        callback,
    )
}

/// Open the Learning category: its (label, value) and the rows inside.
fn learning_rows(pane: &SettingsSelectorComponent) -> (String, String, Rc<RefCell<SettingsList>>) {
    let list = pane.settings_list();
    let list = list.borrow();
    let category = list
        .items()
        .iter()
        .find(|i| i.id == "cat-learn")
        .expect("Learning category row");
    let slot: DoneSlot = Rc::default();
    let handle = (category.submenu.as_ref().unwrap())("", slot);
    (
        category.label.clone(),
        category.current_value.clone(),
        submenu_list(&handle).unwrap(),
    )
}

#[test]
fn offers_the_thresholds_as_a_top_level_category_not_buried_under_advanced() {
    let _g = lock();
    let (label, value, rows) = learning_rows(&pane(DEFAULT_LEARN, |_| {}));
    assert_eq!(label, "Learning");
    assert_eq!(value, "5 settings");
    let ids: Vec<String> = rows.borrow().items().iter().map(|i| i.id.clone()).collect();
    assert_eq!(
        ids,
        [
            "learnMaxSessions",
            "learnMaxAgeDays",
            "learnMinRepeats",
            "learnMinRequestRepeats",
            "learnMaxProposals"
        ]
    );
}

#[test]
fn routes_a_change_to_the_settings_key_the_row_is_named_for() {
    let _g = lock();
    let changes = Rc::new(RefCell::new(Vec::new()));
    let seen = changes.clone();
    let pane = pane(DEFAULT_LEARN, move |change| {
        if let SettingsChange::LearnSetting(key, value) = change {
            seen.borrow_mut().push((key, value));
        }
    });
    let (_, _, rows) = learning_rows(&pane);
    rows.borrow_mut().emit_change("learnMaxAgeDays", "90");
    rows.borrow_mut().emit_change("learnMinRepeats", "4");
    assert_eq!(
        *changes.borrow(),
        [
            (LearnSettingKey::MaxAgeDays, 90),
            (LearnSettingKey::MinRepeats, 4)
        ]
    );
}

#[test]
fn keeps_a_hand_edited_value_in_the_cycle_rather_than_snapping_away_from_it() {
    let _g = lock();
    let (_, _, rows) = learning_rows(&pane(
        LearnSettings {
            max_age_days: 45,
            ..DEFAULT_LEARN
        },
        |_| {},
    ));
    let rows = rows.borrow();
    let row = rows
        .items()
        .iter()
        .find(|i| i.id == "learnMaxAgeDays")
        .unwrap();
    assert_eq!(row.current_value, "45");
    assert_eq!(
        row.values.as_deref().unwrap(),
        ["7", "14", "30", "45", "60", "90", "180"]
    );
}

#[test]
fn set_learn_setting_persists_to_the_user_settings_json_that_learn_re_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SettingsManager::create(dir.path(), dir.path());
    manager.set_learn_setting(LearnSettingKey::MaxAgeDays, 90);
    assert_eq!(manager.learn_settings().max_age_days, 90);

    manager.flush();
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("settings.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk["learnMaxAgeDays"], 90);
    assert_eq!(
        SettingsManager::create(dir.path(), dir.path())
            .learn_settings()
            .max_age_days,
        90
    );
}

#[test]
fn set_learn_setting_refuses_a_value_that_would_shrink_the_window_to_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = SettingsManager::create(dir.path(), dir.path());
    manager.set_learn_setting(LearnSettingKey::MaxSessions, 0);
    assert_eq!(manager.learn_settings().max_sessions, 1);
}
