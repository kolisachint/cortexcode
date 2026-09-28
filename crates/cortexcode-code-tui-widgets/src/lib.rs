//! The coding agent's chat transcript widgets, ported from hoocode's
//! `modes/interactive/components/`.

mod assistant_message;
pub mod bash_execution;
pub mod brand;
pub mod diff;
pub mod dynamic_border;
pub mod input_frame;
mod js_json;
pub mod jsdiff;
pub mod read_output;
pub mod render_utils;
pub mod session_chip;
pub mod tool_chain;
pub mod tool_chain_summary;
pub mod tool_execution;
pub mod tool_output_view;
pub mod tool_signal;
pub mod tools;
mod user_message;
pub mod visual_truncate;

pub use assistant_message::{
    segment_streaming_markdown, AssistantMessageComponent, ThinkingDisplay,
};
pub use user_message::UserMessageComponent;

/// JavaScript's `\s` (and `String.prototype.trim`) whitespace.
pub(crate) fn is_js_space(c: char) -> bool {
    cortexcode_tui_components::markdown::js_trim(c.encode_utf8(&mut [0; 4])).is_empty()
}

/// Whether `s` is empty after `String.prototype.trim`.
pub(crate) fn is_blank(s: &str) -> bool {
    cortexcode_tui_components::markdown::js_trim(s).is_empty()
}

/// OSC 133 semantic-prompt zone markers the message blocks carry.
pub const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
pub const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
pub const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

/// Wrap rendered lines in an OSC 133 zone (`A` on the first line, `B` and
/// `C` in front of the last).
pub(crate) fn wrap_zone(mut lines: Vec<String>) -> Vec<String> {
    if lines.is_empty() {
        return lines;
    }
    lines[0].insert_str(0, OSC133_ZONE_START);
    let last = lines.len() - 1;
    lines[last].insert_str(0, &format!("{OSC133_ZONE_END}{OSC133_ZONE_FINAL}"));
    lines
}
