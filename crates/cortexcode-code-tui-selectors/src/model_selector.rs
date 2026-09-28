//! The `/model` picker, hoocode `components/model-selector.ts`.
//!
//! Adaptations: the original loads the available models itself (refreshing
//! the registry first) and saves the pick as the new default before calling
//! back; here the owner hands in what it loaded, and does the save when the
//! [`ModelSelectorEvent::Select`] arrives.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_ai_models::models_are_equal;
use cortexcode_ai_types::Model;
use cortexcode_code_tui_keybindings::key_hint;
use cortexcode_code_tui_theme::{select_gutter, style_input, theme, SELECT_CURSOR};
use cortexcode_code_tui_widgets::input_frame::{InputFrame, InputFrameOptions};
use cortexcode_code_tui_widgets::selected_row_list::{SelectableRow, SelectedRowList};
use cortexcode_tui_components::{Input, Spacer, Text};
use cortexcode_tui_fuzzy::fuzzy_filter;
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::{Component, ComponentHandle};

use crate::config_selector::locale_compare;

/// Which list the picker shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelScope {
    All,
    Scoped,
}

/// What the picker asks its owner to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelSelectorEvent {
    Select(Box<Model>),
    Cancel,
}

struct State {
    current: Option<Model>,
    all: Vec<Model>,
    scoped: Vec<Model>,
    scope: ModelScope,
    filtered: Vec<Model>,
    selected: usize,
    error: Option<String>,
}

impl State {
    fn active(&self) -> &[Model] {
        match self.scope {
            ModelScope::All => &self.all,
            ModelScope::Scoped => &self.scoped,
        }
    }

    /// `filterModels`.
    fn filter(&mut self, query: &str) {
        self.filtered = if query.is_empty() {
            self.active().to_vec()
        } else {
            fuzzy_filter(self.active(), query, |m| {
                let (id, provider) = (&m.id, &m.provider);
                format!("{id} {provider} {provider}/{id} {provider} {id}")
            })
        };
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    fn scope_text(&self) -> String {
        let t = theme();
        let pick = |scope, label| {
            t.fg(
                if self.scope == scope {
                    "accent"
                } else {
                    "muted"
                },
                label,
            )
        };
        format!(
            "{}{}{}{}",
            t.fg("muted", "Scope: "),
            pick(ModelScope::All, "all"),
            t.fg("muted", " | "),
            pick(ModelScope::Scoped, "scoped")
        )
    }
}

fn scope_hint_text() -> String {
    key_hint("tui.input.tab", "scope") + &theme().fg("muted", " (all/scoped)")
}

/// The list under the query line (`updateList`).
struct ModelList(Rc<RefCell<State>>);

impl Component for ModelList {
    fn render(&mut self, width: u16) -> Vec<String> {
        let t = theme();
        let state = self.0.borrow();
        const MAX_VISIBLE: usize = 10;
        let len = state.filtered.len();
        let start = (state.selected as isize - (MAX_VISIBLE / 2) as isize)
            .min(len as isize - MAX_VISIBLE as isize)
            .max(0) as usize;
        let end = (start + MAX_VISIBLE).min(len);
        let rows = (start..end)
            .map(|i| {
                let model = &state.filtered[i];
                let selected = i == state.selected;
                let prefix = if selected {
                    t.fg("accent", SELECT_CURSOR)
                } else {
                    select_gutter()
                };
                let id = if selected {
                    t.fg("accent", &model.id)
                } else {
                    model.id.clone()
                };
                let badge = t.fg("muted", &format!("[{}]", model.provider));
                let check = if models_are_equal(state.current.as_ref(), Some(model)) {
                    t.fg("success", " ✓")
                } else {
                    String::new()
                };
                SelectableRow {
                    text: format!("{prefix}{id} {badge}{check}"),
                    selected,
                }
            })
            .collect();
        let mut lines = SelectedRowList::new(rows, 0).render(width);
        let mut add = |text: String| lines.extend(Text::new(text, 0, 0).render(width));
        if start > 0 || end < len {
            add(t.fg("muted", &format!("  ({}/{len})", state.selected + 1)));
        }
        if let Some(error) = &state.error {
            for line in error.split('\n') {
                add(t.fg("error", line));
            }
        } else if len == 0 {
            add(t.fg("muted", "  No matching models"));
        } else {
            let name = &state.filtered[state.selected].name;
            lines.extend(Spacer::new(1).render(width));
            lines.extend(
                Text::new(t.fg("muted", &format!("  Model Name: {name}")), 0, 0).render(width),
            );
        }
        lines
    }
}

/// `ModelSelectorComponent`: a searchable model list, with an all/scoped
/// switch when the session has a model scope.
pub struct ModelSelectorComponent {
    frame: InputFrame,
    search: Rc<RefCell<Input>>,
    state: Rc<RefCell<State>>,
    scope_text: Option<Rc<RefCell<Text>>>,
    events: Vec<ModelSelectorEvent>,
    submitted: Rc<RefCell<bool>>,
}

impl ModelSelectorComponent {
    /// `available` is the registry's models with configured auth, or why they
    /// could not be listed; `load_error` is a models.json problem (built-ins
    /// still list). `scoped` is the session's model scope, in its order.
    pub fn new(
        current: Option<Model>,
        available: Result<Vec<Model>, String>,
        load_error: Option<String>,
        scoped: Vec<Model>,
        initial_search: Option<&str>,
    ) -> Self {
        let scope = if scoped.is_empty() {
            ModelScope::All
        } else {
            ModelScope::Scoped
        };
        let (all, scoped, error) = match available {
            Ok(models) => {
                let mut sorted = models;
                // Current model first, then by provider (a stable sort).
                sorted.sort_by(|a, b| {
                    let a_current = models_are_equal(current.as_ref(), Some(a));
                    let b_current = models_are_equal(current.as_ref(), Some(b));
                    b_current
                        .cmp(&a_current)
                        .then_with(|| locale_compare(&a.provider, &b.provider))
                });
                (sorted, scoped, load_error)
            }
            Err(error) => (Vec::new(), Vec::new(), Some(error)),
        };
        let mut state = State {
            current,
            all,
            scoped,
            scope,
            filtered: Vec::new(),
            selected: 0,
            error,
        };
        state.filtered = state.active().to_vec();
        state.selected = state
            .filtered
            .iter()
            .position(|m| models_are_equal(state.current.as_ref(), Some(m)))
            .unwrap_or(0);

        let mut frame = InputFrame::new(InputFrameOptions {
            title: Some("model".to_string()),
            ..Default::default()
        });
        let mut scope_text = None;
        if state.scope == ModelScope::Scoped {
            let text = Rc::new(RefCell::new(Text::new(state.scope_text(), 0, 0)));
            frame.add_child(text.clone());
            frame.add_child(Rc::new(RefCell::new(Text::new(scope_hint_text(), 0, 0))));
            scope_text = Some(text);
        } else {
            let hint =
                "Only showing models from configured providers. Use /login to add providers.";
            frame.add_child(Rc::new(RefCell::new(Text::new(
                theme().fg("warning", hint),
                0,
                0,
            ))));
        }
        let mut input = Input::new();
        style_input(&mut input);
        if let Some(search) = initial_search.filter(|s| !s.is_empty()) {
            input.set_value(search);
            state.filter(search);
        }
        let submitted: Rc<RefCell<bool>> = Rc::default();
        let flag = submitted.clone();
        input.on_submit = Some(Box::new(move |_: &str| *flag.borrow_mut() = true));
        let search = Rc::new(RefCell::new(input));
        frame.add_child(search.clone());
        frame.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        let state = Rc::new(RefCell::new(state));
        frame.add_child(Rc::new(RefCell::new(ModelList(state.clone()))) as ComponentHandle);
        Self {
            frame,
            search,
            state,
            scope_text,
            events: Vec::new(),
            submitted,
        }
    }

