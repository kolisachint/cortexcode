//! Method names, request params and results, notifications and server
//! requests. Incoming params ignore fields we don't use: real clients send
//! many more.

use crate::jsonrpc::RequestId;
use crate::types::{SandboxPolicy, Thread, ThreadItem, ThreadStatus, Turn, TurnError, UserInput};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Wire method names.
pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const INITIALIZED: &str = "initialized";
    pub const THREAD_START: &str = "thread/start";
    pub const THREAD_RESUME: &str = "thread/resume";
    pub const THREAD_LIST: &str = "thread/list";
    pub const THREAD_READ: &str = "thread/read";
    pub const THREAD_UNSUBSCRIBE: &str = "thread/unsubscribe";
    pub const TURN_START: &str = "turn/start";
    pub const TURN_STEER: &str = "turn/steer";
    pub const TURN_INTERRUPT: &str = "turn/interrupt";
    pub const ACCOUNT_READ: &str = "account/read";
    pub const MODEL_LIST: &str = "model/list";
    pub const CONFIG_REQUIREMENTS_READ: &str = "configRequirements/read";
    pub const SKILLS_LIST: &str = "skills/list";
    pub const THREAD_LOADED_LIST: &str = "thread/loaded/list";

    pub const THREAD_STARTED: &str = "thread/started";
    pub const THREAD_STATUS_CHANGED: &str = "thread/status/changed";
    pub const TURN_STARTED: &str = "turn/started";
    pub const TURN_COMPLETED: &str = "turn/completed";
    pub const ITEM_STARTED: &str = "item/started";
    pub const ITEM_COMPLETED: &str = "item/completed";
    pub const AGENT_MESSAGE_DELTA: &str = "item/agentMessage/delta";
    pub const SERVER_REQUEST_RESOLVED: &str = "serverRequest/resolved";
    pub const ERROR: &str = "error";

    pub const COMMAND_EXECUTION_REQUEST_APPROVAL: &str = "item/commandExecution/requestApproval";
    pub const FILE_CHANGE_REQUEST_APPROVAL: &str = "item/fileChange/requestApproval";
}

// ---------------------------------------------------------------------------
// initialize

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeCapabilities {
    #[serde(default)]
    pub experimental_api: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opt_out_notification_methods: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<InitializeCapabilities>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub user_agent: String,
    pub codex_home: String,
    pub platform_family: String,
    pub platform_os: String,
}

