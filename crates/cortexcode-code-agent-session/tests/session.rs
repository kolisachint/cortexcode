//! Ports the model/thinking cases of `test/suite/agent-session-model-extension.test.ts`,
//! `test/agent-session-stats.test.ts`, the SDK-tool cases of
//! `test/agent-session-dynamic-tools.test.ts`, plus registry, session-info and
//! persistence checks.

mod common;

use std::sync::Arc;

use common::{text_result, tool, Harness, HarnessOptions};
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_provider_faux::FauxModelDefinition;
use cortexcode_ai_types::{
    AssistantMessage, Content, Cost, StopReason, ThinkingLevel, Usage, UserMessage,
};
use cortexcode_code_agent_session::{
    stats::sum_assistant_usage, AgentSessionEvent, CycleDirection, PromptOptions, ScopedModel,
    ToolSource, TranscriptSelection,
};
use cortexcode_code_session::FileEntry;

fn two_models(second_reasoning: bool) -> Vec<FauxModelDefinition> {
    vec![
        FauxModelDefinition {
            id: "faux-1".into(),
            name: Some("One".into()),
            reasoning: Some(true),
            ..Default::default()
        },
        FauxModelDefinition {
            id: "faux-2".into(),
            name: Some("Two".into()),
            reasoning: Some(second_reasoning),
            ..Default::default()
        },
    ]
}

#[test]
fn set_model_saves_the_model_to_the_session_and_settings() {
    let h = Harness::new(HarnessOptions {
        models: two_models(true),
        ..Default::default()
    });
    let next = h.faux.get_model_by_id("faux-2").unwrap();
    h.session.set_model(next.clone()).unwrap();
    assert_eq!(h.session.model().unwrap().id, "faux-2");
    let changes: Vec<String> = h
        .session
        .session_manager()
        .entries()
        .iter()
        .filter_map(|e| match e {
            FileEntry::ModelChange {
                provider, model_id, ..
            } => Some(format!("{provider}/{model_id}")),
            _ => None,
        })
        .collect();
    assert_eq!(changes, [format!("{}/faux-2", next.provider)]);
    let settings = h.session.settings();
    assert_eq!(settings.default_provider(), Some(next.provider.clone()));
    assert_eq!(settings.default_model().as_deref(), Some("faux-2"));
}

#[test]
fn cycles_through_scoped_models_and_preserves_the_scoped_thinking_preference() {
    let h = Harness::new(HarnessOptions {
        models: two_models(false),
        ..Default::default()
    });
    let one = h.faux.get_model_by_id("faux-1").unwrap();
    let two = h.faux.get_model_by_id("faux-2").unwrap();
    h.session.set_scoped_models(vec![
        ScopedModel {
            model: one,
            thinking_level: Some(ThinkingLevel::High),
        },
        ScopedModel {
            model: two,
            thinking_level: None,
        },
    ]);
    h.session.set_thinking_level(ThinkingLevel::High);

    let result = h.session.cycle_model(CycleDirection::Forward).unwrap();
    assert!(result.is_scoped);
    assert_eq!(h.session.model().unwrap().id, "faux-2");
    assert_eq!(h.session.thinking_level(), ThinkingLevel::Off);

    h.session.cycle_model(CycleDirection::Forward).unwrap();
    assert_eq!(h.session.model().unwrap().id, "faux-1");
    assert_eq!(h.session.thinking_level(), ThinkingLevel::High);

    // Backward wraps the other way.
    h.session.cycle_model(CycleDirection::Backward).unwrap();
    assert_eq!(h.session.model().unwrap().id, "faux-2");
}

#[test]
fn clamps_thinking_levels_to_model_capabilities() {
    let h = Harness::new(HarnessOptions {
        models: vec![FauxModelDefinition {
            id: "faux-1".into(),
            reasoning: Some(false),
            ..Default::default()
        }],
        ..Default::default()
    });
    h.session.set_thinking_level(ThinkingLevel::High);
    assert_eq!(h.session.thinking_level(), ThinkingLevel::Off);
    assert_eq!(
        h.session.cycle_thinking_level(CycleDirection::Forward),
        None
    );
}

