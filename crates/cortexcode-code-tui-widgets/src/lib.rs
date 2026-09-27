//! The coding agent's chat transcript widgets, ported from hoocode's
//! `modes/interactive/components/`.

mod assistant_message;
mod user_message;

pub use assistant_message::{
    segment_streaming_markdown, AssistantMessageComponent, ThinkingDisplay,
};
pub use user_message::UserMessageComponent;

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
