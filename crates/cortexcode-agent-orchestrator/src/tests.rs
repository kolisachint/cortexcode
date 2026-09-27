//! Port of hoocode `packages/agent/test/harness/agent-harness.test.ts`
//! (v0.5.89), plus turn, hook, compaction and tree-navigation cases against
//! the faux provider.

use super::*;
use cortexcode_agent_harness::LocalExecutionEnv;
use cortexcode_agent_session::InMemorySessionStorage;
use cortexcode_ai_provider_faux::{
    faux_assistant_message, register_faux_provider, FauxProviderRegistration, FauxResponseStep,
};

/// Values a listener records.
type Recorded<T> = Arc<Mutex<Vec<T>>>;

type Harness<SK = Skill, PT = PromptTemplate> = AgentHarness<InMemorySessionStorage, SK, PT>;

fn new_harness(options: AgentHarnessOptions<InMemorySessionStorage>) -> Harness {
    AgentHarness::new(options)
}

fn env() -> Arc<dyn ExecutionEnv> {
    Arc::new(LocalExecutionEnv::new(
        std::env::current_dir().unwrap().to_string_lossy(),
    ))
}

fn session() -> Session<InMemorySessionStorage> {
    Session::new(InMemorySessionStorage::default())
}

fn catalog_model() -> Model {
    cortexcode_ai_models::get_model("anthropic", "claude-sonnet-4-5")
        .unwrap()
        .clone()
}

/// Unregisters on drop.
struct Faux(FauxProviderRegistration);

impl Drop for Faux {
    fn drop(&mut self) {
        self.0.unregister();
    }
}

fn faux(replies: Vec<FauxResponseStep>) -> (Faux, Model) {
    let registration = register_faux_provider(Default::default());
    registration.set_responses(replies);
    let model = registration.get_model();
    (Faux(registration), model)
}

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(text, Default::default()).into()
}

fn with_auth<SK, PT>(
    mut options: AgentHarnessOptions<InMemorySessionStorage, SK, PT>,
) -> AgentHarnessOptions<InMemorySessionStorage, SK, PT> {
    options.get_api_key_and_headers = Some(Arc::new(|_model: &Model| {
        Some(ApiKeyAndHeaders {
            api_key: "test-key".into(),
            headers: None,
        })
    }));
    options
}

fn roles(harness: &Harness) -> Vec<&'static str> {
    harness.with_session(|s| {
        s.branch(None)
            .iter()
            .map(|e| match e {
                FileEntry::Message { message, .. } => match message {
                    AgentMessage::User(_) => "user",
                    AgentMessage::Assistant(_) => "assistant",
                    _ => "other",
                },
                FileEntry::Compaction { .. } => "compaction",
                FileEntry::ModelChange { .. } => "model_change",
                FileEntry::ThinkingLevelChange { .. } => "thinking_level_change",
                FileEntry::BranchSummary { .. } => "branch_summary",
                _ => "entry",
            })
            .collect()
    })
}

// --- agent-harness.test.ts ---

#[test]
fn constructs_directly_and_exposes_queue_modes() {
    let env = env();
    let model = catalog_model();
    let mut options =
        AgentHarnessOptions::<_, Skill, PromptTemplate>::new(env.clone(), session(), model.clone());
    options.system_prompt = Some(SystemPrompt::Text("You are helpful.".into()));
    options.steering_mode = Some(QueueMode::All);
    options.follow_up_mode = Some(QueueMode::All);
    let harness = new_harness(options);
    assert!(Arc::ptr_eq(harness.env(), &env));
    assert_eq!(harness.agent().state().model, model);
    assert_eq!(harness.steering_mode(), QueueMode::All);
    assert_eq!(harness.follow_up_mode(), QueueMode::All);
    harness.set_steering_mode(QueueMode::OneAtATime);
    harness.set_follow_up_mode(QueueMode::OneAtATime);
    assert_eq!(harness.agent().steering_mode(), QueueMode::OneAtATime);
    assert_eq!(harness.agent().follow_up_mode(), QueueMode::OneAtATime);
}

#[derive(Debug, Clone, PartialEq)]
struct AppSkill {
    skill: Skill,
    source: &'static str,
}

