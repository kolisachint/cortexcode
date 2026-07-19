//! Scrollable settings list with cycle-through values and submenus, ported
//! from `components/settings-list.ts`.
//!
//! Adaptation: the TypeScript `submenu` factory receives a `done` callback
//! closure that mutates the enclosing `SettingsList` when the submenu
//! finishes. Rust's borrow checker doesn't allow a closure to hold a
//! mutable reference back to its owner across calls, so the factory here
//! instead receives a shared `Rc<RefCell<Option<SubmenuOutcome>>>` "result
//! slot": the submenu component writes its outcome into the slot when
//! done, and `SettingsList` polls the slot after dispatching each input.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_tui_fuzzy::fuzzy_filter;
use cortexcode_tui_keys::KeybindingsManager;
use cortexcode_tui_render::{Component, ComponentHandle};
use cortexcode_tui_util::{truncate_to_width, visible_width, wrap_text_with_ansi};

use crate::color::ColorFn;
use crate::input::Input;

pub enum SubmenuOutcome {
    Selected(String),
    Cancelled,
}

pub type SubmenuFactory = Box<dyn Fn(&str, Rc<RefCell<Option<SubmenuOutcome>>>) -> ComponentHandle>;
pub type OnChangeFn = Box<dyn FnMut(&str, &str)>;

pub struct SettingItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub current_value: String,
    /// If provided, Enter/Space cycles through these values.
    pub values: Option<Vec<String>>,
    /// If provided, Enter opens this submenu.
    pub submenu: Option<SubmenuFactory>,
}

pub type LabelValueFn = Box<dyn Fn(&str, bool) -> String>;

pub struct SettingsListTheme {
    pub label: LabelValueFn,
    pub value: LabelValueFn,
    pub description: ColorFn,
    pub cursor: String,
    pub hint: ColorFn,
}

#[derive(Default)]
pub struct SettingsListOptions {
    pub enable_search: bool,
}

pub struct SettingsList {
    items: Vec<SettingItem>,
    filtered_indices: Vec<usize>,
    theme: SettingsListTheme,
    selected_index: usize,
    max_visible: usize,
    pub on_change: Option<OnChangeFn>,
    pub on_cancel: Option<Box<dyn FnMut()>>,
    search_input: Option<Input>,
    search_enabled: bool,

    submenu_component: Option<ComponentHandle>,
    submenu_result_slot: Option<Rc<RefCell<Option<SubmenuOutcome>>>>,
    submenu_item_index: Option<usize>,
}

impl SettingsList {
    pub fn new(
        items: Vec<SettingItem>,
        max_visible: usize,
        theme: SettingsListTheme,
        options: SettingsListOptions,
    ) -> Self {
        let filtered_indices = (0..items.len()).collect();
        let search_enabled = options.enable_search;
        Self {
            items,
            filtered_indices,
            theme,
            selected_index: 0,
            max_visible,
            on_change: None,
            on_cancel: None,
            search_input: if search_enabled {
                Some(Input::new())
            } else {
                None
            },
            search_enabled,
            submenu_component: None,
            submenu_result_slot: None,
            submenu_item_index: None,
        }
    }

