//! Port of `external-tools-pane.test.ts`: the external binaries as the
//! `/settings` pane shows them.

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tools::external_tools::{
    build_row_gates, describe_external_tools, status_label, ExternalToolStatus, ManagedToolSource,
    EXTERNAL_TOOLS,
};
use cortexcode_code_tui_selectors::settings_selector::{
    submenu_list, DoneSlot, SettingsConfig, SettingsSelectorComponent, ToolGroupInfo,
};
use cortexcode_tui_components::{SettingItem, SettingsList};

use support::lock;

/// A status with the binary absent, so the gated-row paths run.
fn missing(tool: &str) -> ExternalToolStatus {
    let doc = EXTERNAL_TOOLS
        .iter()
        .find(|d| d.tool == tool)
        .unwrap_or_else(|| panic!("no catalog entry for {tool}"))
        .clone();
    ExternalToolStatus {
        name: tool.to_string(),
        repo: if tool == "rg" {
            "BurntSushi/ripgrep".into()
        } else {
            "kolisachint/x".into()
        },
        override_env: format!("HOOCODE_{}_BINARY", tool.to_uppercase()),
        path: None,
        source: None,
        installed: false,
        downloadable: true,
        doc,
    }
}

fn present(tool: &str) -> ExternalToolStatus {
    ExternalToolStatus {
        path: Some(format!("/bin/{tool}")),
        source: Some(ManagedToolSource::Path),
        installed: true,
        ..missing(tool)
    }
}

fn build_pane(external_tools: Vec<ExternalToolStatus>) -> SettingsSelectorComponent {
    let group = |id: &str, label: &str, description: &str, enabled| ToolGroupInfo {
        id: id.into(),
        label: label.into(),
        description: description.into(),
        enabled,
    };
    SettingsSelectorComponent::new(
        SettingsConfig {
            tool_groups: vec![
                group("web", "Web tools", "webfetch + websearch.", false),
                group("embsearch", "Semantic search", "Semantic index.", true),
            ],
            external_tools,
            tips_enabled: false,
            ..SettingsConfig::default()
        },
        |_| {},
    )
}

/// A row as plain data (the item itself holds a factory).
#[derive(Debug)]
struct Row {
    id: String,
    current_value: String,
    description: String,
    value_suffix: Option<String>,
    keywords: Option<String>,
    values: Option<Vec<String>>,
    has_submenu: bool,
}

fn row(item: &SettingItem) -> Row {
    Row {
        id: item.id.clone(),
        current_value: item.current_value.clone(),
        description: item.description.clone().unwrap_or_default(),
        value_suffix: item.value_suffix.clone(),
        keywords: item.keywords.clone(),
        values: item.values.clone(),
        has_submenu: item.submenu.is_some(),
    }
}

fn rows(list: &Rc<RefCell<SettingsList>>) -> Vec<Row> {
    list.borrow().items().iter().map(row).collect()
}

/// Open the submenu of row `id` in `list`.
fn open(list: &Rc<RefCell<SettingsList>>, id: &str) -> Rc<RefCell<SettingsList>> {
    let list = list.borrow();
    let item = list.items().iter().find(|i| i.id == id).expect(id);
    let slot: DoneSlot = Rc::default();
    submenu_list(&(item.submenu.as_ref().unwrap())("", slot)).expect("a settings-list submenu")
}

/// A leaf row, found at the top level or one category down.
fn leaf(pane: &SettingsSelectorComponent, id: &str) -> Row {
    let top = pane.settings_list();
    let top_rows = rows(&top);
    for r in &top_rows {
        if r.id == id {
            return rows(&top).into_iter().find(|r| r.id == id).unwrap();
        }
        if !r.has_submenu {
            continue;
        }
        if let Some(found) = rows(&open(&top, &r.id)).into_iter().find(|c| c.id == id) {
            return found;
        }
    }
    panic!("no row {id} in the pane");
}

#[test]
fn documents_every_binary_the_tools_manager_can_resolve() {
    let mut tools: Vec<&str> = EXTERNAL_TOOLS.iter().map(|d| d.tool).collect();
    tools.sort_unstable();
    assert_eq!(tools, ["embsearch", "fd", "rg", "voicetools", "webtools"]);
}

#[test]
fn gives_every_binary_a_fallback_because_none_of_them_is_required() {
    for doc in EXTERNAL_TOOLS {
        assert!(!doc.fallback.is_empty(), "{}", doc.tool);
        assert!(!doc.enables.is_empty(), "{}", doc.tool);
    }
}

