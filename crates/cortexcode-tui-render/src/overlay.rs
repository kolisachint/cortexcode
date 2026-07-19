//! Overlay positioning types, ported from `tui.ts`'s `OverlayAnchor` /
//! `OverlayOptions` / `resolveOverlayLayout`.

/// Anchor position for overlays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayAnchor {
    #[default]
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    TopCenter,
    BottomCenter,
    LeftCenter,
    RightCenter,
}

/// Margin configuration for overlays.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayMargin {
    pub top: i64,
    pub right: i64,
    pub bottom: i64,
    pub left: i64,
}

impl OverlayMargin {
    pub fn all(value: i64) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

/// A value that can be absolute (cells) or a percentage of a reference size.
#[derive(Debug, Clone, Copy)]
pub enum SizeValue {
    Absolute(i64),
    Percent(f64),
}

/// Parse a [`SizeValue`] into an absolute value given a reference size.
pub fn parse_size_value(value: Option<SizeValue>, reference_size: i64) -> Option<i64> {
    match value? {
        SizeValue::Absolute(v) => Some(v),
        SizeValue::Percent(p) => Some(((reference_size as f64) * p / 100.0).floor() as i64),
    }
}

/// Options for overlay positioning and sizing.
#[derive(Debug, Clone, Default)]
pub struct OverlayOptions {
    // === Sizing ===
    pub width: Option<SizeValue>,
    pub min_width: Option<i64>,
    pub max_height: Option<SizeValue>,

    // === Positioning - anchor-based ===
    pub anchor: Option<OverlayAnchor>,
    pub offset_x: Option<i64>,
    pub offset_y: Option<i64>,

    // === Positioning - percentage or absolute ===
    pub row: Option<SizeValue>,
    pub col: Option<SizeValue>,

    // === Margin from terminal edges ===
    pub margin: Option<OverlayMargin>,