    pub fn update_value(&mut self, id: &str, new_value: &str) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.current_value = new_value.to_string();
        }
    }

    fn render_main_list(&mut self, width: u16) -> Vec<String> {
        let width_usize = width as usize;
        let mut lines = Vec::new();

        if self.search_enabled {
            if let Some(input) = &mut self.search_input {
                lines.extend(input.render(width));
            }
            lines.push(String::new());
        }

        if self.items.is_empty() {
            lines.push((self.theme.hint)("  No settings available"));
            if self.search_enabled {
                self.add_hint_line(&mut lines, width_usize);
            }
            return lines;
        }

        let display_indices: Vec<usize> = if self.search_enabled {
            self.filtered_indices.clone()
        } else {
            (0..self.items.len()).collect()
        };

        if display_indices.is_empty() {
            lines.push(truncate_to_width(
                &(self.theme.hint)("  No matching settings"),
                width_usize,
                "...",
                false,
            ));
            self.add_hint_line(&mut lines, width_usize);
            return lines;
        }

        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(display_indices.len().saturating_sub(self.max_visible));
        let end_index = (start_index + self.max_visible).min(display_indices.len());

        let max_label_width = self
            .items
            .iter()
            .map(|i| visible_width(&i.label))
            .max()
            .unwrap_or(0)
            .min(30);

        for (i, &item_idx) in display_indices
            .iter()
            .enumerate()
            .take(end_index)
            .skip(start_index)
        {
            let item = &self.items[item_idx];
            let is_selected = i == self.selected_index;
            let prefix = if is_selected {
                self.theme.cursor.clone()
            } else {
                "  ".to_string()
            };
            let prefix_width = visible_width(&prefix);

            let label_padded = format!(
                "{}{}",
                item.label,
                " ".repeat(max_label_width.saturating_sub(visible_width(&item.label)))
            );
            let label_text = (self.theme.label)(&label_padded, is_selected);

            let separator = "  ";
            let used_width = prefix_width + max_label_width + visible_width(separator);
            let value_max_width = (width_usize as i64 - used_width as i64 - 2).max(0) as usize;

            let value_text = (self.theme.value)(
                &truncate_to_width(&item.current_value, value_max_width, "...", false),
                is_selected,
            );

            lines.push(truncate_to_width(
                &format!("{prefix}{label_text}{separator}{value_text}"),
                width_usize,
                "...",
                false,
            ));
        }

        if start_index > 0 || end_index < display_indices.len() {
            let scroll_text = format!("  ({}/{})", self.selected_index + 1, display_indices.len());
            lines.push((self.theme.hint)(&truncate_to_width(
                &scroll_text,
                width_usize.saturating_sub(2),
                "",
                false,
            )));
        }

        if let Some(&idx) = display_indices.get(self.selected_index) {
            if let Some(desc) = &self.items[idx].description {
                lines.push(String::new());
                let wrapped = wrap_text_with_ansi(desc, width_usize.saturating_sub(4));
                for line in wrapped {
                    lines.push((self.theme.description)(&format!("  {line}")));
                }
            }
        }

        self.add_hint_line(&mut lines, width_usize);

        lines
    }

    fn add_hint_line(&self, lines: &mut Vec<String>, width: usize) {
        lines.push(String::new());
        let text = if self.search_enabled {
            "  Type to search · Enter/Space to change · Esc to cancel"
        } else {
            "  Enter/Space to change · Esc to cancel"
        };
        lines.push(truncate_to_width(
            &(self.theme.hint)(text),
            width,
            "...",
            false,
        ));
    }

    pub fn handle_input_with(&mut self, data: &str, kb: &KeybindingsManager) {
        if let Some(submenu) = &self.submenu_component {
            submenu.borrow_mut().handle_input(data);
            self.check_submenu_result();
            return;
        }

        let display_indices: Vec<usize> = if self.search_enabled {
            self.filtered_indices.clone()
        } else {
            (0..self.items.len()).collect()
        };

        if kb.matches(data, "tui.select.up") {
            if display_indices.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == 0 {
                display_indices.len() - 1
            } else {
                self.selected_index - 1
            };
        } else if kb.matches(data, "tui.select.down") {
            if display_indices.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == display_indices.len() - 1 {
                0
            } else {
                self.selected_index + 1
            };
        } else if kb.matches(data, "tui.select.confirm") || data == " " {
            self.activate_item(&display_indices);
        } else if kb.matches(data, "tui.select.cancel") {
            if let Some(cb) = &mut self.on_cancel {
                cb();
            }
        } else if self.search_enabled {
            let sanitized: String = data.chars().filter(|&c| c != ' ').collect();
            if sanitized.is_empty() {
                return;
            }
            if let Some(input) = &mut self.search_input {
                input.handle_input_with(&sanitized, kb);
                let query = input.get_value().to_string();
                self.apply_filter(&query);
            }
        }
    }

    fn check_submenu_result(&mut self) {
        let Some(slot) = &self.submenu_result_slot else {
            return;
        };
        let outcome = slot.borrow_mut().take();
        if let Some(outcome) = outcome {
            match outcome {
                SubmenuOutcome::Selected(value) => {
                    if let Some(idx) = self.submenu_item_index {
                        self.items[idx].current_value = value.clone();
                        let id = self.items[idx].id.clone();
                        if let Some(cb) = &mut self.on_change {
                            cb(&id, &value);
                        }
                    }
                }
                SubmenuOutcome::Cancelled => {}
            }
            self.close_submenu();
        }
    }

    fn activate_item(&mut self, display_indices: &[usize]) {
        let Some(&idx) = display_indices.get(self.selected_index) else {
            return;
        };

        if self.items[idx].submenu.is_some() {
            self.submenu_item_index = Some(self.selected_index);
            let slot = Rc::new(RefCell::new(None));
            self.submenu_result_slot = Some(slot.clone());
            let current_value = self.items[idx].current_value.clone();
            let factory = self.items[idx].submenu.as_ref().unwrap();
            let component = factory(&current_value, slot);
            self.submenu_component = Some(component);
        } else if let Some(values) = &self.items[idx].values {
            if !values.is_empty() {
                let current_index = values
                    .iter()
                    .position(|v| v == &self.items[idx].current_value)
                    .unwrap_or(0);
                let next_index = (current_index + 1) % values.len();
                let new_value = values[next_index].clone();
                self.items[idx].current_value = new_value.clone();
                let id = self.items[idx].id.clone();
                if let Some(cb) = &mut self.on_change {
                    cb(&id, &new_value);
                }
            }
        }
    }

    fn close_submenu(&mut self) {
        self.submenu_component = None;
        self.submenu_result_slot = None;
        if let Some(idx) = self.submenu_item_index.take() {
            self.selected_index = idx;
        }
    }

    fn apply_filter(&mut self, query: &str) {
        struct View {
            index: usize,
            label: String,
        }
        let views: Vec<View> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| View {
                index: i,
                label: item.label.clone(),
            })
            .collect();
        impl Clone for View {
            fn clone(&self) -> Self {
                View {
                    index: self.index,
                    label: self.label.clone(),
                }
            }
        }
        let filtered = fuzzy_filter(&views, query, |v| v.label.clone());
        self.filtered_indices = filtered.into_iter().map(|v| v.index).collect();
        self.selected_index = 0;
    }
}

