//! Wire compatibility with session files written by the pinned hoocode (0.5.89).
//! Fixtures: tests/fixtures/hoocode-0.5.89 (see its README).

use cortexcode_agent_types::AgentMessage;
use cortexcode_code_session::{
    build_session_context, load_raw_entries, load_session_file, migrate_session_entries, FileEntry,
};
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hoocode-0.5.89")
        .join(name)
}

/// Semantic equality with the two documented, hoocode-readable normalizations:
/// JS writes `0` where serde writes `0.0`, and `content: "text"` is written as
/// `[{"type":"text","text":"text"}]` (TS accepts both forms everywhere).
fn canon(v: &Value) -> Value {
    match v {
        Value::Number(n) => serde_json::json!(n.as_f64().unwrap()),
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| match (k.as_str(), v) {
                    ("content", Value::String(s)) => {
                        (k.clone(), serde_json::json!([{"type": "text", "text": s}]))
                    }
                    _ => (k.clone(), canon(v)),
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Every line parses into a typed entry and serializes back to the same JSON.
fn assert_round_trip(name: &str) {
    let text = std::fs::read_to_string(fixture(name)).unwrap();
    for (i, line) in text.lines().enumerate() {
        let original: Value = serde_json::from_str(line).unwrap();
        let entry: FileEntry = serde_json::from_value(original.clone())
            .unwrap_or_else(|e| panic!("{name}:{}: does not parse: {e}\n{line}", i + 1));
        let back = serde_json::to_value(&entry).unwrap();
        assert_eq!(
            canon(&back),
            canon(&original),
            "{name}:{}: round trip changed the entry",
            i + 1
        );
    }
}

#[test]
fn recorded_sessions_round_trip_exactly() {
    for name in ["chat-basic.jsonl", "tool-read.jsonl", "session-mixed.jsonl"] {
        assert_round_trip(name);
    }
}

#[test]
fn every_entry_and_message_type_round_trips() {
    assert_round_trip("all-entry-types.jsonl");
}

#[test]
fn string_content_is_accepted_and_emitted_as_blocks() {
    // `content: "plain string content"` on a custom message: accepted, written as blocks.
    let loaded = load_session_file(fixture("all-entry-types.jsonl"));
    assert_eq!(loaded.unrecognized, 0);
    let custom = loaded
        .entries
        .iter()
        .find_map(|e| match e {
            FileEntry::Message {
                message: AgentMessage::Custom(c),
                ..
            } => Some(c.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(&custom.content).unwrap(),
        serde_json::json!([{"type": "text", "text": "plain string content"}])
    );
}

#[test]
fn recorded_session_context() {
    let loaded = load_session_file(fixture("session-mixed.jsonl"));
    assert!(!loaded.migrated);
    assert_eq!(loaded.unrecognized, 0);
    let ctx = build_session_context(&loaded.entries[1..]);
    let model = ctx.model.unwrap();
    assert_eq!(
        (model.provider.as_str(), model.model_id.as_str()),
        ("mock", "mock-model")
    );
    assert_eq!(ctx.thinking_level, "off");
    let roles: Vec<&str> = ctx.messages.iter().map(|m| m.role()).collect();
    assert_eq!(
        roles,
        [
            "user",
            "assistant",
            "toolResult",
            "toolResult",
            "assistant",
            "user",
            "assistant"
        ]
    );
    match &ctx.messages[1] {
        AgentMessage::Assistant(a) => {
            assert_eq!(a.api, "openai-completions");
            assert_eq!(a.stop_reason, cortexcode_ai_types::StopReason::ToolUse);
        }
        other => panic!("{other:?}"),
    }
}

// Ports of test/session-manager/migration.test.ts

#[test]
fn migration_adds_id_and_parent_id_to_v1_entries() {
    let mut entries = load_raw_entries(fixture("legacy-v1.jsonl"));
    assert!(migrate_session_entries(&mut entries));
    assert_eq!(entries[0]["version"], 3);
    let (m1, m2) = (&entries[1], &entries[2]);
    assert_eq!(m1["id"].as_str().unwrap().len(), 8);
    assert!(m1["parentId"].is_null());
    assert_eq!(m2["id"].as_str().unwrap().len(), 8);
    assert_eq!(m2["parentId"], m1["id"]);
    // firstKeptEntryIndex -> firstKeptEntryId
    assert_eq!(entries[3]["firstKeptEntryId"], m2["id"]);
    assert!(entries[3].get("firstKeptEntryIndex").is_none());
}

#[test]
fn migration_is_idempotent_for_current_ids() {
    let mut entries = load_raw_entries(fixture("legacy-v2-hook.jsonl"));
    migrate_session_entries(&mut entries);
    assert_eq!(entries[1]["id"], "abc12345");
    assert_eq!(entries[2]["id"], "def67890");
    assert_eq!(entries[2]["parentId"], "abc12345");
    assert!(
        !migrate_session_entries(&mut entries),
        "second run is a no-op"
    );
}

#[test]
fn migration_renames_hook_message_role() {
    let loaded = load_session_file(fixture("legacy-v2-hook.jsonl"));
    assert!(loaded.migrated);
    assert_eq!(loaded.unrecognized, 0);
    assert!(matches!(
        &loaded.entries[2],
        FileEntry::Message { message: AgentMessage::Custom(c), .. } if c.custom_type == "x"
    ));
}

#[test]
fn v1_session_loads_through_typed_entries() {
    let loaded = load_session_file(fixture("legacy-v1.jsonl"));
    assert!(loaded.migrated);
    assert_eq!(loaded.unrecognized, 0);
    assert_eq!(loaded.entries.len(), 4);
}
