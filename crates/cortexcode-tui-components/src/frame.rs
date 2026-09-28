//! The frame the prompt box draws, and the one every surface that takes its
//! place draws with it: port of `components/frame.ts`.
//!
//! One renderer for the edge ([`render_frame_edge`]), one component for the
//! frame ([`Frame`]); the app points both at the same border style.

use cortexcode_tui_render::{Component, ComponentHandle, Container};
use cortexcode_tui_util::{truncate_to_width, visible_width};

use crate::color::ColorFn;

/// `box` rules all four sides; `rule` is the bare pair of horizontals; `none`
/// draws no border at all (for a component nested in a frame that already
/// rules it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameBorderStyle {
    #[default]
    Rule,
    Box,
    None,
}

impl FrameBorderStyle {
    /// Parse `"rule" | "box" | "none"`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rule" => Some(Self::Rule),
            "box" => Some(Self::Box),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Box => "box",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameBorderChars {
    pub horizontal: String,
    pub vertical: String,
    pub top_left: String,
    pub top_right: String,
    pub bottom_left: String,
    pub bottom_right: String,
}

impl Default for FrameBorderChars {
    /// `DEFAULT_FRAME_BORDER_CHARS`.
    fn default() -> Self {
        Self {
            horizontal: "─".into(),
            vertical: "│".into(),
            top_left: "┌".into(),
            top_right: "┐".into(),
            bottom_left: "└".into(),
            bottom_right: "┘".into(),
        }
    }
}

/// `Partial<FrameBorderChars>`: overrides on top of the defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameBorderCharsOverride {
    pub horizontal: Option<String>,
    pub vertical: Option<String>,
    pub top_left: Option<String>,
    pub top_right: Option<String>,
    pub bottom_left: Option<String>,
    pub bottom_right: Option<String>,
}

impl FrameBorderChars {
    /// `{ ...DEFAULT_FRAME_BORDER_CHARS, ...overrides }`.
    pub fn with_overrides(overrides: Option<&FrameBorderCharsOverride>) -> Self {
        let mut chars = Self::default();
        if let Some(o) = overrides {
            let pick = |v: &Option<String>, d: &mut String| {
                if let Some(v) = v {
                    *d = v.clone();
                }
            };
            pick(&o.horizontal, &mut chars.horizontal);
            pick(&o.vertical, &mut chars.vertical);
            pick(&o.top_left, &mut chars.top_left);
            pick(&o.top_right, &mut chars.top_right);
            pick(&o.bottom_left, &mut chars.bottom_left);
            pick(&o.bottom_right, &mut chars.bottom_right);
        }
        chars
    }
}

/// A label laid into a frame's top border, flush right. `plain` drives the
/// width math; `styled` is what is emitted and must occupy exactly
/// `visible_width(plain)` cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameLabel {
    pub plain: String,
    pub styled: String,
}

/// Border cells kept between the label and the top-right corner.
pub const LABEL_RIGHT_INSET: usize = 2;

/// Border cells required to the *left* of the label; below this the label is
/// dropped rather than drawn.
pub const MIN_LABEL_LEAD_IN: i64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameEdge {
    Top,
    Bottom,
}

pub struct FrameEdgeOptions<'a> {
    pub edge: FrameEdge,
    /// Cells between the corners in box mode; the whole width in rule mode.
    pub bar_width: usize,
    pub is_box: bool,
    pub chars: &'a FrameBorderChars,
    pub color: &'a dyn Fn(&str) -> String,
    /// Rides the top border only, and yields to the scroll indicator.
    pub label: Option<&'a FrameLabel>,
    /// Rows hidden past this edge, announced as `─── ↑ N more `.
    pub hidden: usize,
}

/// One horizontal edge of a frame (`renderFrameEdge`): the rule, the scroll
/// indicator that eats into it, the label that rides what is left, and the
/// corners in box mode.
pub fn render_frame_edge(options: &FrameEdgeOptions<'_>) -> String {
    let chars = options.chars;
    let color = options.color;
    let bar_width = options.bar_width;
    let indicator = if options.hidden > 0 {
        let arrow = match options.edge {
            FrameEdge::Top => "↑",
            FrameEdge::Bottom => "↓",
        };
        format!(
            "{} {arrow} {} more ",
            chars.horizontal.repeat(3),
            options.hidden
        )
    } else {
        String::new()
    };
    let indicator_width = visible_width(&indicator);

    let label = match options.edge {
        FrameEdge::Top => options.label,
        FrameEdge::Bottom => None,
    };
    let lead_in = match label {
        Some(l) => {
            bar_width as i64
                - indicator_width as i64
                - visible_width(&l.plain) as i64
                - LABEL_RIGHT_INSET as i64
        }
        None => -1,
    };

    let mut bar = match label {
        Some(l) if lead_in >= MIN_LABEL_LEAD_IN => format!(
            "{}{}{}",
            color(&format!(
                "{indicator}{}",
                chars.horizontal.repeat(lead_in as usize)
            )),
            l.styled,
            color(&chars.horizontal.repeat(LABEL_RIGHT_INSET))
        ),
        _ if indicator_width > 0 => {
            if bar_width >= indicator_width {
                color(&format!(
                    "{indicator}{}",
                    chars.horizontal.repeat(bar_width - indicator_width)
                ))
            } else {
                color(&truncate_to_width(&indicator, bar_width, "...", false))
            }
        }
        _ => color(&chars.horizontal.repeat(bar_width)),
    };
    if !options.is_box {
        return bar;
    }
    // Corners must stay aligned, so pad back any width lost to truncation.
    bar.push_str(&color(
        &chars
            .horizontal
            .repeat(bar_width.saturating_sub(visible_width(&bar))),
    ));
    let (left, right) = match options.edge {
        FrameEdge::Top => (&chars.top_left, &chars.top_right),
        FrameEdge::Bottom => (&chars.bottom_left, &chars.bottom_right),
    };
    format!("{}{bar}{}", color(left), color(right))
}

