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

/// The paper treatment a box asks its owner for on every frame (`PaperSheet`):
/// the ink of its shadow and the gutter it holds back from the right margin.
/// A sheet with neither is the plain full-width band.
#[derive(Default)]
pub struct PaperSheet {
    /// Paints the shadow's own glyphs. `None`: the sheet casts no shadow.
    pub shadow: Option<ColorFn>,
    /// Columns of page held back at the right margin, so the sheet has a right
    /// edge to show and somewhere to put the shadow's column.
    pub inset: Option<usize>,
}

/// Resolves the paper treatment per frame, so a box already on screen follows
/// a theme switch.
pub type PaperFn = Box<dyn Fn() -> Option<PaperSheet>>;

pub struct BoxComponent {
    pub children: Vec<ComponentHandle>,
    padding_x: usize,
    padding_y: usize,
    bg_fn: Option<ColorFn>,
    paper_fn: Option<PaperFn>,
}

impl BoxComponent {
    pub fn new(padding_x: usize, padding_y: usize, bg_fn: Option<ColorFn>) -> Self {
        Self {
            children: Vec::new(),
            padding_x,
            padding_y,
            bg_fn,
            paper_fn: None,
        }
    }

    /// Change the horizontal padding after construction (`setPaddingX`): with
    /// no band to draw, padding is just an indent.
    pub fn set_padding_x(&mut self, padding_x: usize) {
        self.padding_x = padding_x;
    }

    /// Give the box the paper treatment, resolved on every frame (`setPaper`).
    ///
    /// The shadow is one extra row of `▔` (an upper one-eighth block) indented
    /// one column; an inset box also gets a column of `▏` down its right edge,
    /// starting on the second row, so the two meet as an L. No provider, or
    /// one returning `None`, draws the plain full-width band.
    pub fn set_paper(&mut self, paper_fn: Option<PaperFn>) {
        self.paper_fn = paper_fn;
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
        // Asked for per frame, so a box on screen follows a theme switch.
        let paper = self.paper_fn.as_ref().and_then(|f| f());
        let paper_inset = paper.as_ref().and_then(|p| p.inset).unwrap_or(0);
        // A sheet needs a band wide enough to carry its own bottom run; below
        // that the box falls back to the plain full-width band.
        let wide = width as i64 - paper_inset as i64 > 1;
        let shadow_fn = if wide {
            paper.as_ref().and_then(|p| p.shadow.as_ref())
        } else {
            None
        };
        let inset = if wide { paper_inset } else { 0 };

        let band_width = width.saturating_sub(inset).max(1);
        // Padding gives way rather than pushing content off the end of the band.
        let padding_x = self.padding_x.min((band_width - 1) / 2);
        let content_width = band_width.saturating_sub(padding_x * 2).max(1);
        let left_pad = " ".repeat(padding_x);

        let mut child_lines = Vec::new();
        for child in &self.children {
            for line in child.borrow_mut().render(content_width as u16) {
                child_lines.push(format!("{left_pad}{line}"));
            }
        }

        if child_lines.is_empty() {
            return Vec::new();
        }

        let mut rows: Vec<String> = Vec::with_capacity(child_lines.len() + self.padding_y * 2);
        rows.extend(std::iter::repeat_n(String::new(), self.padding_y));
        rows.extend(child_lines);
        rows.extend(std::iter::repeat_n(String::new(), self.padding_y));

        // The right-hand column only exists when a gutter was reserved for it.
        let has_column = shadow_fn.is_some() && inset > 0;
        let mut result: Vec<String> = rows
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let band = self.apply_bg(line, band_width);
                // The first row has no column: the offset is down *and* right.
                let column = match shadow_fn {
                    Some(shadow) if has_column && index > 0 => shadow("\u{258f}"),
                    _ => String::new(),
                };
                format!("{band}{column}")
            })
            .collect();

        // The shadow's bottom run, offset one column right of the band.
        if let Some(shadow) = shadow_fn {
            if band_width > 1 {
                result.push(format!(" {}", shadow(&"\u{2594}".repeat(band_width - 1))));
            }
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
