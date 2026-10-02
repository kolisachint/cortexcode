//! Port of the pin's `coding-agent/test/suite/disabled-tools.test.ts`, on the
//! real built-in tools.

use crate::common::{Harness, HarnessOptions};

fn session(tools: Option<&[&str]>, disallowed: &[&str]) -> Harness {
    let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Harness::new(HarnessOptions {
        real_builtin_tools: true,
        allowed_tool_names: tools.map(names),
        disallowed_tool_names: Some(names(disallowed)),
        ..Default::default()
    })
}

fn all_tools(h: &Harness) -> Vec<String> {
    h.session
        .get_all_tools()
        .into_iter()
        .map(|t| t.name)
        .collect()
}

#[test]
fn removes_a_disabled_tool_from_the_registry() {
    let h = session(None, &["bash"]);
    let names = all_tools(&h);
    assert!(names.contains(&"read".to_string()));
    assert!(!names.contains(&"bash".to_string()));
    assert!(!h
        .session
        .get_active_tool_names()
        .contains(&"bash".to_string()));
}

#[test]
fn keeps_a_tool_disabled_even_when_it_is_in_the_allowlist() {
    let h = session(Some(&["read", "bash"]), &["bash"]);
    assert_eq!(all_tools(&h), ["read"]);
    assert_eq!(h.session.get_active_tool_names(), ["read"]);
}
