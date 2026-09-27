//! Port of hoocode `packages/coding-agent/test/agent-frontmatter.test.ts` (v0.5.89).

use cortexcode_code_resources::{
    normalize_model, normalize_tools, parse_agent_definition, AgentSource, CLAUDE_TOOL_ALIASES,
    HOOCODE_TOOL_NAMES, MODEL_INHERIT,
};
use serde_json::{json, Value};

fn tools(value: Value) -> (Vec<String>, Vec<String>) {
    let (tools, diagnostics) = normalize_tools(&value, None);
    (tools, diagnostics.into_iter().map(|d| d.message).collect())
}

// normalizeTools (D7 Claude Code shim)

#[test]
fn maps_claude_tool_names_case_insensitive() {
    let (t, d) = tools(json!("Read, Grep, Bash"));
    assert_eq!(t, ["read", "SearchCodebase", "bash"]);
    assert!(d.is_empty());
}

#[test]
fn accepts_a_yaml_list_but_emits_a_format_warning() {
    let (value, diagnostics) = normalize_tools(&json!(["Read", "SearchCodebase"]), None);
    assert_eq!(value, ["read", "SearchCodebase"]);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].kind,
        cortexcode_code_resources::DiagnosticType::Warning
    );
    assert!(diagnostics[0].message.contains("comma-separated string"));
}

#[test]
fn drops_unknown_tools_with_a_diagnostic() {
    let (t, d) = tools(json!("Read, NotebookEdit, Task, MultiEdit"));
    assert_eq!(t, ["read"]);
    assert_eq!(d.len(), 3);
    assert!(d.iter().any(|m| m.contains("NotebookEdit")));
}

#[test]
fn maps_claude_webfetch_websearch_to_the_opt_in_web_tools() {
    let (t, d) = tools(json!("WebFetch, WebSearch"));
    assert_eq!(t, ["webfetch", "websearch"]);
    assert!(d.is_empty());
}

#[test]
fn dedupes_resolved_tools() {
    assert_eq!(tools(json!("Grep, Glob, find")).0, ["SearchCodebase"]);
}

#[test]
fn drops_the_pre_rename_search_name() {
    let (t, d) = tools(json!("read, search"));
    assert_eq!(t, ["read"]);
    assert!(d.iter().any(|m| m.contains("tool \"search\"")));
}

#[test]
fn drops_ls_which_has_no_counterpart() {
    let (t, d) = tools(json!("Read, LS"));
    assert_eq!(t, ["read"]);
    assert!(d.iter().any(|m| m.contains("LS")));
}

#[test]
fn alias_map_only_targets_known_hoocode_tools() {
    for (_, target) in CLAUDE_TOOL_ALIASES {
        assert!(HOOCODE_TOOL_NAMES.contains(target));
    }
}

// normalizeModel

#[test]
fn preserves_the_inherit_sentinel() {
    assert_eq!(
        normalize_model(Some(&json!("inherit"))).as_deref(),
        Some(MODEL_INHERIT)
    );
}

#[test]
fn passes_through_aliases_and_trims() {
    assert_eq!(
        normalize_model(Some(&json!("  sonnet "))).as_deref(),
        Some("sonnet")
    );
}

#[test]
fn returns_undefined_for_empty_or_missing() {
    assert_eq!(normalize_model(Some(&json!(""))), None);
    assert_eq!(normalize_model(None), None);
}

// parseAgentDefinition

fn parse(
    raw: &str,
    source: AgentSource,
) -> (
    Option<cortexcode_code_resources::AgentDefinition>,
    Vec<String>,
) {
    let (agent, diagnostics) = parse_agent_definition(raw, source, None, None);
    (agent, diagnostics.into_iter().map(|d| d.message).collect())
}

#[test]
fn parses_a_claude_code_style_agent_natively() {
    let raw = "---\nname: explorer\ndescription: Use this agent to explore the codebase read-only.\ntools: Read, Grep, Glob, Bash\nmodel: sonnet\n---\nYou are a read-only explorer.";
    let (agent, d) = parse(raw, AgentSource::ClaudeProject);
    assert!(d.is_empty());
    let agent = agent.unwrap();
    assert_eq!(agent.name, "explorer");
    assert_eq!(agent.tools.unwrap(), ["read", "SearchCodebase", "bash"]);
    assert_eq!(agent.model.as_deref(), Some("sonnet"));
    assert_eq!(agent.prompt, "You are a read-only explorer.");
    assert_eq!(agent.source, AgentSource::ClaudeProject);
}

#[test]
fn omitted_tools_means_inherit_all() {
    let raw =
        "---\nname: agent-a\ndescription: An agent that inherits all parent tools.\n---\nbody";
    assert_eq!(parse(raw, AgentSource::Project).0.unwrap().tools, None);
}

#[test]
fn falls_back_to_fallback_name_when_name_is_omitted() {
    let raw = "---\ndescription: Description long enough to be valid.\n---\nbody";
    let (agent, _) = parse_agent_definition(raw, AgentSource::Builtin, None, Some("explore"));
    assert_eq!(agent.unwrap().name, "explore");
}

#[test]
fn returns_null_when_description_is_missing() {
    let (agent, d) = parse("---\nname: no-desc\n---\nbody", AgentSource::Project);
    assert!(agent.is_none());
    assert!(d.iter().any(|m| m.contains("description is required")));
}