#[test]
fn resolves_live_status_without_downloading_anything() {
    let statuses = describe_external_tools();
    assert_eq!(statuses.len(), EXTERNAL_TOOLS.len());
    for status in &statuses {
        assert_eq!(status.installed, status.path.is_some());
        assert!(!status_label(status).is_empty());
    }
}

#[test]
fn distinguishes_where_an_installed_binary_came_from() {
    assert_eq!(status_label(&present("rg")), "system");
    assert_eq!(
        status_label(&ExternalToolStatus {
            source: Some(ManagedToolSource::Managed),
            ..present("rg")
        }),
        "installed"
    );
    assert_eq!(
        status_label(&ExternalToolStatus {
            source: Some(ManagedToolSource::Override),
            ..present("rg")
        }),
        "env override"
    );
    assert_eq!(status_label(&missing("rg")), "not installed");
    assert_eq!(
        status_label(&ExternalToolStatus {
            downloadable: false,
            ..missing("rg")
        }),
        "unavailable"
    );
}

#[test]
fn maps_gated_rows_back_to_the_binary_they_need() {
    let gates = build_row_gates(&[
        missing("webtools"),
        missing("voicetools"),
        missing("embsearch"),
    ]);
    assert_eq!(gates["group:web"].doc.tool, "webtools");
    assert_eq!(gates["webtools-timeout-secs"].doc.tool, "webtools");
    assert_eq!(gates["voice-silence-ms"].doc.tool, "voicetools");
    assert_eq!(gates["group:embsearch"].doc.tool, "embsearch");
    // rg/fd gate nothing: their absence changes speed, not what a setting does.
    assert!(!gates.contains_key("tools"));
}

#[test]
fn puts_the_category_at_the_top_level_with_a_count() {
    let _g = lock();
    let pane = build_pane(vec![present("rg"), missing("webtools")]);
    let row = rows(&pane.settings_list())
        .into_iter()
        .find(|r| r.id == "cat-external")
        .expect("cat-external");
    assert_eq!(row.current_value, "1 of 2 installed");
    // Searching for a binary by name must land on the category.
    assert!(row.keywords.unwrap_or_default().contains("webtools"));
}

#[test]
fn lists_one_row_per_binary_each_opening_a_detail_submenu() {
    let _g = lock();
    let statuses = EXTERNAL_TOOLS.iter().map(|d| missing(d.tool)).collect();
    let pane = build_pane(statuses);
    let rows = rows(&open(&pane.settings_list(), "cat-external"));
    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let expected: Vec<String> = EXTERNAL_TOOLS
        .iter()
        .map(|d| format!("ext-{}", d.tool))
        .collect();
    assert_eq!(ids, expected);
    // Status rows are readouts of the machine, not settings: nothing to cycle.
    for r in &rows {
        assert!(r.values.is_none(), "{}", r.id);
    }
    assert!(rows[0].has_submenu);
}

#[test]
fn marks_a_gated_row_when_its_binary_is_missing_and_leaves_it_settable() {
    let _g = lock();
    let voice = leaf(&build_pane(vec![missing("voicetools")]), "voice-silence-ms");
    assert_eq!(voice.value_suffix.as_deref(), Some("needs voicetools"));
    assert!(voice.description.contains("voicetools"));
    // The setting still cycles: the feature reads it once the binary lands.
    assert!(voice.values.unwrap().len() > 1);
}

#[test]
fn leaves_a_gated_row_unmarked_when_its_binary_is_present() {
    let _g = lock();
    let web = leaf(
        &build_pane(vec![present("webtools")]),
        "webtools-timeout-secs",
    );
    assert_eq!(web.value_suffix, None);
    assert!(!web.description.contains("Needs the"));
}

#[test]
fn marks_the_tool_group_switches_whose_tools_cannot_run_yet() {
    let _g = lock();
    let pane = build_pane(vec![missing("webtools"), present("embsearch")]);
    let rows = rows(&open(&pane.settings_list(), "tools"));
    let web = rows.iter().find(|r| r.id == "group:web").unwrap();
    let semantic = rows.iter().find(|r| r.id == "group:embsearch").unwrap();
    assert_eq!(web.value_suffix.as_deref(), Some("needs webtools"));
    assert!(web.description.contains("fetches the first time"));
    assert_eq!(semantic.value_suffix, None);
}

#[test]
fn says_the_binary_will_not_arrive_when_the_environment_cannot_fetch_it() {
    let _g = lock();
    let offline = ExternalToolStatus {
        downloadable: false,
        ..missing("webtools")
    };
    let web = leaf(&build_pane(vec![offline]), "webtools-timeout-secs");
    assert!(web.description.contains("will not fetch it"));
}