impl SkillLike for AppSkill {
    fn skill(&self) -> &Skill {
        &self.skill
    }
}

#[derive(Debug, Clone, PartialEq)]
struct AppPromptTemplate {
    template: PromptTemplate,
    source: &'static str,
}

impl PromptTemplateLike for AppPromptTemplate {
    fn prompt_template(&self) -> &PromptTemplate {
        &self.template
    }
}

#[test]
fn preserves_app_resource_types_for_getters_and_update_events() {
    let harness: Harness<AppSkill, AppPromptTemplate> =
        AgentHarness::new(AgentHarnessOptions::new(env(), session(), catalog_model()));
    let skill = AppSkill {
        skill: Skill {
            name: "inspect".into(),
            description: "Inspect things".into(),
            content: "Use inspection tools.".into(),
            file_path: "/skills/inspect/SKILL.md".into(),
            disable_model_invocation: None,
        },
        source: "project",
    };
    let template = AppPromptTemplate {
        template: PromptTemplate {
            name: "review".into(),
            description: None,
            content: "Review $1".into(),
        },
        source: "user",
    };
    let resources = HarnessResources {
        skills: Some(vec![skill]),
        prompt_templates: Some(vec![template]),
    };
    let updates: Recorded<(Option<&'static str>, Option<&'static str>)> = Default::default();
    let record = updates.clone();
    let _unsubscribe = harness.subscribe(move |event, _| {
        if let HarnessEvent::ResourcesUpdate {
            resources,
            previous_resources,
        } = event
        {
            record.lock().unwrap().push((
                resources
                    .skills
                    .as_ref()
                    .and_then(|s| s.first())
                    .map(|s| s.source),
                previous_resources
                    .skills
                    .as_ref()
                    .and_then(|s| s.first())
                    .map(|s| s.source),
            ));
        }
    });
    harness.set_resources(resources.clone());
    harness.set_resources(resources);
    let resolved = harness.get_resources();
    assert_eq!(
        *updates.lock().unwrap(),
        [(Some("project"), None), (Some("project"), Some("project"))]
    );
    assert_eq!(resolved.skills.unwrap()[0].source, "project");
    assert_eq!(resolved.prompt_templates.unwrap()[0].source, "user");
}

// --- turns ---

#[tokio::test]
async fn a_prompt_writes_the_turn_to_the_session_and_settles() {
    let (_faux, model) = faux(vec![reply("hello back")]);
    let harness = new_harness(with_auth(AgentHarnessOptions::new(env(), session(), model)));
    let events: Arc<Mutex<Vec<String>>> = Default::default();
    let record = events.clone();
    let _unsubscribe = harness.subscribe(move |event, _| {
        let name = match event {
            HarnessEvent::Agent(AgentEvent::AgentEnd { .. }) => "agent_end",
            HarnessEvent::SavePoint { .. } => "save_point",
            HarnessEvent::Settled { .. } => "settled",
            HarnessEvent::AfterProviderResponse { status, .. } => {
                assert_eq!(*status, 200);
                "after_provider_response"
            }
            _ => return,
        };
        record.lock().unwrap().push(name.to_string());
    });
    let response = harness.prompt("hi", None).await.unwrap();
    assert_eq!(response.content, vec![Content::text("hello back")]);
    assert_eq!(harness.phase(), Phase::Idle);
    assert_eq!(roles(&harness), ["user", "assistant"]);
    assert_eq!(
        *events.lock().unwrap(),
        [
            "after_provider_response",
            "save_point",
            "agent_end",
            "settled"
        ]
    );
    // The default system prompt applies.
    assert_eq!(
        harness.agent().state().system_prompt,
        "You are a helpful assistant."
    );
}