#[test]
fn cycles_thinking_levels_and_persists_only_changes() {
    let h = Harness::new(HarnessOptions {
        models: two_models(true),
        ..Default::default()
    });
    let levels = h.session.get_available_thinking_levels();
    assert_eq!(levels.first(), Some(&ThinkingLevel::Off));
    let next = h
        .session
        .cycle_thinking_level(CycleDirection::Forward)
        .unwrap();
    assert_eq!(next, levels[1]);
    let back = h
        .session
        .cycle_thinking_level(CycleDirection::Backward)
        .unwrap();
    assert_eq!(back, ThinkingLevel::Off);
    let wrapped = h
        .session
        .cycle_thinking_level(CycleDirection::Backward)
        .unwrap();
    assert_eq!(&wrapped, levels.last().unwrap());
    // Setting the current level again writes nothing.
    h.session.set_thinking_level(wrapped.clone());
    let changes = h
        .session
        .session_manager()
        .entries()
        .iter()
        .filter(|e| matches!(e, FileEntry::ThinkingLevelChange { .. }))
        .count();
    assert_eq!(changes, 3);
    let events = h
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, AgentSessionEvent::ThinkingLevelChanged { .. }))
        .count();
    assert_eq!(events, 3);
}

#[test]
fn set_model_requires_configured_auth() {
    let h = Harness::new(HarnessOptions {
        models: two_models(true),
        without_auth: true,
        ..Default::default()
    });
    let provider = h.faux.get_model().provider;
    let error = h
        .session
        .set_model(h.faux.get_model_by_id("faux-2").unwrap())
        .unwrap_err();
    assert_eq!(error.0, format!("No API key for {provider}/faux-2"));
}

fn snippet_tool(name: &str, snippet: Option<&str>) -> cortexcode_code_tool_api::ToolDefinition {
    let mut t = tool(name, |_| text_result("ok"));
    t.description = format!("{name} description");
    t.prompt_snippet = snippet.map(String::from);
    t.prompt_guidelines = vec![format!("  Use {name} well.  "), format!("Use {name} well.")];
    t
}

#[test]
fn sdk_tools_carry_source_metadata_and_are_active() {
    let h = Harness::new(HarnessOptions {
        tools: vec![snippet_tool("builtin_tool", Some("A  built-in\n tool"))],
        custom_tools: vec![snippet_tool("sdk_tool", None)],
        ..Default::default()
    });
    let tools = h.session.get_all_tools();
    let sdk = tools.iter().find(|t| t.name == "sdk_tool").unwrap();
    assert_eq!(sdk.source, ToolSource::Sdk);
    assert_eq!(sdk.source_path, "<sdk:sdk_tool>");
    let builtin = tools.iter().find(|t| t.name == "builtin_tool").unwrap();
    assert_eq!(builtin.source_path, "<builtin:builtin_tool>");
    assert_eq!(
        h.session.get_active_tool_names(),
        ["builtin_tool", "sdk_tool"]
    );

    // Snippets are normalized to one line; a tool without one stays out of
    // the prompt's tool list; guidelines are trimmed and deduplicated.
    let prompt = h.session.system_prompt();
    assert!(
        prompt.contains("- builtin_tool: A built-in tool"),
        "{prompt}"
    );
    assert!(!prompt.contains("sdk_tool description"));
    assert_eq!(prompt.matches("Use builtin_tool well.").count(), 1);
}

#[test]
fn allow_and_deny_lists_filter_the_registry() {
    let h = Harness::new(HarnessOptions {
        tools: vec![
            snippet_tool("a", Some("A")),
            snippet_tool("b", Some("B")),
            snippet_tool("c", Some("C")),
        ],
        allowed_tool_names: Some(vec!["a".into(), "b".into()]),
        disallowed_tool_names: Some(vec!["b".into()]),
        initial_active_tool_names: Some(vec![]),
        ..Default::default()
    });
    assert_eq!(h.session.get_active_tool_names(), ["a"]);
    let all: Vec<String> = h
        .session
        .get_all_tools()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(all, ["a"]);
}

#[test]
fn set_active_tools_ignores_unknown_names_and_rebuilds_the_prompt() {
    let h = Harness::new(HarnessOptions {
        tools: vec![
            snippet_tool("a", Some("Tool A")),
            snippet_tool("b", Some("Tool B")),
        ],
        ..Default::default()
    });
    h.session
        .set_active_tools_by_name(&["b".into(), "missing".into()]);
    assert_eq!(h.session.get_active_tool_names(), ["b"]);
    let prompt = h.session.system_prompt();
    assert!(prompt.contains("- b: Tool B"));
    assert!(!prompt.contains("- a: Tool A"));
    assert!(h.session.get_tool_definition("a").is_some());
}

