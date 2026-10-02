//! Live Anthropic tests ported from hoocode (v0.5.89)
//! `anthropic-eager-tool-input-e2e.test.ts`,
//! `anthropic-long-cache-retention-e2e.test.ts`,
//! `anthropic-opus-4-7-smoke.test.ts`, `interleaved-thinking.test.ts`, the E2E
//! case of `anthropic-thinking-disable.test.ts` and
//! `anthropic-tool-name-normalization.test.ts`.
//!
//! Ignored by default; each test returns early (skip) without its key, like
//! `describe.skipIf`. Run with
//! `ANTHROPIC_API_KEY=... cargo test -p cortexcode-ai-provider-anthropic --test live_e2e -- --ignored`.
//! The github-copilot cases need a Copilot session token (`resolveApiKey`
//! from OAuth storage) and are left to 8.4b.

use cortexcode_ai_provider_anthropic::{stream, stream_anthropic, AnthropicOptions};
use cortexcode_ai_types::{
    AssistantMessage, AssistantMessageEvent, CacheRetention, Content, Context, Message, Model,
    SimpleStreamOptions, StopReason, ThinkingLevel, Tool, ToolResultMessage, UserMessage,
};
use futures_util::StreamExt;
use serde_json::json;

fn key(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: text.into(),
        timestamp: cortexcode_ai_types::now_ms(),
    })
}

fn model(id: &str) -> Model {
    cortexcode_ai_models::get_model("anthropic", id)
        .unwrap_or_else(|| panic!("anthropic/{id} in catalog"))
        .clone()
}

