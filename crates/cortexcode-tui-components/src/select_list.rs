//! Filterable, scrollable selection list, ported from `components/select-list.ts`.

use cortexcode_tui_keys::KeybindingsManager;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{truncate_to_width, visible_width};

use crate::color::ColorFn;

const DEFAULT_PRIMARY_COLUMN_WIDTH: usize = 32;
const PRIMARY_COLUMN_GAP: usize = 2;
const MIN_DESCRIPTION_WIDTH: usize = 10;

fn normalize_to_single_line(text: &str) -> String {
    text.split(['\r', '\n'])
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

#[derive(Debug, Clone)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

pub struct SelectListTheme {
    pub selected_prefix: ColorFn,
    pub selected_text: ColorFn,
    pub description: ColorFn,
    pub scroll_info: ColorFn,
    pub no_match: ColorFn,
}

pub struct SelectListTruncatePrimaryContext<'a> {
    pub text: &'a str,
    pub max_width: usize,
    pub column_width: usize,
    pub item: &'a SelectItem,
    pub is_selected: bool,
}

pub type TruncatePrimaryFn = Box<dyn Fn(&SelectListTruncatePrimaryContext) -> String>;
pub type SelectItemCallback = Box<dyn FnMut(&SelectItem)>;

#[derive(Default)]
pub struct SelectListLayoutOptions {
    pub min_primary_column_width: Option<usize>,
    pub max_primary_column_width: Option<usize>,
    pub truncate_primary: Option<TruncatePrimaryFn>,
}

pub struct SelectList {
    items: Vec<SelectItem>,
    filtered_items: Vec<SelectItem>,
    selected_index: usize,
    max_visible: usize,
    theme: SelectListTheme,
    layout: SelectListLayoutOptions,

    pub on_select: Option<SelectItemCallback>,
    pub on_cancel: Option<Box<dyn FnMut()>>,
    pub on_selection_change: Option<SelectItemCallback>,
}

