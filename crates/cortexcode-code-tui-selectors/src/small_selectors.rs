//! The one-list pickers: `thinking-selector.ts`, `theme-selector.ts`,
//! `show-images-selector.ts` and `session-color-selector.ts`. Each is a
//! [`FramedSelectList`]; the callbacks go on its list.

use cortexcode_code_session::identity::{session_color_name, SESSION_COLOR_SLOTS};
use cortexcode_code_tui_theme::get_available_themes;
use cortexcode_code_tui_widgets::session_chip::render_session_chip;
use cortexcode_tui_components::SelectItem;

use crate::framed_list::{layout, FramedSelectList};

fn item(value: &str, label: &str, description: Option<String>) -> SelectItem {
    SelectItem {
        value: value.to_string(),
        label: label.to_string(),
        description,
    }
}

/// `LEVEL_DESCRIPTIONS`.
pub fn thinking_level_description(level: &str) -> Option<&'static str> {
    Some(match level {
        "off" => "No reasoning",
        "minimal" => "Very brief reasoning (~1k tokens)",
        "low" => "Light reasoning (~2k tokens)",
        "medium" => "Moderate reasoning (~8k tokens)",
        "high" => "Deep reasoning (~16k tokens)",
        "xhigh" => "Maximum reasoning (~32k tokens)",
        _ => return None,
    })
}

/// `ThinkingSelectorComponent`: the levels the model offers, the current one
/// selected. The selected item's value is the level.
pub fn thinking_selector(current_level: &str, available_levels: &[&str]) -> FramedSelectList {
    let items: Vec<SelectItem> = available_levels
        .iter()
        .map(|level| {
            item(
                level,
                level,
                thinking_level_description(level).map(str::to_string),
            )
        })
        .collect();
    let selected = items.iter().position(|i| i.value == current_level);
    let count = items.len();
    FramedSelectList::new("thinking", items, count, layout(12, 32), selected)
}

/// `ThemeSelectorComponent`: every available theme, the current one marked
/// and selected. Wire `on_selection_change` for the live preview.
pub fn theme_selector(current_theme: &str) -> FramedSelectList {
    let themes = get_available_themes();
    let items = themes
        .iter()
        .map(|name| {
            item(
                name,
                name,
                (name == current_theme).then(|| "(current)".to_string()),
            )
        })
        .collect();
    let selected = themes.iter().position(|t| t == current_theme);
    FramedSelectList::new("theme", items, 10, layout(12, 32), selected)
}

/// `ShowImagesSelectorComponent`: values `yes` / `no`.
pub fn show_images_selector(current_value: bool) -> FramedSelectList {
    let items = vec![
        item(
            "yes",
            "Yes",
            Some("Show images inline in terminal".to_string()),
        ),
        item(
            "no",
            "No",
            Some("Show text placeholder instead".to_string()),
        ),
    ];
    FramedSelectList::new(
        "images",
        items,
        5,
        layout(12, 32),
        Some(if current_value { 0 } else { 1 }),
    )
}

/// `SessionColorSelectorComponent`: one row per colour slot, each drawn as the
/// chip it would give the session, named (the name `/color` takes). Values are
/// the slot numbers.
pub fn session_color_selector(session_name: &str, current_slot: u8) -> FramedSelectList {
    let items: Vec<SelectItem> = (1..=SESSION_COLOR_SLOTS)
        .map(|slot| {
            let label = render_session_chip(session_name, i64::from(slot))
                .map(|chip| chip.styled)
                .unwrap_or_else(|| session_name.to_string());
            let name = session_color_name(slot)
                .map(str::to_string)
                .unwrap_or_else(|| slot.to_string());
            let description = if slot == current_slot {
                format!("{name} · current")
            } else {
                name
            };
            SelectItem {
                value: slot.to_string(),
                label,
                description: Some(description),
            }
        })
        .collect();
    let selected = items
        .iter()
        .position(|i| i.value == current_slot.to_string());
    let count = items.len();
    FramedSelectList::new("session colour", items, count, layout(26, 40), selected)
}