    /// Events since the last call.
    pub fn take_events(&mut self) -> Vec<ModelSelectorEvent> {
        std::mem::take(&mut self.events)
    }

    /// The query line.
    pub fn search_value(&self) -> String {
        self.search.borrow().get_value().to_string()
    }

    fn select_current(&mut self) {
        let state = self.state.borrow();
        if let Some(model) = state.filtered.get(state.selected) {
            self.events
                .push(ModelSelectorEvent::Select(Box::new(model.clone())));
        }
    }

    /// `setScope`.
    fn set_scope(&mut self, scope: ModelScope) {
        let query = self.search_value();
        let mut state = self.state.borrow_mut();
        if state.scope == scope {
            return;
        }
        state.scope = scope;
        state.selected = state
            .active()
            .iter()
            .position(|m| models_are_equal(state.current.as_ref(), Some(m)))
            .unwrap_or(0);
        state.filter(&query);
        if let Some(text) = &self.scope_text {
            text.borrow_mut().set_text(state.scope_text());
        }
    }
}

impl Component for ModelSelectorComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        if kb.matches(data, "tui.input.tab") {
            let next = {
                let state = self.state.borrow();
                (!state.scoped.is_empty()).then_some(match state.scope {
                    ModelScope::All => ModelScope::Scoped,
                    ModelScope::Scoped => ModelScope::All,
                })
            };
            if let Some(next) = next {
                self.set_scope(next);
            }
            return;
        }
        if kb.matches(data, "tui.select.up") {
            let mut state = self.state.borrow_mut();
            let len = state.filtered.len();
            if len > 0 {
                state.selected = if state.selected == 0 {
                    len - 1
                } else {
                    state.selected - 1
                };
            }
        } else if kb.matches(data, "tui.select.down") {
            let mut state = self.state.borrow_mut();
            let len = state.filtered.len();
            if len > 0 {
                state.selected = if state.selected + 1 == len {
                    0
                } else {
                    state.selected + 1
                };
            }
        } else if kb.matches(data, "tui.select.confirm") {
            self.select_current();
        } else if kb.matches(data, "tui.select.cancel") {
            self.events.push(ModelSelectorEvent::Cancel);
        } else {
            self.search.borrow_mut().handle_input_with(data, &kb);
            if std::mem::take(&mut *self.submitted.borrow_mut()) {
                self.select_current();
            }
            let query = self.search_value();
            self.state.borrow_mut().filter(&query);
        }
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
