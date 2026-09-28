//! The `/scoped-models` picker, hoocode `components/scoped-models-selector.ts`:
//! which models model cycling steps through, and in what order. Changes are
//! session-only until saved.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use cortexcode_ai_types::Model;
use cortexcode_code_tui_keybindings::key_text;
use cortexcode_code_tui_theme::{select_gutter, style_input, theme, SELECT_CURSOR};
use cortexcode_code_tui_widgets::input_frame::{InputFrame, InputFrameOptions};
use cortexcode_code_tui_widgets::selected_row_list::{SelectableRow, SelectedRowList};
use cortexcode_tui_components::{Input, Spacer, Text};
use cortexcode_tui_fuzzy::fuzzy_filter;
use cortexcode_tui_keys::{get_keybindings, matches_key};
use cortexcode_tui_render::{Component, ComponentHandle};

/// `None` = all enabled (no filter); `Some` = an explicit ordered list of
/// `provider/id`s.
pub type EnabledIds = Option<Vec<String>>;

fn is_enabled(enabled: &EnabledIds, id: &str) -> bool {
    enabled
        .as_ref()
        .is_none_or(|ids| ids.iter().any(|i| i == id))
}

/// `toggle`: the first toggle from all-enabled keeps only that model.
pub fn toggle(enabled: &EnabledIds, id: &str) -> EnabledIds {
    let Some(ids) = enabled else {
        return Some(vec![id.to_string()]);
    };
    let mut ids = ids.clone();
    match ids.iter().position(|i| i == id) {
        Some(index) => {
            ids.remove(index);
        }
        None => ids.push(id.to_string()),
    }
    Some(ids)
}

/// `enableAll`: `targets` (default every model) on; all on reads as `None`.
pub fn enable_all(enabled: &EnabledIds, all: &[String], targets: Option<&[String]>) -> EnabledIds {
    let ids = enabled.as_ref()?;
    let mut result = ids.clone();
    for id in targets.unwrap_or(all) {
        if !result.contains(id) {
            result.push(id.clone());
        }
    }
    (result.len() != all.len()).then_some(result)
}

/// `clearAll`: `targets` (default every enabled model) off.
pub fn clear_all(enabled: &EnabledIds, all: &[String], targets: Option<&[String]>) -> EnabledIds {
    match enabled {
        None => Some(match targets {
            Some(targets) => all
                .iter()
                .filter(|id| !targets.contains(id))
                .cloned()
                .collect(),
            None => Vec::new(),
        }),
        Some(ids) => {
            let targets: HashSet<&String> = targets.unwrap_or(ids).iter().collect();
            Some(
                ids.iter()
                    .filter(|id| !targets.contains(id))
                    .cloned()
                    .collect(),
            )
        }
    }
}

/// `move`: swap `id` with its neighbour `delta` away, when there is one.
pub fn move_id(enabled: &EnabledIds, id: &str, delta: isize) -> EnabledIds {
    let ids = enabled.as_ref()?;
    let mut list = ids.clone();
    if let Some(index) = list.iter().position(|i| i == id) {
        let target = index as isize + delta;
        if target >= 0 && (target as usize) < list.len() {
            list.swap(index, target as usize);
        }
    }
    Some(list)
}

/// `getSortedIds`: the enabled models in their order, then the rest.
fn sorted_ids(enabled: &EnabledIds, all: &[String]) -> Vec<String> {
    let Some(ids) = enabled else {
        return all.to_vec();
    };
    let set: HashSet<&String> = ids.iter().collect();
    ids.iter()
        .cloned()
        .chain(all.iter().filter(|id| !set.contains(id)).cloned())
        .collect()
}

#[derive(Clone)]
struct Item {
    full_id: String,
    model: Model,
    enabled: bool,
}

/// What the picker asks its owner to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopedModelsEvent {
    /// The enabled set or order changed (session-only).
    Change(EnabledIds),
    /// Save the current selection to settings.
    Persist(EnabledIds),
    Cancel,
}

struct State {
    models: HashMap<String, Model>,
    all: Vec<String>,
    enabled: EnabledIds,
    filtered: Vec<Item>,
    selected: usize,
    dirty: bool,
}

impl State {
    fn build_items(&self) -> Vec<Item> {
        // IDs whose model is gone (e.g. after logout) drop out.
        sorted_ids(&self.enabled, &self.all)
            .into_iter()
            .filter_map(|id| {
                let model = self.models.get(&id)?.clone();
                Some(Item {
                    enabled: is_enabled(&self.enabled, &id),
                    full_id: id,
                    model,
                })
            })
            .collect()
    }