#[test]
fn session_name_and_color_emit_session_info_changes() {
    let h = Harness::new(HarnessOptions::default());
    let slug = h.session.display_name();
    h.session.set_session_name("  My Session ");
    assert_eq!(h.session.session_name().as_deref(), Some("My Session"));
    assert_eq!(h.session.display_name(), "My Session");
    h.session.set_session_color(5);
    assert_eq!(h.session.session_color_slot(), 5);
    assert_eq!(h.session.session_name().as_deref(), Some("My Session"));
    let names: Vec<Option<String>> = h
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::SessionInfoChanged { name } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        [Some("My Session".into()), Some("My Session".into())]
    );
    assert_ne!(slug, "My Session");
}

// ---------------------------------------------------------------------------
// agent-session-stats.test.ts
// ---------------------------------------------------------------------------

fn usage(total: u64) -> Usage {
    Usage {
        input: total,
        total_tokens: total,
        ..Default::default()
    }
}

fn assistant(text: &str, total: u64, timestamp: i64) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![Content::text(text)],
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-5".into(),
        usage: usage(total),
        stop_reason: StopReason::Stop,
        timestamp,
        ..Default::default()
    })
}

fn user(text: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: cortexcode_ai_types::UserContent::Text(text.into()),
        timestamp,
    })
}

fn stats_harness() -> Harness {
    let h = Harness::new(HarnessOptions::default());
    let model = cortexcode_ai_models::get_model("anthropic", "claude-sonnet-4-5")
        .unwrap()
        .clone();
    h.session.agent().set_model(model);
    h
}

fn sync_agent_messages(h: &Harness) {
    let messages = h.session.session_manager().build_context().messages;
    h.session.agent().set_messages(messages);
}

#[test]
fn exposes_the_current_context_usage_alongside_token_totals() {
    let h = stats_harness();
    {
        let mut sm = h.session.session_manager();
        sm.append_message(user("hello", 1));
        sm.append_message(assistant("hi", 200, 2));
    }
    sync_agent_messages(&h);
    let stats = h.session.get_session_stats();
    let window = h.session.model().unwrap().context_window;
    assert_eq!(stats.context_usage, h.session.get_context_usage());
    let usage = stats.context_usage.unwrap();
    assert_eq!(usage.tokens, Some(200));
    assert_eq!(usage.context_window, window);
    assert_eq!(usage.percent, Some(200.0 / window as f64 * 100.0));
    assert_eq!(stats.user_messages, 1);
    assert_eq!(stats.assistant_messages, 1);
    assert_eq!(stats.total_messages, 2);
}

#[test]
fn reports_unknown_context_usage_immediately_after_compaction() {
    let h = stats_harness();
    {
        let mut sm = h.session.session_manager();
        sm.append_message(user("first", 1));
        sm.append_message(assistant("response1", 180_000, 2));
        let kept = sm.append_message(user("second", 3));
        sm.append_message(assistant("response2", 195_000, 4));
        sm.append_compaction("summary", kept, 195_000, None, None, None);
        sm.append_message(user("third", 5));
    }
    sync_agent_messages(&h);
    let stats = h.session.get_session_stats();
    assert_eq!(stats.tokens.input, 195_000);
    let usage = stats.context_usage.unwrap();
    assert_eq!(usage.tokens, None);
    assert_eq!(usage.percent, None);
}

#[test]
fn uses_post_compaction_usage_instead_of_stale_kept_usage() {
    let h = stats_harness();
    {
        let mut sm = h.session.session_manager();
        sm.append_message(user("first", 1));
        sm.append_message(assistant("response1", 180_000, 2));
        let kept = sm.append_message(user("second", 3));
        sm.append_message(assistant("response2", 195_000, 4));
        sm.append_compaction("summary", kept, 195_000, None, None, None);
        sm.append_message(user("third", 5));
        sm.append_message(assistant("response3", 25_000, 6));
    }
    sync_agent_messages(&h);
    let stats = h.session.get_session_stats();
    let window = h.session.model().unwrap().context_window;
    assert_eq!(stats.tokens.input, 220_000);
    let usage = stats.context_usage.unwrap();
    assert_eq!(usage.tokens, Some(25_000));
    assert_eq!(usage.percent, Some(25_000.0 / window as f64 * 100.0));
}

#[test]
fn sum_assistant_usage_sums_only_assistant_messages() {
    let h = stats_harness();
    let mut sm = h.session.session_manager();
    sm.append_message(user("hello", 1));
    sm.append_message(assistant("hi", 200, 2));
    sm.append_message(user("more", 3));
    sm.append_message(assistant("sure", 300, 4));
    let totals = sum_assistant_usage(sm.entries());
    assert_eq!(totals.input, 500);
    assert_eq!(totals.output, 0);
    assert_eq!(totals.cache_read, 0);
    assert_eq!(totals.cache_write, 0);
    assert_eq!(totals.cost, 0.0);
}

