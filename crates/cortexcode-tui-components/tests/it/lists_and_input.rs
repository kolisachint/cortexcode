//! The pin's additions to `select-list.test.ts`, `settings-list.test.ts`,
//! `input.test.ts` (the caret) and `list-right-margin.test.ts`.

use cortexcode_tui_components::*;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;

fn id() -> ColorFn {
    Box::new(|t: &str| t.to_string())
}

fn select_theme(band: bool) -> SelectListTheme {
    SelectListTheme {
        selected_prefix: id(),
        selected_text: id(),
        description: id(),
        scroll_info: id(),
        no_match: id(),
        cursor: None,
        selected_row: band.then(|| -> ColorFn { Box::new(|t: &str| format!("[BG{t}/BG]")) }),
    }
}

fn item(value: &str, description: Option<&str>) -> SelectItem {
    SelectItem {
        value: value.into(),
        label: value.into(),
        description: description.map(String::from),
    }
}

fn select(items: Vec<SelectItem>, band: bool) -> SelectList {
    SelectList::new(
        items,
        5,
        select_theme(band),
        SelectListLayoutOptions::default(),
    )
}

fn unband(row: &str) -> &str {
    &row[3..row.len() - 4]
}

#[test]
fn select_rows_unpadded_without_a_band() {
    let rendered = select(vec![item("first", None), item("second", None)], false).render(40);
    assert_eq!(visible_width(&rendered[0]), visible_width("› first"));
    assert!(rendered[0].starts_with("› first"));
    assert!(rendered[1].starts_with("  second"));
}

#[test]
fn select_band_fills_the_selected_row() {
    let rendered = select(vec![item("first", None), item("second", None)], true).render(40);
    assert!(rendered[0].starts_with("[BG"));
    assert!(rendered[0].ends_with("/BG]"));
    assert_eq!(visible_width(unband(&rendered[0])), 40);
    assert!(!rendered[1].contains("[BG"));
    assert_eq!(visible_width(&rendered[1]), visible_width("  second"));
}

#[test]
fn select_band_covers_the_description_column() {
    let rendered = select(vec![item("first", Some("does a thing"))], true).render(60);
    assert!(rendered[0].contains("does a thing"));
    assert_eq!(visible_width(unband(&rendered[0])), 60);
}

fn settings_theme(cursor: &str, band: bool) -> SettingsListTheme {
    SettingsListTheme {
        label: Box::new(|t: &str, _| t.to_string()),
        value: Box::new(|t: &str, _| t.to_string()),
        description: id(),
        cursor: cursor.into(),
        hint: id(),
        selected_row: band.then(|| -> ColorFn { Box::new(|t: &str| format!("[BG{t}/BG]")) }),
    }
}

fn setting(id: &str, label: &str, value: &str) -> SettingItem {
    SettingItem {
        id: id.into(),
        label: label.into(),
        description: None,
        current_value: value.into(),
        value_suffix: None,
        keywords: None,
        values: None,
        submenu: None,
    }
}

fn settings(band: bool) -> SettingsList {
    SettingsList::new(
        vec![
            setting("theme", "Theme", "dark"),
            setting("thinking", "Thinking", "medium"),
        ],
        5,
        settings_theme("→ ", band),
        SettingsListOptions::default(),
    )
}

fn rows_of(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .filter(|l| l.contains("Theme") || l.contains("Thinking"))
        .collect()
}

#[test]
fn settings_rows_unpadded_without_a_band() {
    let rows = rows_of(settings(false).render(60));
    assert!(visible_width(&rows[0]) < 60);
}

#[test]
fn settings_band_fills_only_the_selected_row() {
    let rows = rows_of(settings(true).render(60));
    assert!(rows[0].starts_with("[BG"));
    assert!(rows[0].ends_with("/BG]"));
    assert_eq!(visible_width(unband(&rows[0])), 60);
    assert!(!rows[1].contains("[BG"));
}

