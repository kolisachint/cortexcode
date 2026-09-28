//! `components/dynamic-border.ts`: a rule across the full width.

use cortexcode_code_tui_theme::theme;
use cortexcode_tui_components::ColorFn;
use cortexcode_tui_render::Component;

/// A `─` rule as wide as the viewport, in the border colour by default.
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
