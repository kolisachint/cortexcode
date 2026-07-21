//! Permission policy for dangerous operations.

use cortexcode_agent_types::{AgentToolCall, PermissionDecision, PermissionGate};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Policy controlling whether dangerous tools require explicit approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionPolicy {
    /// Always require user approval for dangerous tools.
    #[default]
    Ask,
    /// Auto-approve dangerous tools.
    Auto,
    /// Reject dangerous tools entirely.
    Deny,
}

/// Whether a tool name is considered dangerous.
pub fn is_dangerous(tool_name: &str) -> bool {
    matches!(tool_name, "bash" | "write" | "edit")
}

/// Whether a tool name is read-only.
pub fn is_read_only(tool_name: &str) -> bool {
    matches!(tool_name, "read" | "grep" | "find" | "ls")
}

/// Gate that always grants every tool call.
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoPermissionGate;

impl PermissionGate for AutoPermissionGate {
    fn request(&self, _tool_call: &AgentToolCall) -> PermissionDecision {
        PermissionDecision::Grant
    }
}

/// Gate that always denies every tool call.
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyPermissionGate;

impl PermissionGate for DenyPermissionGate {
    fn request(&self, tool_call: &AgentToolCall) -> PermissionDecision {
        PermissionDecision::Deny {
            reason: format!("Tool '{}' is not permitted", tool_call.name),
        }
    }
}

/// Gate driven by a [`PermissionPolicy`] and an optional inner gate used for
/// interactive prompting when the policy is [`PermissionPolicy::Ask`].
///
/// Read-only tools are approved automatically unless `auto_approve_read_only` is
/// set to `false`. Dangerous tools follow the configured policy.
pub struct PolicyPermissionGate {
    policy: PermissionPolicy,
    auto_approve_read_only: bool,
    inner: Option<Arc<dyn PermissionGate>>,
    cache: Mutex<HashMap<String, PermissionDecision>>,
}

impl PolicyPermissionGate {
    /// Create a new policy gate.
    pub fn new(
        policy: PermissionPolicy,
        auto_approve_read_only: bool,
        inner: Option<Arc<dyn PermissionGate>>,
    ) -> Self {
        Self {
            policy,
            auto_approve_read_only,
            inner,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Create a policy gate that auto-approves everything.
    pub fn auto() -> Self {
        Self::new(PermissionPolicy::Auto, true, None)
    }

    /// Create a policy gate that denies everything.
    pub fn deny() -> Self {
        Self::new(PermissionPolicy::Deny, true, None)
    }

    /// Create a policy gate that asks via the provided inner gate.
    pub fn ask(inner: Arc<dyn PermissionGate>) -> Self {
        Self::new(PermissionPolicy::Ask, true, Some(inner))
    }
}

impl PermissionGate for PolicyPermissionGate {
    fn request(&self, tool_call: &AgentToolCall) -> PermissionDecision {
        // Check cached decision first.
        if let Ok(cache) = self.cache.lock() {
            if let Some(decision) = cache.get(&tool_call.name).cloned() {
                return decision;
            }
        }

        let decision = if is_read_only(&tool_call.name) {
            if self.auto_approve_read_only {
                PermissionDecision::Grant
            } else {
                match self.policy {
                    PermissionPolicy::Auto => PermissionDecision::Grant,
                    PermissionPolicy::Deny => PermissionDecision::Deny {
                        reason: format!("Read-only tool '{}' is denied by policy", tool_call.name),
                    },
                    PermissionPolicy::Ask => {
                        if let Some(inner) = &self.inner {
                            inner.request(tool_call)
                        } else {
                            PermissionDecision::Deny {
                                reason: format!(
                                    "Read-only tool '{}' requires approval but no prompt is available",
                                    tool_call.name
                                ),
                            }
                        }
                    }
                }
            }
        } else if is_dangerous(&tool_call.name) {
            match self.policy {
                PermissionPolicy::Auto => PermissionDecision::Grant,
                PermissionPolicy::Deny => PermissionDecision::Deny {
                    reason: format!("Dangerous tool '{}' is denied by policy", tool_call.name),
                },
                PermissionPolicy::Ask => {
                    if let Some(inner) = &self.inner {
                        inner.request(tool_call)
                    } else {
                        PermissionDecision::Deny {
                            reason: format!(
                                "Dangerous tool '{}' requires approval but no prompt is available",
                                tool_call.name
                            ),
                        }
                    }
                }
            }
        } else {
            // Unknown tools are treated as dangerous and require approval.
            PermissionDecision::Deny {
                reason: format!("Unknown tool '{}' is not permitted", tool_call.name),
            }
        };

        // Cache GrantAlways decisions for the tool name.
        if let PermissionDecision::GrantAlways = &decision {
            if let Ok(mut cache) = self.cache.lock() {
                cache.insert(tool_call.name.clone(), PermissionDecision::Grant);
            }
        }

        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_gate_grants() {
        let gate = AutoPermissionGate;
        let tc = AgentToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        assert_eq!(gate.request(&tc), PermissionDecision::Grant);
    }

    #[test]
    fn test_deny_gate_denies() {
        let gate = DenyPermissionGate;
        let tc = AgentToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        assert!(matches!(gate.request(&tc), PermissionDecision::Deny { .. }));
    }

    #[test]
    fn test_policy_auto() {
        let gate = PolicyPermissionGate::auto();
        let bash = AgentToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        let read = AgentToolCall {
            id: "2".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "x"}),
        };
        assert_eq!(gate.request(&bash), PermissionDecision::Grant);
        assert_eq!(gate.request(&read), PermissionDecision::Grant);
    }

    #[test]
    fn test_policy_deny() {
        let gate = PolicyPermissionGate::deny();
        let bash = AgentToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        assert!(matches!(
            gate.request(&bash),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_policy_ask_with_auto_read_only() {
        let gate = PolicyPermissionGate::ask(Arc::new(AutoPermissionGate));
        let read = AgentToolCall {
            id: "1".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "x"}),
        };
        let bash = AgentToolCall {
            id: "2".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        assert_eq!(gate.request(&read), PermissionDecision::Grant);
        assert_eq!(gate.request(&bash), PermissionDecision::Grant);
    }

    #[test]
    fn test_policy_ask_caches_always() {
        struct AlwaysGrant;
        impl PermissionGate for AlwaysGrant {
            fn request(&self, _tool_call: &AgentToolCall) -> PermissionDecision {
                PermissionDecision::GrantAlways
            }
        }

        let gate = PolicyPermissionGate::ask(Arc::new(AlwaysGrant));
        let bash = AgentToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        assert_eq!(gate.request(&bash), PermissionDecision::GrantAlways);
        // Second call should be a plain Grant from the cache.
        assert_eq!(gate.request(&bash), PermissionDecision::Grant);
    }
}
