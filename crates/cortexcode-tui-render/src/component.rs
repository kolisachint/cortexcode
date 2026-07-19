//! Component tree primitives, ported from `tui.ts`'s `Component`/`Container`.

use std::cell::RefCell;
use std::rc::Rc;

/// A node in the TUI component tree.
///
/// The TypeScript original uses optional interface methods
/// (`handleInput?`, `wantsKeyRelease?`) and a `Focusable` type guard
/// (`"focused" in component`). Rust has no structural typing, so those
/// become default-implemented trait methods instead: `handle_input` is a
/// no-op by default, and `is_focusable`/`set_focused` replace the
/// `Focusable` interface.
pub trait Component {
    /// Render the component to lines for the given viewport width.
    fn render(&mut self, width: u16) -> Vec<String>;

    /// Handle keyboard input when the component has focus.
    fn handle_input(&mut self, _data: &str) {}

    /// If true, the component receives key release events (Kitty protocol).
    fn wants_key_release(&self) -> bool {
        false
    }

    /// Invalidate any cached rendering state (e.g. on theme change).
    fn invalidate(&mut self) {}

    /// Whether this component can receive focus and display a hardware cursor.
    fn is_focusable(&self) -> bool {
        false
    }

    /// Called by [`crate::Tui`] when focus changes, for focusable components.
    fn set_focused(&mut self, _focused: bool) {}
}

pub type ComponentHandle = Rc<RefCell<dyn Component>>;

/// A component that contains other components, concatenating their
/// rendered lines. Unlike the TypeScript original, no reference-stable
/// flatten memoization is performed: children are always fully
/// re-rendered and re-concatenated. The differential terminal writer in
/// [`crate::Tui`] diffs by line *content*, not by array-reference
/// identity, so the visible output is unaffected — only the (JS-only)
/// micro-optimization of skipping unchanged subtrees by identity is not
/// ported.
#[derive(Default)]
pub struct Container {
    pub children: Vec<ComponentHandle>,
}

impl Container {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_child(&mut self, component: ComponentHandle) {
        self.children.push(component);
    }

    pub fn remove_child(&mut self, component: &ComponentHandle) {
        self.children.retain(|c| !Rc::ptr_eq(c, component));
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }
}

impl Component for Container {
    fn render(&mut self, width: u16) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &self.children {
            lines.extend(child.borrow_mut().render(width));
        }
        lines
    }

    fn invalidate(&mut self) {
        for child in &self.children {
            child.borrow_mut().invalidate();
        }
    }
}
