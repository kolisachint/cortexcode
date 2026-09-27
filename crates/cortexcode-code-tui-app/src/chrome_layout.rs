//! How much of the screen the chrome gets (`chrome-layout.ts`): one dial
//! (`ChromeDensity`, stepped by `alt+z`) and one table (`resolve_chrome`) from
//! (dial, what is happening) to what the footer and task ledger show. The
//! prompt is deliberately not in the table: it can never be hidden.
//!
//! A hidden slot renders nothing (see `Slot`), and `apply` reports whether
//! anything moved, so callers can push state on hot paths and render only
//! when a frame is owed.

use std::cell::RefCell;
use std::rc::Rc;

pub use cortexcode_code_settings::ChromeDensity;
use cortexcode_tui_render::Slot;

/// Below this many rows the dial starts at `compact` (read once, when no
/// stop is stored).
pub const SMALL_TERMINAL_ROWS: u16 = 25;

/// `ChromeInputs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeInputs {
    pub density: ChromeDensity,
    /// The prompt's completion list is open and wants the room.
    pub autocomplete_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterLayout {
    Full,
    Line,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksLayout {
    Full,
    Summary,
    Hidden,
}

/// `ChromeLayout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeLayout {
    pub footer: FooterLayout,
    pub tasks: TasksLayout,
}

/// `resolveChrome`: the whole policy. The dial says what you asked for; a
/// transient input (an open completion list) wins while it lasts.
pub fn resolve_chrome(inputs: ChromeInputs) -> ChromeLayout {
    let footer = if inputs.autocomplete_open {
        FooterLayout::Hidden
    } else {
        match inputs.density {
            ChromeDensity::Full => FooterLayout::Full,
            ChromeDensity::Compact => FooterLayout::Line,
            ChromeDensity::Bare => FooterLayout::Hidden,
        }
    };
    let tasks = match inputs.density {
        ChromeDensity::Bare => TasksLayout::Hidden,
        ChromeDensity::Compact => TasksLayout::Summary,
        ChromeDensity::Full => TasksLayout::Full,
    };
    ChromeLayout { footer, tasks }
}

/// What the controller needs of the footer and the ledger.
pub struct ChromeSurfaces {
    pub footer_slot: Rc<RefCell<Slot>>,
    pub tasks_slot: Rc<RefCell<Slot>>,
    /// Called with `Full` or `Line`.
    pub set_footer_density: Box<dyn FnMut(FooterLayout)>,
    /// Called with `Full` or `Summary`.
    pub set_tasks_density: Box<dyn FnMut(TasksLayout)>,
}

/// Holds the dial, takes the transient inputs, and moves the slots.
pub struct ChromeLayoutController {
    surfaces: ChromeSurfaces,
    inputs: ChromeInputs,
    applied: Option<ChromeLayout>,
}

impl ChromeLayoutController {
    pub fn new(surfaces: ChromeSurfaces, density: ChromeDensity) -> Self {
        Self {
            surfaces,
            inputs: ChromeInputs {
                density,
                autocomplete_open: false,
            },
            applied: None,
        }
    }

    pub fn density(&self) -> ChromeDensity {
        self.inputs.density
    }

    /// The layout on screen.
    pub fn layout(&self) -> ChromeLayout {
        self.applied.unwrap_or_else(|| resolve_chrome(self.inputs))
    }

    pub fn set_density(&mut self, density: ChromeDensity) -> bool {
        if self.inputs.density == density {
            return false;
        }
        self.inputs.density = density;
        self.apply()
    }

    /// Step the dial, wrapping.
    pub fn cycle_density(&mut self, forward: bool) -> ChromeDensity {
        let all = ChromeDensity::ALL;
        let at = all
            .iter()
            .position(|d| *d == self.inputs.density)
            .unwrap_or(0);
        let next = if forward {
            all[(at + 1) % all.len()]
        } else {
            all[(at + all.len() - 1) % all.len()]
        };
        self.set_density(next);
        next
    }

    pub fn set_autocomplete_open(&mut self, open: bool) -> bool {
        if self.inputs.autocomplete_open == open {
            return false;
        }
        self.inputs.autocomplete_open = open;
        self.apply()
    }

    /// Push the current layout onto the slots; true when anything moved.
    pub fn apply(&mut self) -> bool {
        let next = resolve_chrome(self.inputs);
        if self.applied == Some(next) {
            return false;
        }
        self.applied = Some(next);
        // Density before visibility: a slot about to show renders at its size.
        if next.footer != FooterLayout::Hidden {
            (self.surfaces.set_footer_density)(next.footer);
        }
        if next.tasks != TasksLayout::Hidden {
            (self.surfaces.set_tasks_density)(next.tasks);
        }
        self.surfaces
            .footer_slot
            .borrow_mut()
            .set_visible(next.footer != FooterLayout::Hidden);
        self.surfaces
            .tasks_slot
            .borrow_mut()
            .set_visible(next.tasks != TasksLayout::Hidden);
        true
    }
}
