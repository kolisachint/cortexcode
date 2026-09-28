//! Port of the pin's `test/user-message.test.ts`.

mod support;

use cortexcode_code_tui_widgets::{
    UserMessageComponent, OSC133_ZONE_END, OSC133_ZONE_FINAL, OSC133_ZONE_START,
};
use cortexcode_tui_render::Component;
use support::lock;

const BG_RESET: &str = "\x1b[49m";

#[test]
fn keeps_user_message_height_stable_while_moving_closing_osc_markers_off_line_end() {
    let _g = lock();
    let lines = UserMessageComponent::new("hello").render(20);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains(OSC133_ZONE_START));
    assert!(lines[0].ends_with(BG_RESET), "{:?}", lines[0]);
    assert!(!lines[0].contains(OSC133_ZONE_END));
    assert!(lines[1].contains("hello"));
    assert!(lines[2].starts_with(&format!("{OSC133_ZONE_END}{OSC133_ZONE_FINAL}")));
    assert!(lines[2].ends_with(BG_RESET));
}