impl Component for SettingsList {
    fn render(&mut self, width: u16) -> Vec<String> {
        if let Some(submenu) = &self.submenu_component {
            return submenu.borrow_mut().render(width);
        }
        self.render_main_list(width)
    }

    fn invalidate(&mut self) {
        if let Some(submenu) = &self.submenu_component {
            submenu.borrow_mut().invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_tui_keys::default_tui_keybindings;
    use std::collections::HashMap;

    fn identity() -> ColorFn {
        Box::new(|s: &str| s.to_string())
    }

    fn theme() -> SettingsListTheme {
        SettingsListTheme {
            label: Box::new(|s: &str, _sel| s.to_string()),
            value: Box::new(|s: &str, _sel| s.to_string()),
            description: identity(),
            cursor: "> ".to_string(),
            hint: identity(),
        }
    }

    fn kb() -> KeybindingsManager {
        KeybindingsManager::new(default_tui_keybindings(), HashMap::new())
    }

    fn item(id: &str, values: Vec<&str>) -> SettingItem {
        SettingItem {
            id: id.to_string(),
            label: id.to_string(),
            description: None,
            current_value: values[0].to_string(),
            values: Some(values.into_iter().map(String::from).collect()),
            submenu: None,
        }
    }

    #[test]
    fn renders_no_settings_message_when_empty() {
        let mut list = SettingsList::new(vec![], 5, theme(), SettingsListOptions::default());
        let lines = list.render(40);
        assert!(lines.iter().any(|l| l.contains("No settings available")));
    }

    #[test]
    fn renders_items_with_labels_and_values() {
        let mut list = SettingsList::new(
            vec![item("a", vec!["on", "off"])],
            5,
            theme(),
            SettingsListOptions::default(),
        );
        let lines = list.render(40);
        assert!(lines[0].contains('a'));
        assert!(lines[0].contains("on"));
    }

    #[test]
    fn space_cycles_through_values() {
        let mut list = SettingsList::new(
            vec![item("a", vec!["on", "off"])],
            5,
            theme(),
            SettingsListOptions::default(),
        );
        let changed = Rc::new(RefCell::new(None));
        let changed_clone = changed.clone();
        list.on_change = Some(Box::new(move |id, value| {
            *changed_clone.borrow_mut() = Some((id.to_string(), value.to_string()));
        }));
        list.handle_input_with(" ", &kb());
        assert_eq!(changed.borrow().as_ref().unwrap().1, "off");
    }

    #[test]
    fn cancel_calls_on_cancel() {
        let mut list = SettingsList::new(
            vec![item("a", vec!["on"])],
            5,
            theme(),
            SettingsListOptions::default(),
        );
        let cancelled = Rc::new(RefCell::new(false));
        let cancelled_clone = cancelled.clone();
        list.on_cancel = Some(Box::new(move || *cancelled_clone.borrow_mut() = true));
        list.handle_input_with("\x1b", &kb());
        assert!(*cancelled.borrow());
    }

    #[test]
    fn navigation_wraps() {
        let mut list = SettingsList::new(
            vec![item("a", vec!["1"]), item("b", vec!["1"])],
            5,
            theme(),
            SettingsListOptions::default(),
        );
        list.handle_input_with("\x1b[A", &kb()); // up wraps to last
        assert_eq!(list.selected_index, 1);
    }
}
