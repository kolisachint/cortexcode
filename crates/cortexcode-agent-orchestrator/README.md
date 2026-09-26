# cortexcode-agent-orchestrator

`AgentHarness`: hoocode `packages/agent/src/harness/agent-harness.ts` (v0.5.89). It drives an
`Agent` (cortexcode-agent-core) against a harness `Session` (cortexcode-agent-session): every
turn rebuilds the context from the session, writes messages back as they end, and queues
session writes made mid-turn until a save point. It also runs compaction and tree navigation
(cortexcode-agent-compaction), invokes skills and prompt templates, and exposes hooks
(`context`, `tool_call`, `tool_result`, `before_provider_request`, `before_agent_start`,
`session_before_compact`, `session_before_tree`) and events.

It lives in its own crate because it sits above compaction, which depends on
cortexcode-agent-harness.