#[tokio::test]
async fn next_turn_messages_and_before_agent_start_extend_the_prompt() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let record = seen.clone();
    let (_faux, model) = faux(vec![FauxResponseStep::factory(move |context, _, _, _| {
        let texts: Vec<String> = context
            .messages
            .iter()
            .filter_map(|m| match m {
                cortexcode_ai_types::Message::User(u) => Some(content_text(&u.content)),
                _ => None,
            })
            .collect();
        record.lock().unwrap().extend(texts);
        record
            .lock()
            .unwrap()
            .push(format!("system: {}", context.system_prompt));
        Ok(faux_assistant_message("ok", Default::default()))
    })]);
    let mut options = with_auth(AgentHarnessOptions::new(env(), session(), model));
    options.system_prompt = Some(SystemPrompt::Build(Arc::new(|ctx| {
        format!("model {}", ctx.model.id)
    })));
    let harness = new_harness(options);
    harness.next_turn("queued first", None);
    let _off = harness.on_before_agent_start(|event| {
        assert_eq!(event.prompt, "main");
        Some(BeforeAgentStartResult {
            messages: Some(vec![create_user_message("from hook", None)]),
            system_prompt: None,
        })
    });
    harness.prompt("main", None).await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen[..3], ["from hook", "queued first", "main"]);
    assert!(seen[3].starts_with("system: model "), "{seen:?}");
}

#[tokio::test]
async fn errors_for_unknown_resources_and_idle_queue_calls() {
    let harness = new_harness(AgentHarnessOptions::new(env(), session(), catalog_model()));
    assert_eq!(
        harness.skill("missing", None).await.unwrap_err(),
        "Unknown skill: missing"
    );
    assert_eq!(
        harness
            .prompt_from_template("missing", &[])
            .await
            .unwrap_err(),
        "Unknown prompt template: missing"
    );
    assert_eq!(harness.phase(), Phase::Idle);
    assert_eq!(
        harness.steer("x", None).unwrap_err(),
        "Cannot steer while idle"
    );
    assert_eq!(
        harness.follow_up("x", None).unwrap_err(),
        "Cannot follow up while idle"
    );
    assert_eq!(
        harness
            .set_active_tools(vec!["nope".into(), "also".into()])
            .unwrap_err(),
        "Unknown tool(s): nope, also"
    );
}

#[tokio::test]
async fn model_and_thinking_changes_are_written_when_idle() {
    let harness = new_harness(AgentHarnessOptions::new(env(), session(), catalog_model()));
    let selected: Arc<Mutex<Vec<String>>> = Default::default();
    let record = selected.clone();
    let _unsubscribe = harness.subscribe(move |event, _| match event {
        HarnessEvent::ModelSelect { model, .. } => record.lock().unwrap().push(model.id.clone()),
        HarnessEvent::ThinkingLevelSelect { level, .. } => record
            .lock()
            .unwrap()
            .push(thinking_level_str(level).to_string()),
        _ => {}
    });
    let other = cortexcode_ai_models::get_model("anthropic", "claude-opus-4-6")
        .unwrap()
        .clone();
    harness.set_model(other.clone()).unwrap();
    harness.set_thinking_level(ThinkingLevel::High).unwrap();
    assert_eq!(roles(&harness), ["model_change", "thinking_level_change"]);
    assert_eq!(*selected.lock().unwrap(), [other.id.as_str(), "high"]);
    assert_eq!(harness.agent().state().model, other);
}

#[tokio::test]
async fn compaction_writes_an_entry_and_honors_the_hook() {
    let (_faux, model) = faux(vec![reply("a1"), reply("a2"), reply("## Goal\nsummary")]);
    let harness = new_harness(with_auth(AgentHarnessOptions::new(env(), session(), model)));
    harness.prompt("first", None).await.unwrap();
    harness.prompt("second", None).await.unwrap();

    let off = harness.on_session_before_compact(|_| {
        Some(SessionBeforeCompactResult {
            cancel: true,
            compaction: None,
        })
    });
    assert_eq!(
        harness.compact(None).await.unwrap_err(),
        "Compaction cancelled"
    );
    off();

    let compacted: Arc<Mutex<Option<bool>>> = Default::default();
    let record = compacted.clone();
    let _unsubscribe = harness.subscribe(move |event, _| {
        if let HarnessEvent::SessionCompact { from_hook, .. } = event {
            *record.lock().unwrap() = Some(*from_hook);
        }
    });
    let result = harness.compact(None).await.unwrap();
    assert!(result.summary.starts_with("## Goal\nsummary"));
    assert_eq!(*compacted.lock().unwrap(), Some(false));
    assert_eq!(roles(&harness).last(), Some(&"compaction"));
    assert_eq!(harness.phase(), Phase::Idle);
}

