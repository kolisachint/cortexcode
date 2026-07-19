//! A [`Loader`] that can be cancelled with Escape, ported from
//! `components/cancellable-loader.ts`.
//!
//! TypeScript's `AbortController`/`AbortSignal` are replaced by a minimal
//! `Rc<Cell<bool>>`-backed [`AbortSignal`], cheap to clone and check from
//! async work driven on the same thread as the component tree.

use std::cell::Cell;
use std::rc::Rc;

use cortexcode_tui_keys::KeybindingsManager;
use cortexcode_tui_render::Component;

use crate::color::ColorFn;
use crate::loader::{Loader, LoaderIndicatorOptions};

#[derive(Clone, Default)]
pub struct AbortSignal(Rc<Cell<bool>>);

impl AbortSignal {
    pub fn aborted(&self) -> bool {
        self.0.get()
    }
}

pub struct CancellableLoader {
    loader: Loader,
    aborted: Rc<Cell<bool>>,
    /// Called when the user presses Escape / Ctrl+C.
    pub on_abort: Option<Box<dyn FnMut()>>,
}

impl CancellableLoader {
    pub fn new(
        spinner_color_fn: ColorFn,
        message_color_fn: ColorFn,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        Self {
            loader: Loader::new(spinner_color_fn, message_color_fn, message, indicator),
            aborted: Rc::new(Cell::new(false)),
            on_abort: None,
        }
    }

    pub fn signal(&self) -> AbortSignal {
        AbortSignal(self.aborted.clone())
    }

    pub fn aborted(&self) -> bool {
        self.aborted.get()
    }

    pub fn tick(&mut self) -> bool {
        self.loader.tick()
    }

    pub fn interval(&self) -> std::time::Duration {
        self.loader.interval()
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.loader.set_message(message);
    }

    pub fn dispose(&mut self) {
        self.loader.stop();
    }

    /// Handle raw input, aborting on the `tui.select.cancel` keybinding
    /// (Escape / Ctrl+C by default).
    pub fn handle_input_with(&mut self, data: &str, keybindings: &KeybindingsManager) {
        if keybindings.matches(data, "tui.select.cancel") {
            self.aborted.set(true);
            if let Some(cb) = &mut self.on_abort {
                cb();
            }
        }
    }
}

impl Component for CancellableLoader {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.loader.render(width)
    }

    fn invalidate(&mut self) {
        self.loader.invalidate();
    }

    // `handle_input` (the Component trait method, no keybindings param) is
    // intentionally left as the trait's no-op default: callers that have a
    // KeybindingsManager available should call `handle_input_with` directly
    // instead of going through the generic Component dispatch.
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_tui_keys::default_tui_keybindings;
    use std::collections::HashMap;

    fn identity() -> ColorFn {
        Box::new(|s: &str| s.to_string())
    }

    fn keybindings() -> KeybindingsManager {
        KeybindingsManager::new(default_tui_keybindings(), HashMap::new())
    }

    #[test]
    fn escape_aborts_and_calls_on_abort() {
        let mut loader = CancellableLoader::new(identity(), identity(), "Working...", None);
        let called = Rc::new(Cell::new(false));
        let called_clone = called.clone();
        loader.on_abort = Some(Box::new(move || called_clone.set(true)));

        let kb = keybindings();
        assert!(!loader.aborted());
        loader.handle_input_with("\x1b", &kb);
        assert!(loader.aborted());
        assert!(called.get());
    }

    #[test]
    fn signal_reflects_abort_state() {
        let mut loader = CancellableLoader::new(identity(), identity(), "Working...", None);
        let signal = loader.signal();
        assert!(!signal.aborted());
        loader.handle_input_with("\x1b", &keybindings());
        assert!(signal.aborted());
    }

    #[test]
    fn non_cancel_input_does_not_abort() {
        let mut loader = CancellableLoader::new(identity(), identity(), "Working...", None);
        loader.handle_input_with("a", &keybindings());
        assert!(!loader.aborted());
    }
}