fn text_of(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// `getProbePriority`.
fn probe_priority(model: &Model) -> f64 {
    let id = model.id.to_lowercase();
    let mut priority = model.cost.input + model.cost.output;
    if id.contains("haiku") && (id.contains("4-5") || id.contains("4.5")) {
        priority -= 1000.0;
    } else if id.contains("sonnet") && (id.contains("4-") || id.contains("4.")) {
        priority -= 750.0;
    } else if id.contains("claude") && (id.contains("4-") || id.contains("4.")) {
        priority -= 500.0;
    }
    priority
}

/// `selectOneCasePerProvider` over every anthropic-messages model, for the
/// providers whose key comes from the environment.
fn probe_cases(filter: impl Fn(&Model) -> bool) -> Vec<(Model, String)> {
    let mut cases = Vec::new();
    for provider in cortexcode_ai_models::get_providers() {
        if provider == "github-copilot" {
            continue;
        }
        let mut models: Vec<&Model> = cortexcode_ai_models::get_models(provider)
            .into_iter()
            .filter(|m| m.api == "anthropic-messages" && filter(m))
            .collect();
        models.sort_by(|a, b| {
            probe_priority(a)
                .total_cmp(&probe_priority(b))
                .then_with(|| a.id.cmp(&b.id))
        });
        if let (Some(model), Some(api_key)) =
            (models.first(), cortexcode_ai_env::get_env_api_key(provider))
        {
            cases.push(((*model).clone(), api_key));
        }
    }
    cases
}

fn with_compat(mut model: Model, key: &str) -> Model {
    let mut compat = model.compat.take().unwrap_or_else(|| json!({}));
    compat[key] = json!(true);
    model.compat = Some(compat);
    model
}

async fn complete(model: Model, context: Context, options: AnthropicOptions) -> AssistantMessage {
    stream_anthropic(model, context, options).result().await
}

async fn expect_accepted(
    model: Model,
    api_key: String,
    tools: Vec<Tool>,
    prompt: &str,
    retention: Option<CacheRetention>,
) {
    let name = format!("{}/{}", model.provider, model.id);
    let response = complete(
        model,
        Context::new(
            "You are a concise assistant. Use tools when useful.".into(),
            vec![user(prompt)],
            tools,
        ),
        AnthropicOptions {
            api_key: Some(api_key),
            max_tokens: Some(128),
            thinking_enabled: Some(false),
            cache_retention: retention,
            ..Default::default()
        },
    )
    .await;
    assert!(
        response.error_message.is_none(),
        "{name}: {:?}",
        response.error_message
    );
    assert_ne!(response.stop_reason, StopReason::Error, "{name}");
}

fn echo_tool() -> Tool {
    Tool {
        name: "echo_value".into(),
        description: "Echo a string value".into(),
        parameters: json!({
            "type": "object",
            "properties": {"value": {"type": "string", "description": "The value to echo"}},
            "required": ["value"],
        }),
        defer_loading: None,
    }
}

// --- anthropic-eager-tool-input-e2e.test.ts ---

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn eager_tool_input_generated_compat_settings_are_accepted() {
    for (model, api_key) in probe_cases(|_| true) {
        expect_accepted(
            model,
            api_key,
            vec![echo_tool()],
            "Call echo_value with value set to eager-input-streaming-compat.",
            None,
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn forced_eager_input_streaming_is_accepted() {
    let eager = |m: &Model| {
        m.compat_as::<cortexcode_ai_types::AnthropicMessagesCompat>()
            .supports_eager_tool_input_streaming
            != Some(false)
    };
    for (model, api_key) in probe_cases(eager) {
        expect_accepted(
            with_compat(model, "supportsEagerToolInputStreaming"),
            api_key,
            vec![echo_tool()],
            "Call echo_value with value set to eager-input-streaming-compat.",
            None,
        )
        .await;
    }
}

// --- anthropic-long-cache-retention-e2e.test.ts ---

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn forced_long_cache_retention_is_accepted() {
    for (model, api_key) in probe_cases(|_| true) {
        expect_accepted(
            with_compat(model, "supportsLongCacheRetention"),
            api_key,
            vec![],
            "Reply with exactly: long cache retention accepted",
            Some(CacheRetention::Long),
        )
        .await;
    }
}

// --- anthropic-opus-4-7-smoke.test.ts ---

#[tokio::test]
#[ignore = "live: needs ANTHROPIC_API_KEY and network"]
async fn opus_4_7_streams_with_reasoning_enabled() {
    let Some(api_key) = key("ANTHROPIC_API_KEY") else {
        return;
    };
    let context = Context::new(
        "You are a precise assistant. Follow the user's instructions exactly.".into(),
        vec![user("Compute 48291 * 7317 and 90844 - 17729, add the results, and determine whether the sum is divisible by 11. Reply with exactly this format and nothing else: sum=<sum>; divisibleBy11=<yes|no>")],
        vec![],
    );
    // The TS test asserts the payload's `thinking` is `{type: "adaptive"}`;
    // buildParams at the pin sends `display: "summarized"` too (see the
    // thinking-disable payload tests), so that check is not repeated here.
    let mut s = stream(
        model("claude-opus-4-7"),
        context,
        SimpleStreamOptions {
            api_key: Some(api_key),
            reasoning: Some(ThinkingLevel::High),
            max_tokens: Some(1024),
            ..Default::default()
        },
    )
    .unwrap();
    let mut saw_thinking = false;
    while let Some(event) = s.next().await {
        if matches!(
            event,
            AssistantMessageEvent::ThinkingStart { .. }
                | AssistantMessageEvent::ThinkingDelta { .. }
                | AssistantMessageEvent::ThinkingEnd { .. }
        ) {
            saw_thinking = true;
        }
    }
    let response = s.result().await;
    assert_eq!(
        response.stop_reason,
        StopReason::Stop,
        "{:?}",
        response.error_message
    );
    assert!(saw_thinking);
    let signature = response.content.iter().find_map(|c| match c {
        Content::Thinking(t) => t.signature.clone(),
        _ => None,
    });
    assert!(signature.is_some_and(|s| !s.is_empty()));
    assert_eq!(text_of(&response), "sum=353418362; divisibleBy11=yes");
}

// --- interleaved-thinking.test.ts ---

fn calculator_tool() -> Tool {
    Tool {
        name: "calculator".into(),
        description: "Perform basic arithmetic operations".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "a": {"type": "number", "description": "First number"},
                "b": {"type": "number", "description": "Second number"},
                "operation": {"type": "string", "enum": ["add", "subtract", "multiply", "divide"], "description": "The operation to perform."},
            },
            "required": ["a", "b", "operation"],
        }),
        defer_loading: None,
    }
}

async fn assert_second_tool_call_with_interleaved_thinking(llm: Model, api_key: &str) {
    let mut context = Context::new(
        "You are a helpful assistant that must use tools for arithmetic. Always think before every tool call, not just the first one. Do not answer with plain text when a tool call is required.".into(),
        vec![user("Use calculator to calculate 328 * 29. You must call the calculator tool exactly once. Provide the final answer based on the best guess given the tool result, even if it seems unreliable. Start by thinking about the steps you will take to solve the problem.")],
        vec![calculator_tool()],
    );
    let options = || SimpleStreamOptions {
        api_key: Some(api_key.to_string()),
        reasoning: Some(ThinkingLevel::High),
        ..Default::default()
    };
    let first = stream(llm.clone(), context.clone(), options())
        .unwrap()
        .result()
        .await;
    assert_eq!(
        first.stop_reason,
        StopReason::ToolUse,
        "{:?}",
        first.error_message
    );
    assert!(first
        .content
        .iter()
        .any(|c| matches!(c, Content::Thinking(_))));
    let call = first
        .content
        .iter()
        .find_map(|c| match c {
            Content::ToolCall(call) => Some(call.clone()),
            _ => None,
        })
        .expect("a tool call");
    let (a, b) = (
        call.arguments["a"].as_f64().unwrap(),
        call.arguments["b"].as_f64().unwrap(),
    );
    let answer = match call.arguments["operation"].as_str().unwrap() {
        "add" => a + b,
        "subtract" => a - b,
        "multiply" => a * b,
        _ => a / b,
    };
    context.messages.push(Message::Assistant(first));
    context
        .messages
        .push(Message::ToolResult(ToolResultMessage {
            tool_call_id: call.id,
            tool_name: call.name,
            content: vec![Content::text(format!(
                "The answer is {answer} or {}.",
                answer * 2.0
            ))],
            details: None,
            is_error: false,
            timestamp: cortexcode_ai_types::now_ms(),
        }));
    let second = stream(llm, context, options()).unwrap().result().await;
    assert_eq!(
        second.stop_reason,
        StopReason::Stop,
        "{:?}",
        second.error_message
    );
    assert!(second
        .content
        .iter()
        .any(|c| matches!(c, Content::Thinking(_))));
    assert!(second.content.iter().any(|c| matches!(c, Content::Text(_))));
}

#[tokio::test]
#[ignore = "live: needs Anthropic credentials and network"]
async fn interleaved_thinking_on_opus_4_5_and_4_6() {
    let Some(api_key) = cortexcode_ai_env::get_env_api_key("anthropic") else {
        return;
    };
    for id in ["claude-opus-4-5", "claude-opus-4-6"] {
        assert_second_tool_call_with_interleaved_thinking(model(id), &api_key).await;
    }
}

// --- anthropic-thinking-disable.test.ts: E2E ---

#[tokio::test]
#[ignore = "live: needs ANTHROPIC_API_KEY and network"]
async fn thinking_off_disables_thinking_for_claude_reasoning_models() {
    let Some(api_key) = key("ANTHROPIC_API_KEY") else {
        return;
    };
    let context = Context::new(
        "You are a precise assistant. Follow the requested output format exactly.".into(),
        vec![user("Before replying, carefully solve 36863 * 5279 internally. Then reply with the word pong repeated exactly 40 times, separated by single spaces. Do not add any other text.")],
        vec![],
    );
    let mut s = stream(
        model("claude-sonnet-4-5"),
        context,
        SimpleStreamOptions {
            api_key: Some(api_key),
            temperature: Some(0.0),
            max_tokens: Some(160),
            ..Default::default()
        },
    )
    .unwrap();
    let mut thinking_events = 0;
    while let Some(event) = s.next().await {
        if matches!(
            event,
            AssistantMessageEvent::ThinkingStart { .. }
                | AssistantMessageEvent::ThinkingDelta { .. }
                | AssistantMessageEvent::ThinkingEnd { .. }
        ) {
            thinking_events += 1;
        }
    }
    let response = s.result().await;
    assert_eq!(
        response.stop_reason,
        StopReason::Stop,
        "{:?}",
        response.error_message
    );
    assert_eq!(thinking_events, 0);
    assert!(!response
        .content
        .iter()
        .any(|c| matches!(c, Content::Thinking(_))));
    let pongs = text_of(&response)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.eq_ignore_ascii_case("pong"))
        .count();
    assert!(pongs >= 35, "{pongs} pongs");
}