impl SelectList {
    pub fn new(
        items: Vec<SelectItem>,
        max_visible: usize,
        theme: SelectListTheme,
        layout: SelectListLayoutOptions,
    ) -> Self {
        let filtered_items = items.clone();
        Self {
            items,
            filtered_items,
            selected_index: 0,
            max_visible,
            theme,
            layout,
            on_select: None,
            on_cancel: None,
            on_selection_change: None,
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        let filter_lower = filter.to_lowercase();
        self.filtered_items = self
            .items
            .iter()
            .filter(|item| item.value.to_lowercase().starts_with(&filter_lower))
            .cloned()
            .collect();
        self.selected_index = 0;
    }

    pub fn set_selected_index(&mut self, index: usize) {
        let max = self.filtered_items.len().saturating_sub(1);
        self.selected_index = index.min(max);
    }

    pub fn get_selected_item(&self) -> Option<&SelectItem> {
        self.filtered_items.get(self.selected_index)
    }

    pub fn handle_input_with(&mut self, key_data: &str, kb: &KeybindingsManager) {
        if kb.matches(key_data, "tui.select.up") {
            if self.filtered_items.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == 0 {
                self.filtered_items.len() - 1
            } else {
                self.selected_index - 1
            };
            self.notify_selection_change();
        } else if kb.matches(key_data, "tui.select.down") {
            if self.filtered_items.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == self.filtered_items.len() - 1 {
                0
            } else {
                self.selected_index + 1
            };
            self.notify_selection_change();
        } else if kb.matches(key_data, "tui.select.confirm") {
            if let Some(item) = self.filtered_items.get(self.selected_index).cloned() {
                if let Some(cb) = &mut self.on_select {
                    cb(&item);
                }
            }
        } else if kb.matches(key_data, "tui.select.cancel") {
            if let Some(cb) = &mut self.on_cancel {
                cb();
            }
        }
    }

    fn notify_selection_change(&mut self) {
        if let Some(item) = self.filtered_items.get(self.selected_index).cloned() {
            if let Some(cb) = &mut self.on_selection_change {
                cb(&item);
            }
        }
    }

    fn get_display_value(item: &SelectItem) -> &str {
        if item.label.is_empty() {
            &item.value
        } else {
            &item.label
        }
    }

    fn primary_column_bounds(&self) -> (usize, usize) {
        let raw_min = self
            .layout
            .min_primary_column_width
            .or(self.layout.max_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);
        let raw_max = self
            .layout
            .max_primary_column_width
            .or(self.layout.min_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);
        (raw_min.min(raw_max).max(1), raw_min.max(raw_max).max(1))
    }

    fn primary_column_width(&self) -> usize {
        let (min, max) = self.primary_column_bounds();
        let widest = self
            .filtered_items
            .iter()
            .map(|item| visible_width(Self::get_display_value(item)) + PRIMARY_COLUMN_GAP)
            .max()
            .unwrap_or(0);
        widest.clamp(min, max)
    }

    fn truncate_primary(
        &self,
        item: &SelectItem,
        is_selected: bool,
        max_width: usize,
        column_width: usize,
    ) -> String {
        let display_value = Self::get_display_value(item);
        let truncated_value = match &self.layout.truncate_primary {
            Some(f) => f(&SelectListTruncatePrimaryContext {
                text: display_value,
                max_width,
                column_width,
                item,
                is_selected,
            }),
            None => truncate_to_width(display_value, max_width, "...", false),
        };
        truncate_to_width(&truncated_value, max_width, "...", false)
    }

    fn render_item(
        &self,
        item: &SelectItem,
        is_selected: bool,
        width: usize,
        description_single_line: Option<&str>,
        primary_column_width: usize,
    ) -> String {
        let prefix = if is_selected { "→ " } else { "  " };
        let prefix_width = visible_width(prefix);

        if let Some(desc) = description_single_line {
            if width > 40 {
                let effective_primary_column_width = primary_column_width
                    .min(width.saturating_sub(prefix_width).saturating_sub(4))
                    .max(1);
                let max_primary_width = effective_primary_column_width
                    .saturating_sub(PRIMARY_COLUMN_GAP)
                    .max(1);
                let truncated_value = self.truncate_primary(
                    item,
                    is_selected,
                    max_primary_width,
                    effective_primary_column_width,
                );
                let truncated_value_width = visible_width(&truncated_value);
                let spacing = " ".repeat(
                    effective_primary_column_width
                        .saturating_sub(truncated_value_width)
                        .max(1),
                );
                let description_start = prefix_width + truncated_value_width + spacing.len();
                let remaining_width = (width as i64) - (description_start as i64) - 2;

                if remaining_width > MIN_DESCRIPTION_WIDTH as i64 {
                    let truncated_desc =
                        truncate_to_width(desc, remaining_width as usize, "...", false);
                    if is_selected {
                        return (self.theme.selected_text)(&format!(
                            "{prefix}{truncated_value}{spacing}{truncated_desc}"
                        ));
                    }
                    let desc_text = (self.theme.description)(&format!("{spacing}{truncated_desc}"));
                    return format!("{prefix}{truncated_value}{desc_text}");
                }
            }
        }

        let max_width = width.saturating_sub(prefix_width).saturating_sub(2).max(1);
        let truncated_value = self.truncate_primary(item, is_selected, max_width, max_width);
        if is_selected {
            (self.theme.selected_text)(&format!("{prefix}{truncated_value}"))
        } else {
            format!("{prefix}{truncated_value}")
        }
    }
}

impl Component for SelectList {
    fn render(&mut self, width: u16) -> Vec<String> {
        let width = width as usize;
        let mut lines = Vec::new();

        if self.filtered_items.is_empty() {
            lines.push((self.theme.no_match)("  No matching commands"));
            return lines;
        }

        let primary_column_width = self.primary_column_width();

        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(self.filtered_items.len().saturating_sub(self.max_visible));
        let end_index = (start_index + self.max_visible).min(self.filtered_items.len());

        for i in start_index..end_index {
            let item = &self.filtered_items[i];
            let is_selected = i == self.selected_index;
            let description_single_line = item.description.as_deref().map(normalize_to_single_line);
            lines.push(self.render_item(
                item,
                is_selected,
                width,
                description_single_line.as_deref(),
                primary_column_width,
            ));
        }

        if start_index > 0 || end_index < self.filtered_items.len() {
            let scroll_text = format!(
                "  ({}/{})",
                self.selected_index + 1,
                self.filtered_items.len()
            );
            lines.push((self.theme.scroll_info)(&truncate_to_width(
                &scroll_text,
                width.saturating_sub(2),
                "",
                false,
            )));
        }

        lines
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

    fn theme() -> SelectListTheme {
        SelectListTheme {
            selected_prefix: identity(),
            selected_text: identity(),
            description: identity(),
            scroll_info: identity(),
            no_match: identity(),
        }
    }

    fn kb() -> KeybindingsManager {
        KeybindingsManager::new(default_tui_keybindings(), HashMap::new())
    }

    fn items(n: usize) -> Vec<SelectItem> {
        (0..n)
            .map(|i| SelectItem {
                value: format!("item{i}"),
                label: format!("Item {i}"),
                description: None,
            })
            .collect()
    }

    #[test]
    fn renders_no_match_message_when_empty() {
        let mut list = SelectList::new(vec![], 5, theme(), SelectListLayoutOptions::default());
        let lines = list.render(40);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("No matching"));
    }