#[test]
fn sum_assistant_usage_carries_every_field() {
    let h = stats_harness();
    let mut sm = h.session.session_manager();
    let with_usage = |sm: &mut cortexcode_code_session::SessionManager, u: Usage, ts: i64| {
        let AgentMessage::Assistant(mut message) = assistant("x", 0, ts) else {
            unreachable!()
        };
        message.usage = u;
        sm.append_message(AgentMessage::Assistant(message));
    };
    with_usage(
        &mut sm,
        Usage {
            input: 1000,
            output: 200,
            cache_read: 50,
            cache_write: 10,
            cost: Cost {
                total: 0.01,
                ..Default::default()
            },
            ..Default::default()
        },
        1,
    );
    let anchor = sum_assistant_usage(sm.entries());
    with_usage(
        &mut sm,
        Usage {
            input: 3000,
            output: 450,
            cost: Cost {
                total: 0.015,
                ..Default::default()
            },
            ..Default::default()
        },
        2,
    );
    let after = sum_assistant_usage(sm.entries());
    assert_eq!(after.input - anchor.input, 3000);
    assert_eq!(after.output - anchor.output, 450);
    assert!((after.cost - anchor.cost - 0.015).abs() < 1e-6);
    assert_eq!(anchor.input, 1000);
    assert_eq!(anchor.cache_read, 50);
    assert_eq!(anchor.cache_write, 10);
}

#[tokio::test(flavor = "multi_thread")]
async fn transcript_helpers_and_forkable_messages() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![
        cortexcode_ai_provider_faux::faux_assistant_message("first answer", Default::default())
            .into(),
        cortexcode_ai_provider_faux::faux_assistant_message("second answer", Default::default())
            .into(),
    ]);
    h.session
        .prompt("one", PromptOptions::default())
        .await
        .unwrap();
    h.session
        .prompt("two", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(
        h.session.get_last_assistant_text().as_deref(),
        Some("second answer")
    );
    assert_eq!(
        h.session.get_transcript_markdown(&TranscriptSelection {
            turns: Some(1),
            ..Default::default()
        }),
        "## You\n\ntwo\n\n## Agent\n\nsecond answer"
    );
    let forkable: Vec<String> = h
        .session
        .get_user_messages_for_forking()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(forkable, ["one", "two"]);

    // JSONL export: header, then the branch re-chained into a line.
    let out = h.temp_dir.path().join("out/export.jsonl");
    let written = h.session.export_to_jsonl(Some(&out)).unwrap();
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(written)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["type"], "session");
    assert_eq!(lines[0]["id"], h.session.session_id());
    assert_eq!(lines.len(), 5);
    assert!(lines[1]["parentId"].is_null());
    assert_eq!(lines[2]["parentId"], lines[1]["id"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_see_the_session_branch_and_model_through_their_context() {
    let seen = Arc::new(std::sync::Mutex::new((0usize, String::new())));
    let sink = seen.clone();
    let mut probe = tool("probe", |_| text_result("unused"));
    probe.execute = Arc::new(move |_id, _params, _signal, _update, ctx| {
        let ctx = ctx.expect("context");
        let branch = ctx.session_manager.as_ref().unwrap().get_branch();
        *sink.lock().unwrap() = (branch.len(), ctx.model.as_ref().unwrap().id.clone());
        Ok(text_result("probed"))
    });
    let h = Harness::new(HarnessOptions {
        tools: vec![probe],
        ..Default::default()
    });
    h.set_responses(vec![
        cortexcode_ai_provider_faux::faux_assistant_message(
            cortexcode_ai_provider_faux::faux_tool_call("probe", serde_json::json!({}), None),
            cortexcode_ai_provider_faux::FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..Default::default()
            },
        )
        .into(),
        cortexcode_ai_provider_faux::faux_assistant_message("done", Default::default()).into(),
    ]);
    h.session
        .prompt("go", PromptOptions::default())
        .await
        .unwrap();
    let (branch_len, model) = seen.lock().unwrap().clone();
    // user + assistant(tool call) are persisted before the tool runs.
    assert_eq!(branch_len, 2);
    assert_eq!(model, h.faux.get_model().id);
    // Every message ended up in the session, in order.
    assert_eq!(h.entry_types(), ["message"; 4]);
}