// --- anthropic-tool-name-normalization.test.ts ---

/// The tool-call name the model used, as seen at `toolcall_end`.
async fn oauth_tool_call_name(token: &str, tool: Tool, system: &str, prompt: &str) -> String {
    let context = Context::new(system.into(), vec![user(prompt)], vec![tool]);
    let mut s = stream(
        model("claude-sonnet-4-6"),
        context,
        SimpleStreamOptions {
            api_key: Some(token.to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    let mut name = None;
    while let Some(event) = s.next().await {
        if let AssistantMessageEvent::ToolCallEnd { index, partial } = event {
            if let Some(Content::ToolCall(call)) = partial.content.get(index) {
                name = Some(call.name.clone());
            }
        }
    }
    let response = s.result().await;
    assert_eq!(
        response.stop_reason,
        StopReason::ToolUse,
        "{:?}",
        response.error_message
    );
    name.expect("a tool call")
}

fn one_arg_tool(name: &str, description: &str, arg: &str, arg_description: &str) -> Tool {
    Tool {
        name: name.into(),
        description: description.into(),
        parameters: json!({
            "type": "object",
            "properties": {arg: {"type": "string", "description": arg_description}},
            "required": [arg],
        }),
        defer_loading: None,
    }
}

#[tokio::test]
#[ignore = "live: needs ANTHROPIC_OAUTH_TOKEN and network"]
async fn oauth_tool_names_round_trip() {
    let Some(token) = key("ANTHROPIC_OAUTH_TOKEN") else {
        return;
    };
    let cases = [
        (
            one_arg_tool("todowrite", "Write a todo item", "task", "The task to add"),
            "You are a helpful assistant. Use the todowrite tool when asked to add todos.",
            "Add a todo: buy milk. Use the todowrite tool.",
            "todowrite",
        ),
        (
            one_arg_tool("read", "Read a file", "path", "File path"),
            "You are a helpful assistant. Use the read tool to read files.",
            "Read the file /tmp/test.txt using the read tool.",
            "read",
        ),
        (
            one_arg_tool("find", "Find files by pattern", "pattern", "Glob pattern"),
            "You are a helpful assistant. Use the find tool to search for files.",
            "Find all .ts files using the find tool.",
            "find",
        ),
        (
            one_arg_tool("my_custom_tool", "A custom tool", "input", "Input value"),
            "You are a helpful assistant. Use my_custom_tool when asked.",
            "Use my_custom_tool with input 'hello'.",
            "my_custom_tool",
        ),
    ];
    for (tool, system, prompt, expected) in cases {
        assert_eq!(
            oauth_tool_call_name(&token, tool, system, prompt).await,
            expected
        );
    }
}

// --- "covers every generated anthropic-messages model" (no key needed) ---

#[test]
fn probe_cases_cover_one_model_per_provider() {
    for provider in cortexcode_ai_models::get_providers() {
        let models: Vec<_> = cortexcode_ai_models::get_models(provider)
            .into_iter()
            .filter(|m| m.api == "anthropic-messages")
            .collect();
        if provider == "anthropic" {
            assert!(!models.is_empty());
            // haiku 4.5 is the cheapest probe.
            let best = models
                .iter()
                .min_by(|a, b| probe_priority(a).total_cmp(&probe_priority(b)))
                .unwrap();
            assert!(best.id.contains("haiku-4-5"), "{}", best.id);
        }
    }
}
