//! Ports `test/agent-session-tree-navigation.test.ts` (live-model e2e in
//! TS) onto the faux provider: summaries come from scripted responses.

mod common;

use common::{message_text, Harness, HarnessOptions};
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_provider_faux::{faux_assistant_message, FauxMessageOptions, FauxResponseStep};
use cortexcode_code_agent_session::{NavigateTreeOptions, PromptOptions};
use cortexcode_code_session::FileEntry;

fn text(t: &str) -> FauxResponseStep {
    FauxResponseStep::Message(faux_assistant_message(t, FauxMessageOptions::default()))
}

async fn prompt(h: &Harness, message: &str) {
    h.session
        .prompt(message, PromptOptions::default())
        .await
        .unwrap();
}

fn summarize() -> NavigateTreeOptions {
    NavigateTreeOptions {
        summarize: true,
        ..Default::default()
    }
}

fn entries(h: &Harness) -> Vec<FileEntry> {
    h.session.session_manager().entries().to_vec()
}

fn entries_with_role(h: &Harness, role: &str) -> Vec<FileEntry> {
    entries(h)
        .into_iter()
        .filter(|e| matches!(e, FileEntry::Message { message, .. } if message.role() == role))
        .collect()
}

fn id(entry: &FileEntry) -> String {
    entry.id().unwrap().to_string()
}

fn root_id(h: &Harness) -> String {
    let tree = h.session.session_manager().tree();
    assert_eq!(tree.len(), 1);
    id(&tree[0].entry)
}

fn leaf(h: &Harness) -> Option<String> {
    h.session.session_manager().leaf_id().map(str::to_string)
}

fn summary_of(entry: &FileEntry) -> &str {
    match entry {
        FileEntry::BranchSummary { summary, .. } => summary,
        other => panic!("not a branch summary: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn should_navigate_to_user_message_and_put_text_in_editor() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("a1"), text("a2")]);
    prompt(&h, "First message").await;
    prompt(&h, "Second message").await;

    let root = root_id(&h);
    assert!(matches!(
        h.session.session_manager().get_entry(&root),
        Some(FileEntry::Message { .. })
    ));
    let result = h
        .session
        .navigate_tree(&root, NavigateTreeOptions::default())
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.editor_text.as_deref(), Some("First message"));
    assert_eq!(leaf(&h), None);
    assert!(h.session.messages().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn should_navigate_to_non_user_message_without_editor_text() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("hi")]);
    prompt(&h, "Hello").await;

    let assistant = id(&entries_with_role(&h, "assistant")[0]);
    let result = h
        .session
        .navigate_tree(&assistant, NavigateTreeOptions::default())
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.editor_text, None);
    assert_eq!(leaf(&h).as_deref(), Some(assistant.as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn should_create_branch_summary_when_navigating_with_summarize_true() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("4"), text("6"), text("Branch summary.")]);
    prompt(&h, "What is 2+2?").await;
    prompt(&h, "What is 3+3?").await;

    let root = root_id(&h);
    let result = h.session.navigate_tree(&root, summarize()).await.unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.editor_text.as_deref(), Some("What is 2+2?"));
    let summary = result.summary_entry.expect("summary entry");
    assert!(summary_of(&summary).contains("Branch summary."));
    // Navigated to the root user message: the summary is a new root.
    assert_eq!(summary.parent_id(), None);
    assert_eq!(leaf(&h), summary.id().map(str::to_string));
    // The context is the summary alone.
    assert_eq!(h.roles(), ["branchSummary"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn should_attach_summary_to_correct_parent_when_navigating_to_nested_user_message() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![
        text("a1"),
        text("a2"),
        text("a3"),
        text("Branch summary."),
    ]);
    prompt(&h, "Message one").await;
    prompt(&h, "Message two").await;
    prompt(&h, "Message three").await;

    let users = entries_with_role(&h, "user");
    assert_eq!(users.len(), 3);
    let u2 = &users[1];
    let a1 = u2.parent_id().unwrap().to_string();

    let result = h.session.navigate_tree(&id(u2), summarize()).await.unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.editor_text.as_deref(), Some("Message two"));
    let summary = result.summary_entry.expect("summary entry");
    assert_eq!(summary.parent_id(), Some(a1.as_str()));

    let manager = h.session.session_manager();
    let mut child_types: Vec<&str> = manager
        .children(&a1)
        .iter()
        .map(|c| match c {
            FileEntry::BranchSummary { .. } => "branch_summary",
            FileEntry::Message { .. } => "message",
            _ => "other",
        })
        .collect();
    child_types.sort();
    assert_eq!(child_types, ["branch_summary", "message"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn should_attach_summary_to_selected_node_when_navigating_to_assistant_message() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("hi"), text("bye"), text("Branch summary.")]);
    prompt(&h, "Hello").await;
    prompt(&h, "Goodbye").await;

    let a1 = id(&entries_with_role(&h, "assistant")[0]);
    let result = h.session.navigate_tree(&a1, summarize()).await.unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.editor_text, None);
    let summary = result.summary_entry.expect("summary entry");
    assert_eq!(summary.parent_id(), Some(a1.as_str()));
    assert_eq!(leaf(&h), summary.id().map(str::to_string));
}