#[test]
fn settings_value_suffix_and_keywords() {
    let mut s = setting("a", "Alpha", "on");
    s.value_suffix = Some("~120 tok".into());
    s.keywords = Some("gamma delta".into());
    let mut list = SettingsList::new(
        vec![s, setting("b", "Beta", "off")],
        5,
        settings_theme("› ", false),
        SettingsListOptions {
            enable_search: true,
        },
    );
    let lines = list.render(60);
    assert!(lines.iter().any(|l| l.contains("on  ~120 tok")));
    // Searching a keyword finds the row that stands for it.
    let kb = cortexcode_tui_keys::KeybindingsManager::new(
        cortexcode_tui_keys::default_tui_keybindings(),
        Default::default(),
    );
    for c in "delta".chars() {
        list.handle_input_with(&c.to_string(), &kb);
    }
    let lines = list.render(60);
    assert!(lines.iter().any(|l| l.contains("Alpha")));
    assert!(!lines.iter().any(|l| l.contains("Beta")));
}

// list-right-margin.test.ts

const WIDTHS: [u16; 4] = [120, 100, 80, 60];
const ALL_WIDTHS: [u16; 10] = [120, 100, 80, 60, 41, 40, 24, 12, 6, 2];

fn long() -> String {
    "wide ".repeat(60)
}

#[test]
fn select_description_runs_to_the_last_cell() {
    for width in WIDTHS {
        let row = &select(vec![item("alpha", Some(&long()))], false).render(width)[0];
        assert_eq!(visible_width(row), width as usize, "@{width}");
    }
}

#[test]
fn select_never_overflows() {
    for width in ALL_WIDTHS {
        for row in select(vec![item("alpha", Some(&long()))], false).render(width) {
            assert!(visible_width(&row) <= width as usize, "@{width}: {row:?}");
        }
    }
}

#[test]
fn select_value_runs_to_the_last_cell_without_a_description() {
    for width in ALL_WIDTHS {
        let row = &select(vec![item(&"v".repeat(400), None)], false).render(width)[0];
        assert_eq!(visible_width(row), width as usize, "@{width}");
    }
}

#[test]
fn settings_value_runs_to_the_last_cell() {
    for width in WIDTHS {
        let mut s = setting("a", "a setting", &long());
        s.values = Some(vec![long()]);
        let mut list = SettingsList::new(
            vec![s],
            5,
            settings_theme("› ", false),
            SettingsListOptions::default(),
        );
        let row = &list.render(width)[0];
        assert_eq!(visible_width(row), width as usize, "@{width}");
    }
}

#[test]
fn settings_description_wraps_against_its_own_indent() {
    let width = 60;
    let mut s = setting("a", "a setting", "on");
    s.values = Some(vec!["on".into()]);
    s.description = Some(long());
    let mut list = SettingsList::new(
        vec![s],
        5,
        settings_theme("› ", false),
        SettingsListOptions::default(),
    );
    let wrapped: Vec<String> = list
        .render(width)
        .into_iter()
        .filter(|l| l.starts_with("  wide"))
        .collect();
    assert!(wrapped.len() > 1);
    for line in &wrapped {
        assert!(visible_width(line) <= width as usize);
    }
    assert!(wrapped
        .iter()
        .any(|l| visible_width(l) > width as usize - 8));
}

// input.test.ts: the caret

fn strip(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(pos) = rest.find("\x1b[") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        let n = after
            .bytes()
            .take_while(|b| b.is_ascii_digit() || *b == b';')
            .count();
        rest = &after[n + 1..];
    }
    out.push_str(rest);
    out
}

#[test]
fn caret_is_the_prompts_and_follows_the_value() {
    let mut input = Input::new();
    input.set_value("abc");
    assert_eq!(DEFAULT_INPUT_PROMPT, "❯");
    assert!(strip(&input.render(20)[0]).starts_with("❯ abc"));
}

#[test]
fn caret_is_coloured_without_stealing_a_column() {
    let mut plain = Input::new();
    let mut coloured = Input::new();
    coloured.prompt_color = Box::new(|t: &str| format!("\x1b[2m{t}\x1b[22m"));
    plain.set_value("x".repeat(40));
    coloured.set_value("x".repeat(40));
    assert_eq!(
        visible_width(&coloured.render(20)[0]),
        visible_width(&plain.render(20)[0])
    );
    assert!(coloured.render(20)[0].starts_with("\x1b[2m❯ \x1b[22m"));
}

#[test]
fn caret_can_be_turned_off() {
    let mut bare = Input::new();
    bare.prompt_prefix = String::new();
    bare.set_value("abc");
    assert!(strip(&bare.render(20)[0]).starts_with("abc"));
    assert_eq!(visible_width(&bare.render(20)[0]), 20);
}
