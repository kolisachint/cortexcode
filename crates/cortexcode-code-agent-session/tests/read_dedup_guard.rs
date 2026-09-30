//! Port of the pin's `coding-agent/test/suite/read-dedup-guard.test.ts`: the
//! read dedup guard through the real tool pipeline, gated on `contextGc`.

mod common;

use common::{text_of, Harness, HarnessOptions};
use cortexcode_ai_provider_faux::{faux_assistant_message, faux_tool_call, FauxMessageOptions};
use cortexcode_ai_types::{Message, StopReason};
use cortexcode_code_agent_session::PromptOptions;
use cortexcode_code_tools_fs::read_dedup::DEDUP_POINTER_PREFIX;

fn read_twice(harness: &Harness, file: &str) {
    let call = || {
        faux_assistant_message(
            vec![faux_tool_call(
                "read",
                serde_json::json!({"path": file}),
                None,
            )],
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..Default::default()
            },
        )
        .into()
    };
    harness.set_responses(vec![
        call(),
        call(),
        faux_assistant_message("done", Default::default()).into(),
    ]);
}

fn read_results(harness: &Harness) -> Vec<String> {
    harness
        .session
        .messages()
        .into_iter()
        .filter_map(
            |m| match cortexcode_agent_harness::messages::convert_to_llm(&[m]).pop() {
                Some(Message::ToolResult(r)) if r.tool_name == "read" => Some(text_of(&r.content)),
                _ => None,
            },
        )
        .collect()
}

fn harness(settings: serde_json::Value) -> Harness {
    Harness::new(HarnessOptions {
        real_builtin_tools: true,
        settings: settings.as_object().cloned().unwrap_or_default(),
        ..Default::default()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn short_circuits_a_re_read_of_a_file_already_read() {
    let h = harness(serde_json::json!({}));
    let file = h.temp_dir.path().join("doc.md");
    std::fs::write(&file, "line one\nline two\nline three\n").unwrap();
    read_twice(&h, file.to_str().unwrap());
    h.session
        .prompt("read the doc twice", PromptOptions::default())
        .await
        .unwrap();
    let results = read_results(&h);
    assert_eq!(results.len(), 2);
    assert!(results[0].contains("line two") && !results[0].contains(DEDUP_POINTER_PREFIX));
    assert!(results[1].contains(DEDUP_POINTER_PREFIX) && !results[1].contains("line two"));
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_short_circuit_when_context_gc_is_disabled() {
    let h = harness(serde_json::json!({"contextGc": {"enabled": false}}));
    let file = h.temp_dir.path().join("doc.md");
    std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
    read_twice(&h, file.to_str().unwrap());
    h.session
        .prompt("read the doc twice", PromptOptions::default())
        .await
        .unwrap();
    let results = read_results(&h);
    assert_eq!(results.len(), 2);
    assert!(results[1].contains("beta") && !results[1].contains(DEDUP_POINTER_PREFIX));
}
