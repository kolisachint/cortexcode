//! Port of `settings-token-surface.test.ts`: what `/settings` says a change
//! costs, and the grouping that keeps the token-budget settings together.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tools::light::{
    create_light_tools, measure_prompt_surface, measure_tool_schema_tokens, PromptSurface,
};
use cortexcode_code_tui_selectors::settings_selector::{
    submenu_list, DoneSlot, SettingsChange, SettingsConfig, SettingsSelectorComponent,
    ToolToggleInfo,
};
use cortexcode_tui_components::SettingsList;
use cortexcode_tui_render::Component;

use crate::support::{lock, strip};

fn surface() -> PromptSurface {
    PromptSurface {
        system_prompt_tokens: 1430,
        tool_schema_tokens: 2710,
        total_tokens: 4140,
        tools: vec![("read".into(), 162), ("bash".into(), 128)],
    }
}

fn pane_config() -> SettingsConfig {
    SettingsConfig {
        tools: vec![
            ToolToggleInfo {
                name: "read".into(),
                enabled: true,
                tokens: Some(162),
            },
            ToolToggleInfo {
                name: "ProposePlugin".into(),
                enabled: false,
                tokens: Some(849),
            },
            ToolToggleInfo {
                name: "websearch".into(),
                enabled: true,
                tokens: None,
            },
        ],
        tips_enabled: false,
        ..SettingsConfig::default()
    }
}

fn rendered(pane: &mut SettingsSelectorComponent) -> String {
    strip(&pane.render(100))
}

/// Build the submenu a row opens and hand back the list inside it.
fn open(list: &Rc<RefCell<SettingsList>>, id: &str) -> Rc<RefCell<SettingsList>> {
    let list = list.borrow();
    let item = list.items().iter().find(|i| i.id == id).expect(id);
    let slot: DoneSlot = Rc::default();
    let handle = (item.submenu.as_ref().expect("submenu"))(&item.current_value, slot);
    submenu_list(&handle).expect("a settings-list submenu")
}

fn ignore(_: SettingsChange) {}

#[test]
fn prices_the_whole_surface_under_the_list_exactly_rather_than_rounded() {
    let _g = lock();
    let mut pane = SettingsSelectorComponent::new(
        SettingsConfig {
            measure_token_surface: Some(Rc::new(surface)),
            ..pane_config()
        },
        ignore,
    );
    assert!(rendered(&mut pane).contains(
        "Per-turn surface: 4,140 tokens  (1,430 system prompt + 2,710 schemas, 2 tools)"
    ));
}

#[test]
fn re_prices_after_a_change_since_a_tool_toggle_moves_the_surface_at_once() {
    let _g = lock();
    let current = Rc::new(RefCell::new(surface()));
    let measured = current.clone();
    let mut pane = SettingsSelectorComponent::new(
        SettingsConfig {
            measure_token_surface: Some(Rc::new(move || measured.borrow().clone())),
            ..pane_config()
        },
        ignore,
    );

    *current.borrow_mut() = PromptSurface {
        tool_schema_tokens: 1861,
        total_tokens: 3291,
        tools: vec![("read".into(), 162)],
        ..surface()
    };
    pane.settings_list()
        .borrow_mut()
        .emit_change("autocompact", "false");

    assert!(rendered(&mut pane).contains("Per-turn surface: 3,291 tokens"));
}

#[test]
fn says_nothing_at_all_when_there_is_no_session_to_measure() {
    let _g = lock();
    let mut pane = SettingsSelectorComponent::new(pane_config(), ignore);
    assert!(!rendered(&mut pane).contains("Per-turn surface"));
}

#[test]
fn prices_each_tool_beside_its_switch_including_the_ones_that_are_off() {
    let _g = lock();
    let pane = SettingsSelectorComponent::new(
        SettingsConfig {
            measure_token_surface: Some(Rc::new(surface)),
            ..pane_config()
        },
        ignore,
    );
    let list = pane.settings_list();
    let suffix = list
        .borrow()
        .items()
        .iter()
        .find(|i| i.id == "tools")
        .unwrap()
        .value_suffix
        .clone();
    assert_eq!(suffix.as_deref(), Some("2,710 tok/turn"));

    let tools = open(&list, "tools");
    let tools = tools.borrow();
    let suffix = |id: &str| {
        tools
            .items()
            .iter()
            .find(|i| i.id == id)
            .unwrap()
            .value_suffix
            .clone()
    };
    // What a disabled tool costs is what turning it back on will cost.
    assert_eq!(suffix("ProposePlugin").as_deref(), Some("849 tok/turn"));
    assert_eq!(suffix("read").as_deref(), Some("162 tok/turn"));
    // A tool disabled before launch has no schema this session, so no price.
    assert_eq!(suffix("websearch"), None);
}

#[test]
fn prices_a_tool_the_way_the_session_does() {
    // Same helper, same number as --print-token-surface reports.
    let tools = create_light_tools(std::env::temp_dir());
    let tool = &tools[0];
    assert_eq!(
        measure_tool_schema_tokens(&tool.name, &tool.description, &tool.parameters),
        measure_prompt_surface("sys", &tools[..1]).tools[0].1
    );
}

#[test]
fn keeps_the_token_budget_settings_in_one_place() {
    let _g = lock();
    let pane = SettingsSelectorComponent::new(pane_config(), ignore);
    let list = pane.settings_list();
    let label = list
        .borrow()
        .items()
        .iter()
        .find(|i| i.id == "cat-context")
        .unwrap()
        .label
        .clone();
    assert_eq!(label, "Context");
    let context = open(&list, "cat-context");
    let ids: Vec<String> = context
        .borrow()
        .items()
        .iter()
        .map(|i| i.id.clone())
        .collect();
    assert_eq!(ids, ["autocompact", "context-gc", "light"]);
}

#[test]
fn opens_something_from_every_top_level_row() {
    let _g = lock();
    let pane = SettingsSelectorComponent::new(pane_config(), ignore);
    for item in pane.settings_list().borrow().items() {
        assert!(
            item.submenu.is_some(),
            "top-level row \"{}\" opens a submenu",
            item.id
        );
    }
}

#[test]
fn routes_the_light_preset_to_its_setting() {
    let _g = lock();
    let changes = Rc::new(RefCell::new(Vec::new()));
    let seen = changes.clone();
    let pane = SettingsSelectorComponent::new(pane_config(), move |change| {
        if let SettingsChange::Light(on) = change {
            seen.borrow_mut().push(on);
        }
    });
    let context = open(&pane.settings_list(), "cat-context");
    context.borrow_mut().emit_change("light", "true");
    assert_eq!(*changes.borrow(), [true]);
}
