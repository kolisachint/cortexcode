//! Port of the pin's `coding-agent/test/suite/subagent-footer-indicator.test.ts`:
//! the footer's subagent indicator follows whether the tool named `Task` is
//! active (interactive_mode wires `set_subagent_enabled` from
//! `get_active_tool_names()`, mirrored here).

use cortexcode_code_tui_app::footer_data::FooterDataProvider;

#[test]
fn the_task_tool_definition_is_named_task() {
    let def = cortexcode_code_subagents::tools::create_task_tool_definition(&std::env::temp_dir());
    assert_eq!(def.name, "Task");
}

fn footer_for(active_tools: &[&str]) -> FooterDataProvider {
    let provider = FooterDataProvider::new(std::env::temp_dir());
    provider.set_subagent_enabled(active_tools.contains(&"Task"));
    provider
}

#[test]
fn lights_up_the_footer_when_the_task_tool_is_active() {
    let p = footer_for(&["read", "Task"]);
    assert!(p.get_subagent_enabled());
    assert_eq!(p.get_active_mode(), "build");
    p.dispose();
}

#[test]
fn leaves_the_footer_untouched_when_the_task_tool_is_absent() {
    let p = footer_for(&["read", "bash"]);
    assert!(!p.get_subagent_enabled());
    assert_eq!(p.get_active_mode(), "build");
    p.dispose();
}
