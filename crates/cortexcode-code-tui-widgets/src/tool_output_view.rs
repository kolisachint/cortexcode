//! `core/tool-output-view.ts`: the radar / peek / full dial.

pub use cortexcode_code_settings::ToolOutputView;

/// The dial's order, least to most. Cycling wraps at both ends.
pub const TOOL_OUTPUT_VIEWS: [ToolOutputView; 3] = [
    ToolOutputView::Radar,
    ToolOutputView::Peek,
    ToolOutputView::Full,
];

/// How many lines of a result the `peek` stop shows.
pub const PEEK_LINES: usize = 5;

/// Where a fresh install starts.
pub const DEFAULT_TOOL_OUTPUT_VIEW: ToolOutputView = ToolOutputView::Peek;

/// The stop `app.tools.expand` jumps to, and returns from.
pub const MAX_TOOL_OUTPUT_VIEW: ToolOutputView = ToolOutputView::Full;

/// One-line description per view, shared by the settings pane and `/hotkeys`.
pub fn tool_output_view_description(view: ToolOutputView) -> &'static str {
    match view {
        ToolOutputView::Radar => {
            "one line per run of tool calls; failures still show why they failed"
        }
        ToolOutputView::Peek => "the call line and the first few lines of the result",
        ToolOutputView::Full => "the call line and the whole result",
    }
}

/// `LEGACY_TOOL_OUTPUT_VIEWS`: values written by older versions.
pub const LEGACY_TOOL_OUTPUT_VIEWS: [(&str, ToolOutputView); 3] = [
    ("collapsed", ToolOutputView::Radar),
    ("glance", ToolOutputView::Peek),
    ("standard", ToolOutputView::Full),
];

/// `isToolOutputView`.
pub fn is_tool_output_view(value: Option<&str>) -> bool {
    value.is_some_and(|v| ToolOutputView::parse(v).is_some())
}

/// `cycleToolOutputView`: the next stop, wrapping at both ends.
pub fn cycle_tool_output_view(current: ToolOutputView, forward: bool) -> ToolOutputView {
    let index = TOOL_OUTPUT_VIEWS
        .iter()
        .position(|v| *v == current)
        .unwrap_or(0);
    let step = if forward {
        1
    } else {
        TOOL_OUTPUT_VIEWS.len() - 1
    };
    TOOL_OUTPUT_VIEWS[(index + step) % TOOL_OUTPUT_VIEWS.len()]
}