    #[test]
    fn renders_all_items_when_fewer_than_max_visible() {
        let mut list = SelectList::new(items(3), 5, theme(), SelectListLayoutOptions::default());
        let lines = list.render(40);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("Item 0"));
    }

    #[test]
    fn navigation_wraps_around() {
        let mut list = SelectList::new(items(3), 5, theme(), SelectListLayoutOptions::default());
        let kb = kb();
        list.handle_input_with("\x1b[A", &kb); // up wraps to last
        assert_eq!(list.get_selected_item().unwrap().value, "item2");
        list.handle_input_with("\x1b[B", &kb); // down wraps to first
        assert_eq!(list.get_selected_item().unwrap().value, "item0");
    }

    #[test]
    fn confirm_calls_on_select() {
        let mut list = SelectList::new(items(2), 5, theme(), SelectListLayoutOptions::default());
        let selected = std::rc::Rc::new(std::cell::RefCell::new(None));
        let selected_clone = selected.clone();
        list.on_select = Some(Box::new(move |item| {
            *selected_clone.borrow_mut() = Some(item.value.clone());
        }));
        list.handle_input_with("\r", &kb());
        assert_eq!(selected.borrow().as_deref(), Some("item0"));
    }

    #[test]
    fn cancel_calls_on_cancel() {
        let mut list = SelectList::new(items(2), 5, theme(), SelectListLayoutOptions::default());
        let cancelled = std::rc::Rc::new(std::cell::Cell::new(false));
        let cancelled_clone = cancelled.clone();
        list.on_cancel = Some(Box::new(move || cancelled_clone.set(true)));
        list.handle_input_with("\x1b", &kb());
        assert!(cancelled.get());
    }

    #[test]
    fn set_filter_narrows_items_by_value_prefix() {
        let mut list = SelectList::new(items(10), 5, theme(), SelectListLayoutOptions::default());
        list.set_filter("item1");
        // "item1" prefix matches item1 only among item0..item9 (item10+ doesn't exist here)
        assert_eq!(list.filtered_items.len(), 1);
        assert_eq!(list.get_selected_item().unwrap().value, "item1");
    }

    #[test]
    fn scroll_indicator_appears_when_more_items_than_visible() {
        let mut list = SelectList::new(items(10), 3, theme(), SelectListLayoutOptions::default());
        let lines = list.render(40);
        assert_eq!(lines.len(), 4); // 3 visible + scroll indicator
        assert!(lines[3].contains('/'));
    }
}
