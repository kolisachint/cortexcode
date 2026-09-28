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

    /// The concrete component behind a handle, for components that opt in.
    /// TypeScript reaches into any object's fields; Rust code that needs the
    /// same (a caller inspecting a submenu a factory built) downcasts this.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
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
    /// Where each child's output started in the last render, and at what width.
    last_offsets: Option<(u16, Vec<usize>)>,
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

    /// Where each direct child's output starts, in rows, from the last render
    /// (`childRowOffsets`). `None` before the first render or at a different
    /// width: "no answer", not "zero".
    pub fn child_row_offsets(&self, width: u16) -> Option<Vec<usize>> {
        match &self.last_offsets {
            Some((w, offsets)) if *w == width && offsets.len() == self.children.len() => {
                Some(offsets.clone())
            }
            _ => None,
        }
    }
}

impl Component for Container {
    fn render(&mut self, width: u16) -> Vec<String> {
        let mut lines = Vec::new();
        let mut offsets = Vec::with_capacity(self.children.len());
        for child in &self.children {
            offsets.push(lines.len());
            lines.extend(child.borrow_mut().render(width));
        }
        self.last_offsets = Some((width, offsets));
        lines
    }

    fn invalidate(&mut self) {
        for child in &self.children {
            child.borrow_mut().invalidate();
        }
    }
}

/// A spacer whose height the renderer decides, so the layout can fill the
/// screen (`FlexSpacer`, from `components/spacer.ts`; it lives here because
/// the root sizes it). Put it between the part that flows from the top and
/// the chrome that hangs off the bottom, and hand it to
/// [`crate::Tui::set_flex_spacer`].
#[derive(Default)]
pub struct FlexSpacer {
    height: usize,
}

impl FlexSpacer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rows it is currently contributing.
    pub fn current_height(&self) -> usize {
        self.height
    }

    /// Set the fill; true when it changed and a re-flatten is owed.
    pub fn set_height(&mut self, height: i64) -> bool {
        let next = height.max(0) as usize;
        if next == self.height {
            return false;
        }
        self.height = next;
        true
    }
}

impl Component for FlexSpacer {
    fn render(&mut self, _width: u16) -> Vec<String> {
        vec![String::new(); self.height]
    }
}

/// A child that can be taken off screen (`Slot`). While hidden the child is
/// not rendered at all, so an animating child is paused, not merely hidden.
/// Swapping the occupant keeps the slot's place in the tree.
pub struct Slot {
    component: ComponentHandle,
    hidden: bool,
}

impl Slot {
    pub fn new(component: ComponentHandle) -> Self {
        Self {
            component,
            hidden: false,
        }
    }

    /// Whoever is in the slot right now.
    pub fn child(&self) -> ComponentHandle {
        self.component.clone()
    }

    /// Swap the occupant; returns whether anything changed.
    pub fn set_child(&mut self, component: ComponentHandle) -> bool {
        if Rc::ptr_eq(&self.component, &component) {
            return false;
        }
        self.component = component;
        true
    }

    pub fn visible(&self) -> bool {
        !self.hidden
    }

    /// Returns whether this changed anything, so callers can skip a render.
    pub fn set_visible(&mut self, visible: bool) -> bool {
        if self.hidden != visible {
            return false;
        }
        self.hidden = !visible;
        true
    }
}

impl Component for Slot {
    fn render(&mut self, width: u16) -> Vec<String> {
        if self.hidden {
            return Vec::new();
        }
        self.component.borrow_mut().render(width)
    }

    fn invalidate(&mut self) {
        self.component.borrow_mut().invalidate();
    }
}