/// `FrameOptions`.
#[derive(Default)]
pub struct FrameOptions {
    /// Defaults to `box`.
    pub border: Option<FrameBorderStyle>,
    /// Columns of gutter inside the side borders. Defaults to 1.
    pub padding_x: Option<usize>,
    pub border_chars: Option<FrameBorderCharsOverride>,
    pub color: Option<ColorFn>,
}

/// A container that frames its children (`Frame`). Children render at the
/// width left inside the border and gutter, and every row is padded out to
/// it. A frame with nothing in it draws nothing.
pub struct Frame {
    pub container: Container,
    pub border_color: ColorFn,
    /// Laid into the top border, flush right. `None` draws a plain edge.
    pub label: Option<FrameLabel>,
    border: FrameBorderStyle,
    padding_x: usize,
    border_chars: FrameBorderChars,
}

impl Frame {
    pub fn new(options: FrameOptions) -> Self {
        Self {
            container: Container::new(),
            border_color: options
                .color
                .unwrap_or_else(|| Box::new(|s: &str| s.to_string())),
            label: None,
            border: options.border.unwrap_or(FrameBorderStyle::Box),
            padding_x: options.padding_x.unwrap_or(1),
            border_chars: FrameBorderChars::with_overrides(options.border_chars.as_ref()),
        }
    }

    pub fn add_child(&mut self, component: ComponentHandle) {
        self.container.add_child(component);
    }

    pub fn remove_child(&mut self, component: &ComponentHandle) {
        self.container.remove_child(component);
    }

    pub fn clear(&mut self) {
        self.container.clear();
    }

    pub fn get_border(&self) -> FrameBorderStyle {
        self.border
    }

    pub fn set_border(&mut self, border: FrameBorderStyle) {
        self.border = border;
    }

    pub fn set_padding_x(&mut self, padding_x: usize) {
        self.padding_x = padding_x;
    }

    /// Lay a label into the top border, or clear it.
    pub fn set_label(&mut self, label: Option<FrameLabel>) {
        self.label = label;
    }

    /// The width children are handed at this frame width.
    pub fn content_width(&self, width: usize) -> usize {
        if self.border == FrameBorderStyle::None {
            return width.max(1);
        }
        let inner = self.inner_width(width);
        inner
            .saturating_sub(self.resolve_padding_x(inner) * 2)
            .max(1)
    }

    /// Whether a label of this plain text would ride the top border at this
    /// width rather than being dropped.
    pub fn label_fits(&self, plain: &str, width: usize) -> bool {
        if self.border == FrameBorderStyle::None || plain.is_empty() {
            return false;
        }
        self.inner_width(width) as i64 - visible_width(plain) as i64 - LABEL_RIGHT_INSET as i64
            >= MIN_LABEL_LEAD_IN
    }

    /// Box mode needs two columns for the sides plus one of content.
    fn is_box(&self, width: usize) -> bool {
        self.border == FrameBorderStyle::Box && width >= 4
    }

    fn inner_width(&self, width: usize) -> usize {
        width
            .saturating_sub(if self.is_box(width) { 2 } else { 0 })
            .max(1)
    }

    fn resolve_padding_x(&self, inner_width: usize) -> usize {
        self.padding_x.min((inner_width.saturating_sub(1)) / 2)
    }
}

impl Component for Frame {
    fn render(&mut self, width: u16) -> Vec<String> {
        let width = width as usize;
        if self.border == FrameBorderStyle::None {
            return self.container.render(width as u16);
        }
        let is_box = self.is_box(width);
        let inner_width = self.inner_width(width);
        let padding_x = self.resolve_padding_x(inner_width);
        let content_width = inner_width.saturating_sub(padding_x * 2).max(1);

        let child_lines = self.container.render(content_width as u16);
        if child_lines.is_empty() {
            return Vec::new();
        }

        let color = |s: &str| (self.border_color)(s);
        let edge = |which| {
            render_frame_edge(&FrameEdgeOptions {
                edge: which,
                bar_width: inner_width,
                is_box,
                chars: &self.border_chars,
                color: &color,
                label: self.label.as_ref(),
                hidden: 0,
            })
        };
        let vertical = if is_box {
            color(&self.border_chars.vertical)
        } else {
            String::new()
        };
        let gutter = " ".repeat(padding_x);
        let mut lines = vec![edge(FrameEdge::Top)];
        for line in child_lines {
            // A child that overruns its width is cut rather than trusted.
            let body = if visible_width(&line) > content_width {
                truncate_to_width(&line, content_width, "...", false)
            } else {
                line
            };
            let fill = " ".repeat(content_width.saturating_sub(visible_width(&body)));
            lines.push(format!("{vertical}{gutter}{body}{fill}{gutter}{vertical}"));
        }
        lines.push(edge(FrameEdge::Bottom));
        lines
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}
