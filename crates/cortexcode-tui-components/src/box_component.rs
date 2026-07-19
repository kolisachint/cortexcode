//! Padded, backgrounded container component, ported from `components/box.ts`.
//!
//! `Box` is a reserved keyword in most languages but not Rust; this module
//! is still named `box_component` to avoid clashing with `std::boxed`
//! nomenclature and keep `use` statements unambiguous. `Box` itself
//! (the type) is re-exported from `lib.rs`.

use cortexcode_tui_render::{Component, ComponentHandle};
use cortexcode_tui_util::{apply_background_to_line, visible_width};
use std::rc::Rc;

use crate::color::ColorFn;

pub struct BoxComponent {
    pub children: Vec<ComponentHandle>,
    padding_x: usize,
    padding_y: usize,
    bg_fn: Option<ColorFn>,
}

impl BoxComponent {
    pub fn new(padding_x: usize, padding_y: usize, bg_fn: Option<ColorFn>) -> Self {
        Self {
            children: Vec::new(),
            padding_x,
            padding_y,
            bg_fn,
        }
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

    pub fn set_bg_fn(&mut self, bg_fn: Option<ColorFn>) {
        self.bg_fn = bg_fn;
    }

    fn apply_bg(&self, line: &str, width: usize) -> String {
        let vis_len = visible_width(line);
        let pad_needed = width.saturating_sub(vis_len);
        let padded = format!("{line}{}", " ".repeat(pad_needed));
        match &self.bg_fn {
            Some(bg_fn) => apply_background_to_line(&padded, width, |s| bg_fn(s)),
            None => padded,
        }
    }
}

impl Default for BoxComponent {
    fn default() -> Self {
        Self::new(1, 1, None)
    }
}

impl Component for BoxComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        if self.children.is_empty() {
            return Vec::new();
        }

        let width = width as usize;
        let content_width = (width.saturating_sub(self.padding_x * 2)).max(1);
        let left_pad = " ".repeat(self.padding_x);

        let mut child_lines = Vec::new();
        for child in &self.children {
            for line in child.borrow_mut().render(content_width as u16) {
                child_lines.push(format!("{left_pad}{line}"));
            }
        }

        if child_lines.is_empty() {
            return Vec::new();
        }

        let mut result = Vec::with_capacity(child_lines.len() + self.padding_y * 2);
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }
        for line in &child_lines {
            result.push(self.apply_bg(line, width));
        }
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }

        result
    }

    fn invalidate(&mut self) {
        for child in &self.children {
            child.borrow_mut().invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fixed(Vec<String>);
    impl Component for Fixed {
        fn render(&mut self, _width: u16) -> Vec<String> {
            self.0.clone()
        }
    }

    #[test]
    fn empty_box_renders_nothing() {
        let mut b = BoxComponent::new(1, 1, None);
        assert_eq!(b.render(20), Vec::<String>::new());
    }

    #[test]
    fn wraps_children_with_padding() {
        let mut b = BoxComponent::new(1, 1, None);
        b.add_child(Rc::new(RefCell::new(Fixed(vec!["hi".to_string()]))));
        let lines = b.render(10);
        // top pad, content, bottom pad
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with(" hi"));
        assert_eq!(visible_width(&lines[1]), 10);
    }

    #[test]
    fn remove_child_drops_it_from_output() {
        let mut b = BoxComponent::new(0, 0, None);
        let child: ComponentHandle = Rc::new(RefCell::new(Fixed(vec!["x".to_string()])));
        b.add_child(child.clone());
        b.remove_child(&child);
        assert_eq!(b.render(10), Vec::<String>::new());
    }
}