// ---------------------------------------------------------------------------
// thread/*

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// The result of `thread/start` and `thread/resume`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSessionResponse {
    pub thread: Thread,
    pub model: String,
    pub model_provider: String,
    #[serde(default)]
    pub service_tier: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub instruction_sources: Vec<String>,
    pub approval_policy: String,
    pub approvals_reviewer: String,
    pub sandbox: SandboxPolicy,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListResponse {
    pub data: Vec<Thread>,
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub backwards_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadParams {
    pub thread_id: String,
    #[serde(default)]
    pub include_turns: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadReadResponse {
    pub thread: Thread,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUnsubscribeParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadUnsubscribeStatus {
    NotLoaded,
    NotSubscribed,
    Unsubscribed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadUnsubscribeResponse {
    pub status: ThreadUnsubscribeStatus,
}

// ---------------------------------------------------------------------------
// turn/*

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnStartResponse {
    pub turn: Turn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    pub expected_turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerResponse {
    pub turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: String,
    pub turn_id: String,
}

/// Serializes to `{}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnInterruptResponse {}

// ---------------------------------------------------------------------------
// Neutral results for clients that probe at startup.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAccountResponse {
    #[serde(default)]
    pub account: Option<Value>,
    pub requires_openai_auth: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortOption {
    pub reasoning_effort: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub model: String,
    pub display_name: String,
    pub description: String,
    pub hidden: bool,
    pub is_default: bool,
    pub default_reasoning_effort: String,
    pub supported_reasoning_efforts: Vec<ReasoningEffortOption>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
}

/// `model/list`. Hidden models (outside the user's model scope) are left out
/// unless `includeHidden`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListParams {
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListResponse {
    pub data: Vec<ModelInfo>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfigRequirementsReadResponse {
    #[serde(default)]
    pub requirements: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillsListParams {
    #[serde(default)]
    pub cwds: Vec<String>,
}

/// Skills for one folder. hoocode's skills are not exposed yet: always empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillsListEntry {
    pub cwd: String,
    pub skills: Vec<Value>,
    pub errors: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillsListResponse {
    pub data: Vec<SkillsListEntry>,
}

/// Ids of the threads loaded in memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadLoadedListResponse {
    pub data: Vec<String>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Notifications

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadStartedNotification {
    pub thread: Thread,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStatusChangedNotification {
    pub thread_id: String,
    pub status: ThreadStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedNotification {
    pub thread_id: String,
    pub turn: Turn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedNotification {
    pub thread_id: String,
    pub turn: Turn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemStartedNotification {
    pub item: ThreadItem,
    pub thread_id: String,
    pub turn_id: String,
    pub started_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemCompletedNotification {
    pub item: ThreadItem,
    pub thread_id: String,
    pub turn_id: String,
    pub completed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerRequestResolvedNotification {
    pub thread_id: String,
    pub request_id: RequestId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorNotification {
    pub error: TurnError,
    pub will_retry: bool,
    pub thread_id: String,
    pub turn_id: String,
}

// ---------------------------------------------------------------------------
// Server requests (approvals)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandExecutionRequestApprovalParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub started_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeRequestApprovalParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub started_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_root: Option<String>,
}

/// The answer to an approval request. Amendment variants (objects such as
/// `{"acceptWithExecpolicyAmendment": …}`) all become `AcceptWithAmendment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    AcceptWithAmendment,
    Decline,
    Cancel,
}

impl Serialize for ApprovalDecision {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            // Never sent by us; `accept` is the closest plain decision.
            ApprovalDecision::Accept | ApprovalDecision::AcceptWithAmendment => "accept",
            ApprovalDecision::AcceptForSession => "acceptForSession",
            ApprovalDecision::Decline => "decline",
            ApprovalDecision::Cancel => "cancel",
        })
    }
}

impl<'de> Deserialize<'de> for ApprovalDecision {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => match s.as_str() {
                "accept" => Ok(ApprovalDecision::Accept),
                "acceptForSession" => Ok(ApprovalDecision::AcceptForSession),
                "decline" => Ok(ApprovalDecision::Decline),
                "cancel" => Ok(ApprovalDecision::Cancel),
                other => Err(serde::de::Error::custom(format!(
                    "unknown approval decision `{other}`"
                ))),
            },
            Value::Object(_) => Ok(ApprovalDecision::AcceptWithAmendment),
            _ => Err(serde::de::Error::custom("invalid approval decision")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub decision: ApprovalDecision,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initialize_ignores_unknown_fields() {
        let p: InitializeParams = serde_json::from_value(json!({
            "clientInfo": {"name":"codex-tui","title":null,"version":"0.159.3"},
            "capabilities": {"experimentalApi": true, "requestAttestation": false}
        }))
        .unwrap();
        assert_eq!(p.client_info.name, "codex-tui");
        assert!(p.capabilities.unwrap().experimental_api);
        let r = InitializeResponse {
            user_agent: "hoocode/0.1.1".into(),
            codex_home: "/h".into(),
            platform_family: "unix".into(),
            platform_os: "macos".into(),
        };
        assert_eq!(
            serde_json::to_value(r).unwrap(),
            json!({"userAgent":"hoocode/0.1.1","codexHome":"/h","platformFamily":"unix","platformOs":"macos"})
        );
    }

    #[test]
    fn thread_start_accepts_empty_and_full_params() {
        let p: ThreadStartParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!(p, ThreadStartParams::default());
        let p: ThreadStartParams = serde_json::from_value(json!({
            "model":"m","cwd":"/w","approvalPolicy":"on-request","sandbox":"workspace-write",
            "config":{"hoocode.profile":"reviewer"},"experimentalRawEvents":false
        }))
        .unwrap();
        assert_eq!(p.model.as_deref(), Some("m"));
        assert_eq!(p.config.unwrap()["hoocode.profile"], json!("reviewer"));
    }

    #[test]
    fn required_nullable_fields_are_sent() {
        let r = ThreadListResponse {
            data: vec![],
            next_cursor: None,
            backwards_cursor: None,
        };
        assert_eq!(
            serde_json::to_value(r).unwrap(),
            json!({"data":[],"nextCursor":null,"backwardsCursor":null})
        );
        assert_eq!(
            serde_json::to_value(TurnInterruptResponse {}).unwrap(),
            json!({})
        );
    }

    #[test]
    fn approval_decisions_parse_leniently() {
        let d = |v: Value| serde_json::from_value::<ApprovalResponse>(json!({"decision": v}));
        assert_eq!(
            d(json!("accept")).unwrap().decision,
            ApprovalDecision::Accept
        );
        assert_eq!(
            d(json!("acceptForSession")).unwrap().decision,
            ApprovalDecision::AcceptForSession
        );
        assert_eq!(
            d(json!("decline")).unwrap().decision,
            ApprovalDecision::Decline
        );
        assert_eq!(
            d(json!("cancel")).unwrap().decision,
            ApprovalDecision::Cancel
        );
        assert_eq!(
            d(json!({"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["ls"]}}))
                .unwrap()
                .decision,
            ApprovalDecision::AcceptWithAmendment
        );
        assert!(d(json!("maybe")).is_err());
        assert!(d(json!(3)).is_err());
    }

    #[test]
    fn approval_request_params_shape() {
        let p = CommandExecutionRequestApprovalParams {
            thread_id: "t".into(),
            turn_id: "u".into(),
            item_id: "i".into(),
            started_at_ms: 5,
            command: Some("ls".into()),
            cwd: Some("/w".into()),
            reason: None,
        };
        assert_eq!(
            serde_json::to_value(p).unwrap(),
            json!({"threadId":"t","turnId":"u","itemId":"i","startedAtMs":5,"command":"ls","cwd":"/w"})
        );
    }
}