    fn footer_text(&self) -> String {
        let t = theme();
        let count = match &self.enabled {
            None => "all enabled".to_string(),
            Some(ids) => format!("{}/{} enabled", ids.len(), self.all.len()),
        };
        let parts = [
            format!("{} toggle", key_text("tui.select.confirm")),
            format!("{} all", key_text("app.models.enableAll")),
            format!("{} clear", key_text("app.models.clearAll")),
            format!("{} provider", key_text("app.models.toggleProvider")),
            format!(
                "{}/{} reorder",
                key_text("app.models.reorderUp"),
                key_text("app.models.reorderDown")
            ),
            format!("{} save", key_text("app.models.save")),
            count,
        ];
        let joined = parts.join(" · ");
        if self.dirty {
            t.fg("dim", &format!("  {joined} ")) + &t.fg("warning", "(unsaved)")
        } else {
            t.fg("dim", &format!("  {joined}"))
        }
    }
}

/// The list under the query line (`updateList`).
struct ModelList(Rc<RefCell<State>>);

impl Component for ModelList {
    fn render(&mut self, width: u16) -> Vec<String> {
        let t = theme();
        let state = self.0.borrow();
        let len = state.filtered.len();
        if len == 0 {
            return Text::new(t.fg("muted", "  No matching models"), 0, 0).render(width);
        }
        const MAX_VISIBLE: usize = 8;
        let start = (state.selected as isize - (MAX_VISIBLE / 2) as isize)
            .min(len as isize - MAX_VISIBLE as isize)
            .max(0) as usize;
        let end = (start + MAX_VISIBLE).min(len);
        let all_enabled = state.enabled.is_none();
        let rows = (start..end)
            .map(|i| {
                let item = &state.filtered[i];
                let selected = i == state.selected;
                let prefix = if selected {
                    t.fg("accent", SELECT_CURSOR)
                } else {
                    select_gutter()
                };
                let id = if selected {
                    t.fg("accent", &item.model.id)
                } else {
                    item.model.id.clone()
                };
                let badge = t.fg("muted", &format!(" [{}]", item.model.provider));
                let status = if all_enabled {
                    String::new()
                } else if item.enabled {
                    t.fg("success", " ✓")
                } else {
                    t.fg("dim", " ✗")
                };
                SelectableRow {
                    text: format!("{prefix}{id}{badge}{status}"),
                    selected,
                }
            })
            .collect();
        let mut lines = SelectedRowList::new(rows, 0).render(width);
        if start > 0 || end < len {
            let info = t.fg("muted", &format!("  ({}/{len})", state.selected + 1));
            lines.extend(Text::new(info, 0, 0).render(width));
        }
        let name = &state.filtered[state.selected].model.name;
        lines.extend(Spacer::new(1).render(width));
        lines
            .extend(Text::new(t.fg("muted", &format!("  Model Name: {name}")), 0, 0).render(width));
        lines
    }
}

/// `ScopedModelsSelectorComponent`.
pub struct ScopedModelsSelectorComponent {
    frame: InputFrame,
    search: Rc<RefCell<Input>>,
    state: Rc<RefCell<State>>,
    footer: Rc<RefCell<Text>>,
    events: Vec<ScopedModelsEvent>,
}

impl ScopedModelsSelectorComponent {
    pub fn new(all_models: Vec<Model>, enabled_model_ids: EnabledIds) -> Self {
        let mut models = HashMap::new();
        let mut all = Vec::new();
        for model in all_models {
            let full_id = format!("{}/{}", model.provider, model.id);
            all.push(full_id.clone());
            models.insert(full_id, model);
        }
        let mut state = State {
            models,
            all,
            enabled: enabled_model_ids,
            filtered: Vec::new(),
            selected: 0,
            dirty: false,
        };
        state.filtered = state.build_items();

        let mut frame = InputFrame::new(InputFrameOptions::default());
        frame.set_title("models");
        let t = theme();
        frame.add_child(Rc::new(RefCell::new(Text::new(
            t.fg(
                "muted",
                &format!(
                    "Session-only. {} to save to settings.",
                    key_text("app.models.save")
                ),
            ),
            0,
            0,
        ))));
        let mut input = Input::new();
        style_input(&mut input);
        let search = Rc::new(RefCell::new(input));
        frame.add_child(search.clone());
        frame.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        let footer = Rc::new(RefCell::new(Text::new(state.footer_text(), 0, 0)));
        let state = Rc::new(RefCell::new(state));
        frame.add_child(Rc::new(RefCell::new(ModelList(state.clone()))) as ComponentHandle);
        frame.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        frame.add_child(footer.clone());
        Self {
            frame,
            search,
            state,
            footer,
            events: Vec::new(),
        }
    }

    /// Events since the last call.
    pub fn take_events(&mut self) -> Vec<ScopedModelsEvent> {
        std::mem::take(&mut self.events)
    }

    fn query(&self) -> String {
        self.search.borrow().get_value().to_string()
    }

