//! Threads, turns, items and user input.

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One piece of user input. Types we don't handle are kept as raw JSON in
/// [`UserInput::Other`].
#[derive(Debug, Clone, PartialEq)]
pub enum UserInput {
    Text { text: String },
    Image { url: String },
    LocalImage { path: String },
    Other(Value),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum KnownUserInput {
    Text {
        text: String,
        #[serde(default)]
        text_elements: Vec<Value>,
    },
    Image {
        url: String,
    },
    LocalImage {
        path: String,
    },
}

impl Serialize for UserInput {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            UserInput::Text { text } => KnownUserInput::Text {
                text: text.clone(),
                text_elements: Vec::new(),
            }
            .serialize(s),
            UserInput::Image { url } => KnownUserInput::Image { url: url.clone() }.serialize(s),
            UserInput::LocalImage { path } => {
                KnownUserInput::LocalImage { path: path.clone() }.serialize(s)
            }
            UserInput::Other(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for UserInput {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        Ok(
            match serde_json::from_value::<KnownUserInput>(value.clone()) {
                Ok(KnownUserInput::Text { text, .. }) => UserInput::Text { text },
                Ok(KnownUserInput::Image { url }) => UserInput::Image { url },
                Ok(KnownUserInput::LocalImage { path }) => UserInput::LocalImage { path },
                Err(_) => UserInput::Other(value),
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadActiveFlag {
    WaitingOnApproval,
    WaitingOnUserInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadStatus {
    NotLoaded,
    Idle,
    SystemError,
    #[serde(rename_all = "camelCase")]
    Active {
        active_flags: Vec<ThreadActiveFlag>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_details: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: String,
    pub items: Vec<ThreadItem>,
    pub status: TurnStatus,
    #[serde(default)]
    pub error: Option<TurnError>,
    /// Unix seconds.
    #[serde(default)]
    pub started_at: Option<i64>,
    /// Unix seconds.
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DynamicToolCallStatus {
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PatchChangeKind {
    Add,
    Delete,
    #[serde(rename_all = "camelCase")]
    Update {
        #[serde(default)]
        move_path: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileUpdateChange {
    pub path: String,
    pub kind: PatchChangeKind,
    pub diff: String,
}

/// One item in a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadItem {
    #[serde(rename_all = "camelCase")]
    UserMessage { id: String, content: Vec<UserInput> },
    #[serde(rename_all = "camelCase")]
    AgentMessage { id: String, text: String },
    #[serde(rename_all = "camelCase")]
    Reasoning {
        id: String,
        #[serde(default)]
        summary: Vec<String>,
        #[serde(default)]
        content: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    CommandExecution {
        id: String,
        command: String,
        cwd: String,
        source: String,
        status: ItemStatus,
        #[serde(default)]
        command_actions: Vec<Value>,
        #[serde(default)]
        aggregated_output: Option<String>,
        #[serde(default)]
        exit_code: Option<i32>,
        #[serde(default)]
        duration_ms: Option<i64>,
    },
    #[serde(rename_all = "camelCase")]
    FileChange {
        id: String,
        changes: Vec<FileUpdateChange>,
        status: ItemStatus,
    },
    #[serde(rename_all = "camelCase")]
    DynamicToolCall {
        id: String,
        tool: String,
        arguments: Value,
        status: DynamicToolCallStatus,
        #[serde(default)]
        success: Option<bool>,
        #[serde(default)]
        content_items: Option<Vec<Value>>,
        #[serde(default)]
        duration_ms: Option<i64>,
    },
}

impl ThreadItem {
    pub fn id(&self) -> &str {
        match self {
            ThreadItem::UserMessage { id, .. }
            | ThreadItem::AgentMessage { id, .. }
            | ThreadItem::Reasoning { id, .. }
            | ThreadItem::CommandExecution { id, .. }
            | ThreadItem::FileChange { id, .. }
            | ThreadItem::DynamicToolCall { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub session_id: String,
    pub preview: String,
    pub ephemeral: bool,
    pub model_provider: String,
    pub cwd: String,
    pub cli_version: String,
    pub source: String,
    pub status: ThreadStatus,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
    pub turns: Vec<Turn>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Always sent (`null` when unset).
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SandboxPolicy {
    DangerFullAccess,
    #[serde(rename_all = "camelCase")]
    ReadOnly {
        #[serde(default)]
        network_access: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_input_text_uses_snake_case_text_elements() {
        let v = serde_json::to_value(UserInput::Text { text: "hi".into() }).unwrap();
        assert_eq!(v, json!({"type":"text","text":"hi","text_elements":[]}));
        let back: UserInput = serde_json::from_value(v).unwrap();
        assert_eq!(back, UserInput::Text { text: "hi".into() });
    }

    #[test]
    fn unknown_user_input_is_kept() {
        let raw = json!({"type":"mention","name":"x","path":"/a"});
        let input: UserInput = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(input, UserInput::Other(raw.clone()));
        assert_eq!(serde_json::to_value(input).unwrap(), raw);
    }

    #[test]
    fn items_are_tagged_and_camel_case() {
        let item = ThreadItem::CommandExecution {
            id: "i".into(),
            command: "ls".into(),
            cwd: "/w".into(),
            source: "agent".into(),
            status: ItemStatus::InProgress,
            command_actions: vec![],
            aggregated_output: None,
            exit_code: None,
            duration_ms: None,
        };
        assert_eq!(
            serde_json::to_value(&item).unwrap(),
            json!({"type":"commandExecution","id":"i","command":"ls","cwd":"/w","source":"agent",
                   "status":"inProgress","commandActions":[],"aggregatedOutput":null,
                   "exitCode":null,"durationMs":null})
        );
        let fc = ThreadItem::FileChange {
            id: "f".into(),
            changes: vec![FileUpdateChange {
                path: "a.rs".into(),
                kind: PatchChangeKind::Update { move_path: None },
                diff: "-a\n+b".into(),
            }],
            status: ItemStatus::Declined,
        };
        assert_eq!(
            serde_json::to_value(&fc).unwrap(),
            json!({"type":"fileChange","id":"f","status":"declined",
                   "changes":[{"path":"a.rs","kind":{"type":"update","movePath":null},"diff":"-a\n+b"}]})
        );
        assert_eq!(fc.id(), "f");
    }

    #[test]
    fn statuses() {
        assert_eq!(
            serde_json::to_value(ThreadStatus::Active {
                active_flags: vec![ThreadActiveFlag::WaitingOnApproval]
            })
            .unwrap(),
            json!({"type":"active","activeFlags":["waitingOnApproval"]})
        );
        assert_eq!(
            serde_json::to_value(ThreadStatus::NotLoaded).unwrap(),
            json!({"type":"notLoaded"})
        );
        assert_eq!(
            serde_json::to_value(SandboxPolicy::DangerFullAccess).unwrap(),
            json!({"type":"dangerFullAccess"})
        );
    }

    #[test]
    fn thread_always_sends_project_id() {
        let t = Thread {
            id: "t".into(),
            session_id: "t".into(),
            preview: "".into(),
            ephemeral: false,
            model_provider: "p".into(),
            cwd: "/w".into(),
            cli_version: "0".into(),
            source: "appServer".into(),
            status: ThreadStatus::Idle,
            created_at: 1,
            updated_at: 2,
            turns: vec![],
            path: None,
            name: None,
            model: None,
            project_id: None,
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["projectId"], Value::Null);
        assert_eq!(v["sessionId"], json!("t"));
        assert_eq!(v["createdAt"], json!(1));
    }
}
