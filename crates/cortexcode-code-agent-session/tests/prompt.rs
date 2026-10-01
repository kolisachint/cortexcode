//! Ports `test/suite/agent-session-prompt.test.ts` and the non-extension cases
//! of `test/agent-session-concurrent.test.ts`.

mod common;

use std::sync::{Arc, Mutex};

use common::{message_text, text_result, tool, Harness, HarnessOptions, WaitTool};
use cortexcode_ai_provider_faux::{
    faux_assistant_message, faux_tool_call, FauxMessageOptions, FauxResponseStep,
};
use cortexcode_ai_types::{Content, ImageContent, Message, StopReason};
use cortexcode_code_agent_session::{
    CommandFuture, ExpandedInput, ExtensionHooks, PromptOptions, ResourceLoader, StreamingBehavior,
    TemplateKind,
};

fn text(t: &str) -> FauxResponseStep {
    faux_assistant_message(t, Default::default()).into()
}

fn tool_use(calls: Vec<Content>) -> FauxResponseStep {
    faux_assistant_message(
        calls,
        FauxMessageOptions {
            stop_reason: Some(StopReason::ToolUse),
            ..Default::default()
        },
    )
    .into()
}

#[tokio::test(flavor = "multi_thread")]
async fn prompts_while_idle_and_records_a_single_text_response() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("hello")]);
    h.session
        .prompt("hi", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(h.roles(), ["user", "assistant"]);
    assert_eq!(h.user_texts(), ["hi"]);
    assert_eq!(h.pending_response_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn handles_a_tool_call_turn_and_waits_for_the_follow_up_response() {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let seen = runs.clone();
    let echo = tool("echo", move |params| {
        let t = params["text"].as_str().unwrap_or_default().to_string();
        seen.lock().unwrap().push(t.clone());
        text_result(&format!("echo:{t}"))
    });
    let h = Harness::new(HarnessOptions {
        tools: vec![echo],
        ..Default::default()
    });
    h.set_responses(vec![
        tool_use(vec![faux_tool_call(
            "echo",
            serde_json::json!({"text": "hello"}),
            None,
        )]),
        text("done"),
    ]);
    h.session
        .prompt("start", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(*runs.lock().unwrap(), ["hello"]);
    assert_eq!(h.roles(), ["user", "assistant", "toolResult", "assistant"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn executes_multiple_tool_calls_and_continues_with_one_follow_up() {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let make = |name: &'static str, delay_ms: u64| {
        let seen = runs.clone();
        tool(name, move |params| {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let v = params["value"].as_str().unwrap_or_default();
            seen.lock().unwrap().push(format!("{name}:{v}"));
            text_result(&format!("{name}:{v}"))
        })
    };
    let h = Harness::new(HarnessOptions {
        tools: vec![make("slow", 25), make("fast", 0)],
        ..Default::default()
    });
    h.set_responses(vec![
        tool_use(vec![
            faux_tool_call("slow", serde_json::json!({"value": "a"}), None),
            faux_tool_call("fast", serde_json::json!({"value": "b"}), None),
        ]),
        FauxResponseStep::factory(|context, _, _, _| {
            let results = context
                .messages
                .iter()
                .filter(|m| matches!(m, Message::ToolResult(_)))
                .count();
            Ok(faux_assistant_message(
                format!("tool results: {results}"),
                Default::default(),
            ))
        }),
    ]);
    h.session
        .prompt("run tools", PromptOptions::default())
        .await
        .unwrap();
    let mut sorted = runs.lock().unwrap().clone();
    sorted.sort();
    assert_eq!(sorted, ["fast:b", "slow:a"]);
    assert_eq!(h.roles().iter().filter(|r| *r == "toolResult").count(), 2);
    assert_eq!(h.roles().last().unwrap(), "assistant");
    assert_eq!(h.assistant_texts().last().unwrap(), "tool results: 2");
}

#[tokio::test(flavor = "multi_thread")]
async fn preserves_image_attachments_in_the_provider_context() {
    let h = Harness::new(HarnessOptions::default());
    let saw_image = Arc::new(Mutex::new(false));
    let flag = saw_image.clone();
    h.set_responses(vec![FauxResponseStep::factory(move |context, _, _, _| {
        *flag.lock().unwrap() = context.messages.iter().any(|m| match m {
            Message::User(u) => u
                .content
                .blocks()
                .iter()
                .any(|c| matches!(c, Content::Image(_))),
            _ => false,
        });
        Ok(faux_assistant_message("ok", Default::default()))
    })]);
    h.session
        .prompt(
            "describe",
            PromptOptions {
                images: vec![ImageContent {
                    data: "ZmFrZQ==".into(),
                    media_type: "image/png".into(),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(*saw_image.lock().unwrap());
}

/// A loader whose skills/templates expand like hoocode's (`/review` is a user
/// template, `/sys` a system template, `/ctx` a context template).
struct TemplateLoader;

impl ResourceLoader for TemplateLoader {
    fn expand_input(&self, text: &str) -> ExpandedInput {
        let (name, args) = text.split_once(' ').unwrap_or((text, ""));
        match name {
            "/review" => ExpandedInput {
                text: format!("Review this code: {args}"),
                template: Some(TemplateKind::User),
                args: args.into(),
            },
            "/sys" => ExpandedInput {
                text: "Be terse.".into(),
                template: Some(TemplateKind::System),
                args: args.into(),
            },
            "/ctx" => ExpandedInput {
                text: "Context body.".into(),
                template: Some(TemplateKind::Context),
                args: args.into(),
            },
            _ => ExpandedInput::plain(text),
        }
    }
}

fn capture_user_text(h: &Harness) -> Arc<Mutex<String>> {
    let captured = Arc::new(Mutex::new(String::new()));
    let sink = captured.clone();
    h.set_responses(vec![FauxResponseStep::factory(move |context, _, _, _| {
        let user = context
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m, Message::User(_)))
            .map(message_text)
            .unwrap_or_default();
        *sink.lock().unwrap() = user;
        Ok(faux_assistant_message("ok", Default::default()))
    })]);
    captured
}

#[tokio::test(flavor = "multi_thread")]
async fn expands_prompt_templates_before_sending_the_prompt() {
    let h = Harness::new(HarnessOptions {
        resource_loader: Some(Arc::new(TemplateLoader)),
        ..Default::default()
    });
    let captured = capture_user_text(&h);
    h.session
        .prompt("/review src/index.ts", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(*captured.lock().unwrap(), "Review this code: src/index.ts");
}

#[tokio::test(flavor = "multi_thread")]
async fn system_templates_extend_the_prompt_and_send_their_args() {
    let h = Harness::new(HarnessOptions {
        resource_loader: Some(Arc::new(TemplateLoader)),
        ..Default::default()
    });
    let base = h.session.system_prompt();
    let captured = capture_user_text(&h);
    h.session
        .prompt("/sys do it", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(*captured.lock().unwrap(), "do it");
    assert_eq!(h.session.system_prompt(), format!("{base}\n\nBe terse."));

    // A context template rides along as a hidden custom message; the next
    // prompt resets the system prompt to the base.
    let captured = capture_user_text(&h);
    h.session
        .prompt("/ctx the args", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(*captured.lock().unwrap(), "the args");
    assert_eq!(h.session.system_prompt(), base);
    assert_eq!(
        h.roles()[2..],
        ["custom", "user", "assistant"].map(String::from)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn skips_expansion_when_disabled() {
    let h = Harness::new(HarnessOptions {
        resource_loader: Some(Arc::new(TemplateLoader)),
        ..Default::default()
    });
    let captured = capture_user_text(&h);
    h.session
        .prompt(
            "/review x",
            PromptOptions {
                expand_prompt_templates: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(*captured.lock().unwrap(), "/review x");
}

/// Records `/testcmd` runs.
pub struct TestCommands(pub Arc<Mutex<Vec<String>>>);

impl ExtensionHooks for TestCommands {
    fn has_command(&self, name: &str) -> bool {
        name == "testcmd"
    }
    fn run_command(&self, _name: &str, args: &str) -> CommandFuture {
        self.0.lock().unwrap().push(args.to_string());
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatches_extension_commands_without_consuming_a_provider_response() {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let h = Harness::new(HarnessOptions {
        extensions: Some(Arc::new(TestCommands(runs.clone()))),
        ..Default::default()
    });
    h.set_responses(vec![text("should stay queued")]);
    h.session
        .prompt("/testcmd hello world", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(*runs.lock().unwrap(), ["hello world"]);
    assert!(h.session.messages().is_empty());
    assert_eq!(h.pending_response_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn send_user_message_while_idle_triggers_a_turn() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("response")]);
    h.session
        .send_user_message("from extension".to_string().into(), None)
        .await
        .unwrap();
    assert_eq!(h.roles(), ["user", "assistant"]);
    assert_eq!(h.user_texts(), ["from extension"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn throws_when_prompted_during_streaming_without_a_streaming_behavior() {
    let wait = WaitTool::new();
    let h = Harness::new(HarnessOptions {
        tools: vec![wait.definition.clone()],
        ..Default::default()
    });
    h.set_responses(vec![
        tool_use(vec![faux_tool_call("wait", serde_json::json!({}), None)]),
        text("done"),
    ]);
    let (started, _sub) = common::wait_for_tool_start(&h.session, "wait");
    let session = h.session.clone();
    let run = tokio::spawn(async move { session.prompt("start", PromptOptions::default()).await });
    started.await.unwrap();

    assert_eq!(
        h.session
            .prompt("second", PromptOptions::default())
            .await
            .unwrap_err()
            .0,
        "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message."
    );
    // concurrent: steer() and followUp() are allowed while streaming.
    h.session.steer("steer msg", &[]).unwrap();
    h.session.follow_up("follow msg", &[]).unwrap();
    assert_eq!(h.session.pending_message_count(), 2);
    h.session.clear_queue();

    wait.release();
    run.await.unwrap().unwrap();
    // concurrent: prompt() works again once the run is over.
    h.set_responses(vec![text("second response")]);
    h.session
        .prompt("second", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(h.assistant_texts().last().unwrap(), "second response");
}

#[tokio::test(flavor = "multi_thread")]
async fn queues_with_streaming_behavior_while_streaming() {
    let wait = WaitTool::new();
    let h = Harness::new(HarnessOptions {
        tools: vec![wait.definition.clone()],
        ..Default::default()
    });
    h.set_responses(vec![
        tool_use(vec![faux_tool_call("wait", serde_json::json!({}), None)]),
        text("handled steer"),
    ]);
    let (started, _sub) = common::wait_for_tool_start(&h.session, "wait");
    let session = h.session.clone();
    let run = tokio::spawn(async move { session.prompt("start", PromptOptions::default()).await });
    started.await.unwrap();
    h.session
        .prompt(
            "queued steer",
            PromptOptions {
                streaming_behavior: Some(StreamingBehavior::Steer),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(h.session.get_steering_messages(), ["queued steer"]);
    wait.release();
    run.await.unwrap().unwrap();
    assert_eq!(h.user_texts(), ["start", "queued steer"]);
    assert_eq!(h.session.pending_message_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn throws_when_prompting_without_a_model() {
    let h = Harness::new(HarnessOptions::default());
    h.session
        .agent()
        .set_model(cortexcode_agent_core::Agent::new().state().model);
    let error = h
        .session
        .prompt("hi", PromptOptions::default())
        .await
        .unwrap_err();
    assert!(error.0.starts_with("No model selected."), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn throws_when_prompting_without_configured_auth() {
    let h = Harness::new(HarnessOptions {
        without_auth: true,
        ..Default::default()
    });
    let provider = h.faux.get_model().provider;
    let error = h
        .session
        .prompt("hi", PromptOptions::default())
        .await
        .unwrap_err();
    assert!(
        error
            .0
            .starts_with(&format!("No API key found for {provider}.")),
        "{error}"
    );
}

/// A real loader (hoocode's `DefaultResourceLoader`) whose skills are fixed.
fn skill_loader(temp: &std::path::Path, skill_path: &str) -> Arc<dyn ResourceLoader> {
    use cortexcode_code_resources::source_info::{create_synthetic_source_info, SourceScope};
    let base = temp.to_string_lossy().into_owned();
    let skill = cortexcode_code_resources::Skill {
        name: "test".into(),
        description: "Test skill".into(),
        file_path: skill_path.into(),
        base_dir: base.clone(),
        source_info: create_synthetic_source_info(
            skill_path,
            "local",
            Some(SourceScope::Project),
            None,
            Some(&base),
        ),
        disable_model_invocation: false,
        allowed_tools: None,
    };
    Arc::new(cortexcode_code_agent_session::DefaultResources::loaded(
        cortexcode_code_resources::DefaultResourceLoaderOptions {
            cwd: base.clone(),
            agent_dir: format!("{base}/agent"),
            home: Some(format!("{base}/home")),
            user_agents_dir: Some(format!("{base}/home/.agents")),
            no_context_files: true,
            skills_override: Some(Box::new(move |_| {
                cortexcode_code_resources::LoadSkillsResult {
                    skills: vec![skill.clone()],
                    diagnostics: vec![],
                }
            })),
            ..Default::default()
        },
    ))
}

#[tokio::test(flavor = "multi_thread")]
async fn expands_skill_commands_before_sending_the_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let skill_path = temp.path().join("test-skill.md");
    std::fs::write(&skill_path, "# Test Skill\n\nUse the skill body.").unwrap();
    let h = Harness::new(HarnessOptions {
        resource_loader: Some(skill_loader(temp.path(), &skill_path.to_string_lossy())),
        ..Default::default()
    });
    let captured = capture_user_text(&h);
    h.session
        .prompt("/skill:test explain this", PromptOptions::default())
        .await
        .unwrap();
    let text = captured.lock().unwrap().clone();
    assert!(text.contains("<skill name=\"test\" location=\""));
    assert!(text.contains("Use the skill body."));
    assert!(text.contains("explain this"));
}

/// A `session_start` handler that narrows the active tools (`hoo.setActiveTools`).
struct ReloadFilter(Arc<Mutex<Vec<cortexcode_code_agent_session::SessionStartReason>>>);

impl ExtensionHooks for ReloadFilter {
    fn has_handlers(&self, event_type: &str) -> bool {
        event_type == "session_start"
    }
    fn emit_session_event(
        &self,
        event: cortexcode_code_agent_session::SessionEvent,
    ) -> cortexcode_code_agent_session::SessionEventFuture {
        if let cortexcode_code_agent_session::SessionEvent::Start(start) = event {
            self.0.lock().unwrap().push(start.reason);
        }
        Box::pin(async {
            cortexcode_code_agent_session::SessionEventResult {
                active_tools: Some(vec!["read".to_string()]),
                ..Default::default()
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_emits_session_start_and_applies_its_tool_filter() {
    let reasons = Arc::new(Mutex::new(Vec::new()));
    let h = Harness::new(HarnessOptions {
        tools: vec![
            tool("read", |_| text_result("")),
            tool("write", |_| text_result("")),
        ],
        extensions: Some(Arc::new(ReloadFilter(reasons.clone()))),
        ..Default::default()
    });
    assert_eq!(h.session.get_active_tool_names(), ["read", "write"]);
    h.session.reload().await;
    assert_eq!(
        *reasons.lock().unwrap(),
        [cortexcode_code_agent_session::SessionStartReason::Reload]
    );
    assert_eq!(h.session.get_active_tool_names(), ["read"]);
}