#[test]
fn returns_null_for_an_invalid_name() {
    let raw = "---\nname: Bad_Name\ndescription: Description long enough to be valid.\n---\nbody";
    let (agent, d) = parse(raw, AgentSource::Project);
    assert!(agent.is_none());
    assert!(d.iter().any(|m| m.contains("invalid characters")));
}

#[test]
fn captures_a_disallowed_tools_denylist() {
    let raw = "---\nname: limited\ndescription: An agent with a denied tool.\ntools: read, SearchCodebase, bash\ndisallowedTools: bash\n---\nbody";
    let agent = parse(raw, AgentSource::Project).0.unwrap();
    assert!(agent.tools.unwrap().contains(&"bash".to_string()));
    assert_eq!(agent.disallowed_tools.unwrap(), ["bash"]);
}

#[test]
fn captures_the_max_turns_extension() {
    let raw = "---\nname: capped\ndescription: An agent with a turn cap.\nmaxTurns: 12\n---\nbody";
    assert_eq!(
        parse(raw, AgentSource::Project).0.unwrap().max_turns,
        Some(12)
    );
}

#[test]
fn captures_the_background_flag_and_leaves_it_undefined_when_absent() {
    let bg = "---\nname: watcher\ndescription: A non-blocking background agent.\nbackground: true\n---\nbody";
    assert_eq!(
        parse(bg, AgentSource::Project).0.unwrap().background,
        Some(true)
    );
    let plain = "---\nname: plain\ndescription: A normal foreground agent.\n---\nbody";
    assert_eq!(
        parse(plain, AgentSource::Project).0.unwrap().background,
        None
    );
}

#[test]
fn warns_when_background_is_not_a_boolean() {
    let raw = "---\nname: bad-bg\ndescription: Agent with invalid background.\nbackground: yes\n---\nbody";
    let (agent, d) = parse(raw, AgentSource::Project);
    assert_eq!(agent.unwrap().background, None);
    assert!(d.iter().any(|m| m.contains("background must be a boolean")));
}

#[test]
fn captures_the_delegate_flag_and_leaves_it_undefined_when_absent() {
    let raw = "---\nname: orchestrator\ndescription: An agent that delegates to other subagents.\ndelegate: true\n---\nbody";
    let agent = parse(raw, AgentSource::Project).0.unwrap();
    assert_eq!(agent.delegate, Some(true));
    assert_eq!(agent.delegate_to, None);
    let plain = "---\nname: plain\ndescription: A normal agent.\n---\nbody";
    assert_eq!(parse(plain, AgentSource::Project).0.unwrap().delegate, None);
}

#[test]
fn captures_a_scoped_delegate_list() {
    let raw = "---\nname: scoped\ndescription: Delegates only to explore and plan.\ndelegate: explore, plan\n---\nbody";
    let agent = parse(raw, AgentSource::Project).0.unwrap();
    assert_eq!(agent.delegate, Some(true));
    assert_eq!(agent.delegate_to.unwrap(), ["explore", "plan"]);
}

#[test]
fn captures_the_fork_flag() {
    let raw = "---\nname: forker\ndescription: An agent that inherits the parent conversation.\nfork: true\n---\nbody";
    assert_eq!(parse(raw, AgentSource::Project).0.unwrap().fork, Some(true));
    let plain = "---\nname: plain\ndescription: A normal agent.\n---\nbody";
    assert_eq!(parse(plain, AgentSource::Project).0.unwrap().fork, None);
}

#[test]
fn warns_when_delegate_is_neither_a_boolean_nor_a_name_list() {
    let raw = "---\nname: bad-delegate\ndescription: Agent with invalid delegate.\ndelegate: 123\n---\nbody";
    let (agent, d) = parse(raw, AgentSource::Project);
    assert_eq!(agent.unwrap().delegate, None);
    assert!(d.iter().any(|m| m.contains("delegate must be")));
}

#[test]
fn warns_on_unknown_model_alias() {
    let raw = "---\nname: weird-model\ndescription: Agent using a non-standard model name.\nmodel: gpt-4o\n---\nbody";
    let (agent, d) = parse(raw, AgentSource::Project);
    assert_eq!(agent.unwrap().model.as_deref(), Some("gpt-4o"));
    assert!(d.iter().any(|m| m.contains("is not a recognized alias")));
}

#[test]
fn allows_full_claude_model_ids_without_warning() {
    let raw = "---\nname: full-id\ndescription: Agent using a full model ID.\nmodel: claude-sonnet-4-6\n---\nbody";
    let (_, d) = parse(raw, AgentSource::Project);
    assert!(d.iter().all(|m| !m.contains("is not a recognized alias")));
}

#[test]
fn warns_when_tools_is_a_yaml_list() {
    let raw = "---\nname: list-tools\ndescription: Agent declaring tools as a YAML list.\ntools:\n  - read\n  - bash\n---\nbody";
    let (agent, d) = parse(raw, AgentSource::Project);
    assert_eq!(agent.unwrap().tools.unwrap(), ["read", "bash"]);
    assert!(d.iter().any(|m| m.contains("comma-separated string")));
}
