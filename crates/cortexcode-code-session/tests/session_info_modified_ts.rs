//! Port of the pin's `coding-agent/test/session-info-modified-timestamp.test.ts`:
//! `SessionInfo.modified` is the last user/assistant message time, not the
//! file's mtime.

use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{AssistantMessage, Content, StopReason};
use cortexcode_code_session::{list_sessions, SessionManager};

fn assistant(text: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![Content::text(text)],
        api: "openai-completions".into(),
        provider: "openai".into(),
        model: "test".into(),
        stop_reason: StopReason::Stop,
        timestamp,
        ..Default::default()
    })
}

#[test]
fn uses_the_last_message_timestamp_instead_of_file_mtime() {
    let dir = std::env::temp_dir().join(format!("session-modified-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"session\",\"id\":\"test-session\",\"version\":3,\"timestamp\":\"1970-01-01T00:00:00.000Z\",\"cwd\":\"/tmp\"}\n",
    )
    .unwrap();
    let now = || chrono::Utc::now().timestamp_millis();
    SessionManager::open(&path, None, None).append_message(assistant("hi", now()));
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));

    let msg_time = now();
    SessionManager::open(&path, None, None).append_message(assistant("later", msg_time));

    let sessions = list_sessions(&dir, None).unwrap();
    let info = sessions.iter().find(|s| s.path == path).expect("listed");
    assert_eq!(info.modified.timestamp_millis(), msg_time);
    let before_ms = before
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert_ne!(info.modified.timestamp_millis(), before_ms);
    let _ = std::fs::remove_dir_all(&dir);
}