    /// `refresh`: rebuild the rows and the footer.
    fn refresh(&mut self) {
        let query = self.query();
        let mut state = self.state.borrow_mut();
        let items = state.build_items();
        state.filtered = if query.is_empty() {
            items
        } else {
            fuzzy_filter(&items, &query, |i| {
                format!("{} {}", i.model.id, i.model.provider)
            })
        };
        state.selected = state.selected.min(state.filtered.len().saturating_sub(1));
        self.footer.borrow_mut().set_text(state.footer_text());
    }

    fn notify_change(&mut self) {
        let enabled = self.state.borrow().enabled.clone();
        self.events.push(ScopedModelsEvent::Change(enabled));
    }

    /// Set a new enabled set: marked unsaved, redrawn, reported.
    fn apply(&mut self, enabled: EnabledIds) {
        {
            let mut state = self.state.borrow_mut();
            state.enabled = enabled;
            state.dirty = true;
        }
        self.refresh();
        self.notify_change();
    }

    /// The filtered ids when a search is active (the bulk keys act on those).
    fn search_targets(&self) -> Option<Vec<String>> {
        if self.query().is_empty() {
            return None;
        }
        Some(
            self.state
                .borrow()
                .filtered
                .iter()
                .map(|i| i.full_id.clone())
                .collect(),
        )
    }
}

impl Component for ScopedModelsSelectorComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        if kb.matches(data, "tui.select.up") || kb.matches(data, "tui.select.down") {
            let up = kb.matches(data, "tui.select.up");
            let mut state = self.state.borrow_mut();
            let len = state.filtered.len();
            if len > 0 {
                state.selected = match (up, state.selected) {
                    (true, 0) => len - 1,
                    (true, i) => i - 1,
                    (false, i) if i + 1 == len => 0,
                    (false, i) => i + 1,
                };
            }
            return;
        }

        let reorder_up = kb.matches(data, "app.models.reorderUp");
        let reorder_down = kb.matches(data, "app.models.reorderDown");
        if reorder_up || reorder_down {
            let (enabled, item, selected) = {
                let state = self.state.borrow();
                (
                    state.enabled.clone(),
                    state
                        .filtered
                        .get(state.selected)
                        .map(|i| i.full_id.clone()),
                    state.selected,
                )
            };
            let (Some(ids), Some(item)) = (&enabled, item) else {
                return;
            };
            let Some(index) = ids.iter().position(|i| *i == item) else {
                return;
            };
            let delta: isize = if reorder_up { -1 } else { 1 };
            let target = index as isize + delta;
            if target >= 0 && (target as usize) < ids.len() {
                self.state.borrow_mut().selected = (selected as isize + delta).max(0) as usize;
                self.apply(move_id(&enabled, &item, delta));
            }
            return;
        }

        if kb.matches(data, "tui.select.confirm") {
            let (enabled, item) = {
                let state = self.state.borrow();
                (
                    state.enabled.clone(),
                    state
                        .filtered
                        .get(state.selected)
                        .map(|i| i.full_id.clone()),
                )
            };
            if let Some(item) = item {
                self.apply(toggle(&enabled, &item));
            }
            return;
        }

        let enable = kb.matches(data, "app.models.enableAll");
        if enable || kb.matches(data, "app.models.clearAll") {
            let targets = self.search_targets();
            let next = {
                let state = self.state.borrow();
                if enable {
                    enable_all(&state.enabled, &state.all, targets.as_deref())
                } else {
                    clear_all(&state.enabled, &state.all, targets.as_deref())
                }
            };
            self.apply(next);
            return;
        }

        if kb.matches(data, "app.models.toggleProvider") {
            let next = {
                let state = self.state.borrow();
                state.filtered.get(state.selected).map(|item| {
                    let provider = &item.model.provider;
                    let ids: Vec<String> = state
                        .all
                        .iter()
                        .filter(|id| &state.models[*id].provider == provider)
                        .cloned()
                        .collect();
                    if ids.iter().all(|id| is_enabled(&state.enabled, id)) {
                        clear_all(&state.enabled, &state.all, Some(&ids))
                    } else {
                        enable_all(&state.enabled, &state.all, Some(&ids))
                    }
                })
            };
            if let Some(next) = next {
                self.apply(next);
            }
            return;
        }

        if kb.matches(data, "app.models.save") {
            let enabled = {
                let mut state = self.state.borrow_mut();
                state.dirty = false;
                self.footer.borrow_mut().set_text(state.footer_text());
                state.enabled.clone()
            };
            self.events.push(ScopedModelsEvent::Persist(enabled));
            return;
        }

        if matches_key(data, "ctrl+c") {
            if self.query().is_empty() {
                self.events.push(ScopedModelsEvent::Cancel);
            } else {
                self.search.borrow_mut().set_value("");
                self.refresh();
            }
            return;
        }

        if matches_key(data, "escape") {
            self.events.push(ScopedModelsEvent::Cancel);
            return;
        }

        self.search.borrow_mut().handle_input_with(data, &kb);
        self.refresh();
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.search.borrow_mut().set_focused(focused);
    }
}