    // === Visibility ===
    /// If provided, overlay is only rendered when this returns true.
    pub visible: Option<fn(u16, u16) -> bool>,
    /// If true, don't capture keyboard focus when shown.
    pub non_capturing: bool,
}

/// Resolved layout for an overlay.
#[derive(Debug, Clone, Copy)]
pub struct OverlayLayout {
    pub width: i64,
    pub row: i64,
    pub col: i64,
    pub max_height: Option<i64>,
}

fn resolve_anchor_row(
    anchor: OverlayAnchor,
    height: i64,
    avail_height: i64,
    margin_top: i64,
) -> i64 {
    use OverlayAnchor::*;
    match anchor {
        TopLeft | TopCenter | TopRight => margin_top,
        BottomLeft | BottomCenter | BottomRight => margin_top + avail_height - height,
        LeftCenter | Center | RightCenter => margin_top + (avail_height - height) / 2,
    }
}

fn resolve_anchor_col(
    anchor: OverlayAnchor,
    width: i64,
    avail_width: i64,
    margin_left: i64,
) -> i64 {
    use OverlayAnchor::*;
    match anchor {
        TopLeft | LeftCenter | BottomLeft => margin_left,
        TopRight | RightCenter | BottomRight => margin_left + avail_width - width,
        TopCenter | Center | BottomCenter => margin_left + (avail_width - width) / 2,
    }
}

/// Resolve overlay layout from options, mirroring `TUI.resolveOverlayLayout`.
pub fn resolve_overlay_layout(
    options: Option<&OverlayOptions>,
    overlay_height: i64,
    term_width: i64,
    term_height: i64,
) -> OverlayLayout {
    let default_opts = OverlayOptions::default();
    let opt = options.unwrap_or(&default_opts);

    let margin = opt.margin.unwrap_or_default();
    let margin_top = margin.top.max(0);
    let margin_right = margin.right.max(0);
    let margin_bottom = margin.bottom.max(0);
    let margin_left = margin.left.max(0);

    let avail_width = (term_width - margin_left - margin_right).max(1);
    let avail_height = (term_height - margin_top - margin_bottom).max(1);

    let mut width =
        parse_size_value(opt.width, term_width).unwrap_or_else(|| 80i64.min(avail_width));
    if let Some(min_width) = opt.min_width {
        width = width.max(min_width);
    }
    width = width.max(1).min(avail_width);

    let mut max_height = parse_size_value(opt.max_height, term_height);
    if let Some(mh) = max_height {
        max_height = Some(mh.max(1).min(avail_height));
    }

    let effective_height = max_height.map_or(overlay_height, |mh| overlay_height.min(mh));

    let row = match opt.row {
        Some(SizeValue::Percent(p)) => {
            let max_row = (avail_height - effective_height).max(0);
            margin_top + ((max_row as f64) * p / 100.0).floor() as i64
        }
        Some(SizeValue::Absolute(v)) => v,
        None => {
            let anchor = opt.anchor.unwrap_or_default();
            resolve_anchor_row(anchor, effective_height, avail_height, margin_top)
        }
    };

    let col = match opt.col {
        Some(SizeValue::Percent(p)) => {
            let max_col = (avail_width - width).max(0);
            margin_left + ((max_col as f64) * p / 100.0).floor() as i64
        }
        Some(SizeValue::Absolute(v)) => v,
        None => {
            let anchor = opt.anchor.unwrap_or_default();
            resolve_anchor_col(anchor, width, avail_width, margin_left)
        }
    };

    let mut row = row;
    let mut col = col;
    if let Some(oy) = opt.offset_y {
        row += oy;
    }
    if let Some(ox) = opt.offset_x {
        col += ox;
    }

    row = row
        .max(margin_top)
        .min(term_height - margin_bottom - effective_height);
    col = col.max(margin_left).min(term_width - margin_right - width);

    OverlayLayout {
        width,
        row,
        col,
        max_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_value_absolute() {
        assert_eq!(
            parse_size_value(Some(SizeValue::Absolute(42)), 100),
            Some(42)
        );
    }

    #[test]
    fn parse_size_value_percent() {
        assert_eq!(
            parse_size_value(Some(SizeValue::Percent(50.0)), 100),
            Some(50)
        );
        assert_eq!(
            parse_size_value(Some(SizeValue::Percent(33.0)), 10),
            Some(3)
        );
    }

    #[test]
    fn parse_size_value_none() {
        assert_eq!(parse_size_value(None, 100), None);
    }

    #[test]
    fn resolve_overlay_layout_defaults_to_centered_80_wide() {
        let layout = resolve_overlay_layout(None, 5, 120, 40);
        assert_eq!(layout.width, 80);
        assert_eq!(layout.col, 20);
        assert_eq!(layout.row, 17); // (40 - 5) / 2 = 17
    }

    #[test]
    fn resolve_overlay_layout_respects_anchor_top_left() {
        let opts = OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Absolute(10)),
            ..Default::default()
        };
        let layout = resolve_overlay_layout(Some(&opts), 3, 100, 30);
        assert_eq!(layout.row, 0);
        assert_eq!(layout.col, 0);
        assert_eq!(layout.width, 10);
    }

    #[test]
    fn resolve_overlay_layout_clamps_to_available_space_with_margin() {
        let opts = OverlayOptions {
            margin: Some(OverlayMargin::all(2)),
            anchor: Some(OverlayAnchor::BottomRight),
            width: Some(SizeValue::Absolute(10)),
            ..Default::default()
        };
        let layout = resolve_overlay_layout(Some(&opts), 3, 50, 20);
        assert_eq!(layout.col, 50 - 2 - 10);
        assert_eq!(layout.row, 20 - 2 - 3);
    }

    #[test]
    fn resolve_overlay_layout_applies_offsets() {
        let opts = OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Absolute(10)),
            offset_x: Some(5),
            offset_y: Some(2),
            ..Default::default()
        };
        let layout = resolve_overlay_layout(Some(&opts), 3, 100, 30);
        assert_eq!(layout.row, 2);
        assert_eq!(layout.col, 5);
    }
}