#[tokio::test]
async fn navigating_to_a_user_message_returns_its_text_and_moves_to_its_parent() {
    let (_faux, model) = faux(vec![reply("a1"), reply("a2")]);
    let harness = new_harness(with_auth(AgentHarnessOptions::new(env(), session(), model)));
    harness.prompt("first", None).await.unwrap();
    harness.prompt("second", None).await.unwrap();
    let (second_user, first_assistant) = harness.with_session(|s| {
        let branch = s.branch(None);
        (
            branch[2].id().unwrap().to_string(),
            branch[1].id().unwrap().to_string(),
        )
    });
    let trees: Recorded<(Option<String>, Option<String>)> = Default::default();
    let record = trees.clone();
    let _unsubscribe = harness.subscribe(move |event, _| {
        if let HarnessEvent::SessionTree {
            new_leaf_id,
            old_leaf_id,
            ..
        } = event
        {
            record
                .lock()
                .unwrap()
                .push((new_leaf_id.clone(), old_leaf_id.clone()));
        }
    });
    let result = harness
        .navigate_tree(&second_user, NavigateTreeOptions::default())
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.editor_text.as_deref(), Some("second"));
    assert!(result.summary_entry.is_none());
    assert_eq!(
        harness.with_session(|s| s.leaf_id()),
        Some(first_assistant.clone())
    );
    assert_eq!(trees.lock().unwrap()[0].0, Some(first_assistant.clone()));

    // Navigating to the current leaf is a no-op.
    let again = harness
        .navigate_tree(&first_assistant, NavigateTreeOptions::default())
        .await
        .unwrap();
    assert!(!again.cancelled && again.editor_text.is_none());
    assert_eq!(
        harness
            .navigate_tree("missing", NavigateTreeOptions::default())
            .await
            .unwrap_err(),
        "Entry missing not found"
    );
}

#[tokio::test]
async fn tool_hooks_can_block_calls_and_the_context_hook_sees_messages() {
    use cortexcode_ai_provider_faux::faux_tool_call;
    let tool_call_reply: FauxResponseStep = AssistantMessage {
        content: vec![faux_tool_call(
            "echo",
            json!({"text": "x"}),
            Some("call-1".into()),
        )],
        stop_reason: cortexcode_ai_types::StopReason::ToolUse,
        ..faux_assistant_message("", Default::default())
    }
    .into();
    let (_faux, model) = faux(vec![tool_call_reply, reply("done")]);
    let tool = AgentTool {
        name: "echo".into(),
        description: "Echo".into(),
        label: "echo".into(),
        parameters: json!({"type": "object", "properties": {"text": {"type": "string"}}}),
        prepare_arguments: None,
        execute: Arc::new(|_, _, _, _| panic!("blocked tools do not run")),
        background: false,
        execution_mode: None,
        plain_json_schema: true,
    };
    let mut options = with_auth(AgentHarnessOptions::new(env(), session(), model));
    options.tools = vec![tool];
    let harness = new_harness(options);
    let _block = harness.on_tool_call(|event| {
        assert_eq!(event.tool_name, "echo");
        Some(ToolCallResult {
            block: Some(true),
            reason: Some("nope".into()),
        })
    });
    let contexts = Arc::new(Mutex::new(0));
    let count = contexts.clone();
    let _context = harness.on_context(move |messages| {
        assert!(!messages.is_empty());
        *count.lock().unwrap() += 1;
        None
    });
    let response = harness.prompt("use the tool", None).await.unwrap();
    assert_eq!(response.content, vec![Content::text("done")]);
    assert_eq!(*contexts.lock().unwrap(), 2);
    let blocked = harness.with_session(|s| {
        s.branch(None).iter().find_map(|e| match e {
            FileEntry::Message {
                message: AgentMessage::ToolResult(r),
                ..
            } => Some(r.clone()),
            _ => None,
        })
    });
    let blocked = blocked.unwrap();
    assert!(blocked.is_error);
    assert_eq!(blocked.content, vec![Content::text("nope")]);
}
