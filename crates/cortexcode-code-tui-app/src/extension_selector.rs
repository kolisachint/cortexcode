//! The extension selector dialog: `components/extension-selector.ts` with
//! its parts (`selected-row-list.ts`, `dynamic-border.ts`,
//! `countdown-timer.ts`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cortexcode_code_tui_keybindings::{key_hint, raw_key_hint};
use cortexcode_code_tui_theme::{paint_selected_row, select_gutter, theme, SELECT_CURSOR};
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::{Component, ComponentHandle};
use cortexcode_tui_util::truncate_to_width;

use crate::input_frame::{InputFrame, InputFrameOptions};

/// `SelectableRow`: already-styled content, without the left margin.
#[derive(Debug, Clone, Default)]
pub struct SelectableRow {
    pub text: String,
    pub selected: bool,
}

/// `SelectedRowList`: picker rows rendered at terminal width, so the
/// selected one fills edge to edge.
pub struct SelectedRowList {
    rows: Vec<SelectableRow>,
    margin_x: usize,
}

impl SelectedRowList {
    pub fn new(rows: Vec<SelectableRow>, margin_x: usize) -> Self {
        Self { rows, margin_x }
    }

    pub fn set_rows(&mut self, rows: Vec<SelectableRow>) {
        self.rows = rows;
    }
}

impl Component for SelectedRowList {
    fn render(&mut self, width: u16) -> Vec<String> {
        let margin = " ".repeat(self.margin_x);
        self.rows
            .iter()
            .map(|row| {
                let line = truncate_to_width(
                    &format!("{margin}{}", row.text),
                    width as usize,
                    "...",
                    false,
                );
                if row.selected {
                    paint_selected_row(&line, width as usize)
                } else {
                    line
                }
            })
            .collect()
    }
}

/// A text styler.
pub type ColorFn = Box<dyn Fn(&str) -> String>;

/// `DynamicBorder`: a rule across the viewport.
pub struct DynamicBorder {
    color: ColorFn,
}

impl DynamicBorder {
    pub fn new(color: Option<ColorFn>) -> Self {
        Self {
            color: color.unwrap_or_else(|| Box::new(|s: &str| theme().fg("border", s))),
        }
    }
}

impl Component for DynamicBorder {
    fn render(&mut self, width: u16) -> Vec<String> {
        vec![(self.color)(&"─".repeat((width as usize).max(1)))]
    }
}

/// `CountdownTimer`, driven by the owner's loop: [`CountdownTimer::poll`]
/// advances it once a second has passed.
pub struct CountdownTimer {
    remaining_seconds: u64,
    next_tick: Option<Instant>,
}

impl CountdownTimer {
    /// Starts at `ceil(timeout / 1s)`; the caller shows that first value.
    pub fn new(timeout: Duration) -> Self {
        Self {
            remaining_seconds: timeout.as_millis().div_ceil(1000) as u64,
            next_tick: Some(Instant::now() + Duration::from_secs(1)),
        }
    }

    pub fn remaining_seconds(&self) -> u64 {
        self.remaining_seconds
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.next_tick
    }

    /// Tick if due: `Some(seconds)` after a tick, and `Some(0)` means expired.
    pub fn poll(&mut self, now: Instant) -> Option<u64> {
        let due = self.next_tick?;
        if now < due {
            return None;
        }
        self.remaining_seconds = self.remaining_seconds.saturating_sub(1);
        self.next_tick = if self.remaining_seconds == 0 {
            None
        } else {
            Some(due + Duration::from_secs(1))
        };
        Some(self.remaining_seconds)
    }

    pub fn dispose(&mut self) {
        self.next_tick = None;
    }
}

/// What the selector decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorOutcome {
    Selected(String),
    Cancelled,
}

/// `ExtensionSelectorComponent`: a titled frame of options, navigated with
/// the select keys (and `j`/`k`).
pub struct ExtensionSelectorComponent {
    frame: InputFrame,
    options: Vec<String>,
    selected_index: usize,
    list: Rc<RefCell<SelectedRowList>>,
    base_title: String,
    countdown: Option<CountdownTimer>,
    on_done: Box<dyn FnMut(SelectorOutcome)>,
}

impl ExtensionSelectorComponent {
    pub fn new(
        title: &str,
        options: Vec<String>,
        timeout: Option<Duration>,
        on_done: Box<dyn FnMut(SelectorOutcome)>,
    ) -> Self {
        let mut frame = InputFrame::new(InputFrameOptions {
            title: Some(title.to_string()),
            ..Default::default()
        });
        let countdown = timeout.filter(|t| !t.is_zero()).map(CountdownTimer::new);
        if let Some(c) = &countdown {
            frame.set_title(&format!("{title} ({}s)", c.remaining_seconds()));
        }
        let list = Rc::new(RefCell::new(SelectedRowList::new(Vec::new(), 1)));
        frame.add_child(list.clone() as ComponentHandle);
        frame.set_hint(&format!(
            "{}  {}  {}",
            raw_key_hint("↑↓", "navigate"),
            key_hint("tui.select.confirm", "select"),
            key_hint("tui.select.cancel", "cancel")
        ));
        let mut this = Self {
            frame,
            options,
            selected_index: 0,
            list,
            base_title: title.to_string(),
            countdown,
            on_done,
        };
        this.update_list();
        this
    }

    fn update_list(&mut self) {
        let t = theme();
        let rows = self
            .options
            .iter()
            .enumerate()
            .map(|(i, option)| {
                let selected = i == self.selected_index;
                SelectableRow {
                    text: if selected {
                        format!(
                            "{}{}",
                            t.fg("accent", SELECT_CURSOR),
                            t.fg("accent", option)
                        )
                    } else {
                        format!("{}{}", select_gutter(), t.fg("text", option))
                    },
                    selected,
                }
            })
            .collect();
        self.list.borrow_mut().set_rows(rows);
    }

    /// The countdown's next tick, if it runs.
    pub fn deadline(&self) -> Option<Instant> {
        self.countdown.as_ref().and_then(CountdownTimer::deadline)
    }

    /// Advance the countdown; true when the title changed (or it expired).
    pub fn poll(&mut self) -> bool {
        let Some(countdown) = &mut self.countdown else {
            return false;
        };
        match countdown.poll(Instant::now()) {
            None => false,
            Some(seconds) => {
                let title = format!("{} ({seconds}s)", self.base_title);
                self.frame.set_title(&title);
                if seconds == 0 {
                    self.dispose();
                    (self.on_done)(SelectorOutcome::Cancelled);
                }
                true
            }
        }
    }

    pub fn dispose(&mut self) {
        if let Some(c) = &mut self.countdown {
            c.dispose();
        }
    }
}

impl Component for ExtensionSelectorComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        if kb.matches(data, "tui.select.up") || data == "k" {
            self.selected_index = self.selected_index.saturating_sub(1);
            self.update_list();
        } else if kb.matches(data, "tui.select.down") || data == "j" {
            self.selected_index =
                (self.selected_index + 1).min(self.options.len().saturating_sub(1));
            self.update_list();
        } else if kb.matches(data, "tui.select.confirm") || data == "\n" {
            if let Some(selected) = self.options.get(self.selected_index).cloned() {
                (self.on_done)(SelectorOutcome::Selected(selected));
            }
        } else if kb.matches(data, "tui.select.cancel") {
            (self.on_done)(SelectorOutcome::Cancelled);
        }
    }
}
