//! Ports of hoocode `packages/agent/test/harness/resource-formatting.test.ts`
//! and `system-prompt.test.ts` (v0.5.89), plus the message helpers.

use cortexcode_agent_harness::{
    create_background_placeholder_text, create_background_task_message,
    create_branch_summary_message, create_compaction_summary_message, describe_background_tool,
    format_prompt_template_invocation, format_skill_invocation, format_skills_for_system_prompt,
    parse_command_args, substitute_args, summarize_args, PromptTemplate, Skill,
};
use cortexcode_agent_types::{AgentToolCall, AgentToolResult, BackgroundToolResult};
use cortexcode_ai_types::{Content, UserContent};
use serde_json::json;

fn skill(name: &str, description: &str, file_path: &str, disabled: bool) -> Skill {
    Skill {
        name: name.into(),
        description: description.into(),
        content: format!("{name} content"),
        file_path: file_path.into(),
        disable_model_invocation: disabled.then_some(true),
    }
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

// --- resource-formatting.test.ts ---

#[test]
fn formats_skill_invocations_with_additional_instructions() {
    let skill = Skill {
        name: "inspect".into(),
        description: "Inspect things".into(),
        content: "Use inspection tools.".into(),
        file_path: "/project/.hoocode/skills/inspect/SKILL.md".into(),
        disable_model_invocation: None,
    };
    assert_eq!(
        format_skill_invocation(&skill, Some("Check errors.")),
        "<skill name=\"inspect\" location=\"/project/.hoocode/skills/inspect/SKILL.md\">\nReferences are relative to /project/.hoocode/skills/inspect.\n\nUse inspection tools.\n</skill>\n\nCheck errors."
    );
}

#[test]
fn formats_prompt_template_invocations_with_positional_arguments() {
    let template = PromptTemplate {
        name: "review".into(),
        description: None,
        content: "Review $1 with $ARGUMENTS".into(),
    };
    assert_eq!(
        format_prompt_template_invocation(&template, &args(&["a.ts", "care"])),
        "Review a.ts with a.ts care"
    );
}

// --- system-prompt.test.ts ---

#[test]
fn formats_visible_skills_in_order_and_skips_model_disabled_skills() {
    let skills = [
        skill(
            "visible",
            "Use <this> & that",
            "/skills/visible/SKILL.md",
            false,
        ),
        skill("hidden", "Hidden", "/skills/hidden/SKILL.md", true),
        skill("second", "Second skill", "/skills/second/SKILL.md", false),
    ];
    assert_eq!(
        format_skills_for_system_prompt(&skills),
        "The following skills provide specialized instructions for specific tasks.
Read the full skill file when the task matches its description.
When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.

<available_skills>
  <skill>
    <name>visible</name>
    <description>Use &lt;this&gt; &amp; that</description>
    <location>/skills/visible/SKILL.md</location>
  </skill>
  <skill>
    <name>second</name>
    <description>Second skill</description>
    <location>/skills/second/SKILL.md</location>
  </skill>
</available_skills>"
    );
}

#[test]
fn returns_an_empty_string_when_no_skills_are_model_visible() {
    let skills = [skill("hidden", "Hidden", "/skills/hidden/SKILL.md", true)];
    assert_eq!(format_skills_for_system_prompt(&skills), "");
}

#[test]
fn escapes_xml_in_all_model_visible_skill_fields() {
    let skills = [skill(
        "a&b",
        "Quote \"double\" and 'single'",
        "/skills/<bad>&\"quote\"/SKILL.md",
        false,
    )];
    assert!(format_skills_for_system_prompt(&skills).contains(
        "<name>a&amp;b</name>\n    <description>Quote &quot;double&quot; and &apos;single&apos;</description>\n    <location>/skills/&lt;bad&gt;&amp;&quot;quote&quot;/SKILL.md</location>"
    ));
}

// --- prompt-templates.ts argument handling ---

#[test]
fn parses_quoted_command_arguments() {
    assert_eq!(
        parse_command_args(r#"one "two words"	'three "x"'  four"#),
        ["one", "two words", r#"three "x""#, "four"]
    );
    assert!(parse_command_args("   ").is_empty());
}

#[test]
fn substitutes_positional_slice_and_all_arguments() {
    let a = args(&["a", "b", "c"]);
    assert_eq!(substitute_args("$1 $3 $4 $0", &a), "a c  ");
    assert_eq!(substitute_args("${@:2}", &a), "b c");
    assert_eq!(substitute_args("${@:2:1}|${@:0:2}|${@:9}", &a), "b|a b|");
    assert_eq!(substitute_args("$@ / $ARGUMENTS", &a), "a b c / a b c");
    // `String.replace` expands `$&` in the replacement text.
    assert_eq!(
        substitute_args("[$ARGUMENTS]", &args(&["$&"])),
        "[$ARGUMENTS]"
    );
    assert_eq!(substitute_args("[$@]", &args(&["$$"])), "[$]");
}

// --- messages.ts helpers ---

#[test]
fn summary_messages_take_iso_timestamps() {
    let branch = create_branch_summary_message("s", "e1", "2026-01-02T03:04:05.678Z");
    assert_eq!(branch.timestamp, 1767323045678);
    let compaction =
        create_compaction_summary_message("s", 10, "2026-01-02T03:04:05.678Z", Some(4));
    assert_eq!(
        (compaction.tokens_before, compaction.tokens_after),
        (10, Some(4))
    );
}

fn call(name: &str, arguments: serde_json::Value) -> AgentToolCall {
    AgentToolCall {
        id: "c1".into(),
        name: name.into(),
        arguments,
    }
}

#[test]
fn describes_background_tools_for_mcp_and_subagents() {
    let mcp = describe_background_tool(&call(
        "mcp_github_search",
        json!({"query": "fix bug\nsecond line", "limit": 5, "empty": "", "none": null, "x": 1, "y": 2}),
    ));
    assert!(mcp.is_mcp_tool);
    assert_eq!(mcp.label, "MCP tool `github_search`");
    assert_eq!(
        mcp.summary.as_deref(),
        Some("query: fix bug, limit: 5, x: 1")
    );

    let task = describe_background_tool(&call(
        "Task",
        json!({"subagent_type": "explorer", "prompt": "\n  Find the parser  \nmore"}),
    ));
    assert_eq!(task.subagent_type, "explorer");
    assert_eq!(task.label, "subagent `explorer`");
    assert_eq!(task.summary.as_deref(), Some("Find the parser"));

    let described = describe_background_tool(&call("Task", json!({"description": "  Audit  "})));
    assert_eq!(described.subagent_type, "Task");
    assert_eq!(described.summary.as_deref(), Some("Audit"));

    let long = "x".repeat(200);
    assert_eq!(
        summarize_args(&json!({ "k": long })).unwrap(),
        format!("k: {}…", "x".repeat(47))
    );
}

#[test]
fn background_placeholders_and_finish_messages() {
    assert_eq!(
        create_background_placeholder_text(&call("mcp_s_t", json!({"q": "x"}))),
        "Started MCP tool `s_t` in the background — q: x. Its result arrives as a follow-up; keep working."
    );
    assert_eq!(
        create_background_placeholder_text(&call("Task", json!({"subagent_type": "a"}))),
        "Delegated to subagent `a` in the background. I'll be notified when it finishes; use TaskOutput to check progress or read the result."
    );

    let result = |name: &str, is_error: bool| BackgroundToolResult {
        tool_call: call(name, json!({"q": "x"})),
        result: AgentToolResult {
            content: vec![Content::text("body")],
            details: json!(null),
            terminate: false,
        },
        is_error,
    };
    let mcp = create_background_task_message(&result("mcp_s_t", true));
    assert_eq!(mcp.custom_type, "backgroundTask");
    assert_eq!(
        mcp.content,
        UserContent::Blocks(vec![
            Content::text("Background MCP tool `s_t` (q: x) failed:"),
            Content::text("body")
        ])
    );
    assert_eq!(
        mcp.details,
        Some(json!({"subagentType": "mcp_s_t", "isMcpTool": true, "isError": true}))
    );
    let task = create_background_task_message(&result("Task", false));
    assert_eq!(
        task.content,
        UserContent::Blocks(vec![Content::text("body")])
    );
    assert!(task.display);
}