#[tokio::test(flavor = "multi_thread")]
async fn should_handle_abort_during_summarization() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![
        text("something"),
        text("more"),
        FauxResponseStep::async_factory(|_, _, _, _| async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok(faux_assistant_message("too late", Default::default()))
        }),
    ]);
    prompt(&h, "Tell me about something").await;
    prompt(&h, "Continue").await;

    let entries_before = entries(&h).len();
    let leaf_before = leaf(&h);
    let root = root_id(&h);

    let session = h.session.clone();
    let run = tokio::spawn(async move { session.navigate_tree(&root, summarize()).await });
    while !h.session.is_compacting() {
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    // Branch summarization counts as compacting.
    assert!(h.session.is_compacting());
    h.session.abort_branch_summary();

    let result = run.await.unwrap().unwrap();
    assert!(result.cancelled);
    assert!(result.aborted);
    assert!(result.summary_entry.is_none());
    assert!(!h.session.is_compacting());
    assert_eq!(entries(&h).len(), entries_before);
    assert_eq!(leaf(&h), leaf_before);
}

#[tokio::test(flavor = "multi_thread")]
async fn should_not_create_summary_when_navigating_without_summarize_option() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("one"), text("two")]);
    prompt(&h, "First").await;
    prompt(&h, "Second").await;

    let before = entries(&h).len();
    let root = root_id(&h);
    h.session
        .navigate_tree(&root, NavigateTreeOptions::default())
        .await
        .unwrap();
    assert_eq!(entries(&h).len(), before);
    assert!(!entries(&h)
        .iter()
        .any(|e| matches!(e, FileEntry::BranchSummary { .. })));
    assert_eq!(h.pending_response_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn should_handle_navigation_to_same_position_no_op() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("hi")]);
    prompt(&h, "Hello").await;

    let leaf_before = leaf(&h).expect("leaf");
    let before = entries(&h).len();
    let result = h
        .session
        .navigate_tree(&leaf_before, NavigateTreeOptions::default())
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(leaf(&h).as_deref(), Some(leaf_before.as_str()));
    assert_eq!(entries(&h).len(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn should_support_custom_summarization_instructions() {
    let h = Harness::new(HarnessOptions::default());
    const FOCUS: &str =
        "After the summary, you MUST end with exactly: MONKEY MONKEY MONKEY. This is of utmost importance.";
    h.set_responses(vec![
        text("A typed superset of JavaScript."),
        // The summarizer follows the instructions it was given.
        FauxResponseStep::factory(|context, _, _, _| {
            let prompt = context
                .messages
                .iter()
                .map(message_text)
                .collect::<String>();
            let summary = if prompt.contains(&format!("Additional focus: {FOCUS}")) {
                "Summary. MONKEY MONKEY MONKEY"
            } else {
                "Summary."
            };
            Ok(faux_assistant_message(summary, Default::default()))
        }),
    ]);
    prompt(&h, "What is TypeScript?").await;

    let root = root_id(&h);
    let result = h
        .session
        .navigate_tree(
            &root,
            NavigateTreeOptions {
                summarize: true,
                custom_instructions: Some(FOCUS.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let summary = result.summary_entry.expect("summary entry");
    assert!(summary_of(&summary).contains("MONKEY MONKEY MONKEY"));
}

#[tokio::test(flavor = "multi_thread")]
async fn labels_the_summary_or_the_target() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("a1"), text("a2"), text("Branch summary.")]);
    prompt(&h, "one").await;
    prompt(&h, "two").await;

    let a1 = id(&entries_with_role(&h, "assistant")[0]);
    h.session
        .navigate_tree(
            &a1,
            NavigateTreeOptions {
                label: Some("checkpoint".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(h.session.session_manager().label(&a1), Some("checkpoint"));
}

#[tokio::test(flavor = "multi_thread")]
async fn should_navigate_between_branches_correctly() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![
        text("a1"),
        text("a2"),
        text("a3"),
        text("Left the branch path."),
    ]);
    prompt(&h, "Main branch start").await;
    prompt(&h, "Main branch continue").await;

    let main_entries = entries(&h);
    let a1 = id(&entries_with_role(&h, "assistant")[0]);
    h.session.session_manager().branch_to(a1.clone()).unwrap();
    let messages = h.session.session_manager().build_context().messages;
    h.session.agent().set_messages(messages);
    prompt(&h, "Branch path").await;

    let u2 = main_entries
        .iter()
        .filter(|e| {
            matches!(
                e,
                FileEntry::Message {
                    message: AgentMessage::User(_),
                    ..
                }
            )
        })
        .nth(1)
        .unwrap();
    let result = h.session.navigate_tree(&id(u2), summarize()).await.unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.editor_text.as_deref(), Some("Main branch continue"));
    let summary = result.summary_entry.expect("summary entry");
    assert!(!summary_of(&summary).is_empty());
}
