//! Port of the pin's `test/bash-execution-width.test.ts`.

mod support;

use cortexcode_code_tui_widgets::bash_execution::BashExecutionComponent;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;
use support::lock;

#[test]
fn collapsed_preview_lines_respect_render_time_width_not_construction_time_width() {
    let _g = lock();
    let mut component = BashExecutionComponent::new("pwd", false);
    let long = "x".repeat(150);
    component.append_output(&format!("{long}\n{long}\n"));
    component.set_complete(Some(0), false, None, None);
    for (i, line) in component.render(80).iter().enumerate() {
        let w = visible_width(line);
        assert!(w <= 80, "Line {i} visibleWidth={w} > 80");
    }
}

#[test]
fn re_computes_lines_when_width_changes_between_renders() {
    let _g = lock();
    let mut component = BashExecutionComponent::new("echo hello", false);
    component.append_output(&format!("{}\n", "abcdefghij".repeat(20)));
    component.set_complete(Some(0), false, None, None);
    for line in component.render(200) {
        assert!(visible_width(&line) <= 200);
    }
    for (i, line) in component.render(60).iter().enumerate() {
        let w = visible_width(line);
        assert!(w <= 60, "Line {i} visibleWidth={w} > 60");
    }
}
