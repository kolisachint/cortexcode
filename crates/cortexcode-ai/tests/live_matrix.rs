//! Live cross-provider suites ported from hoocode (v0.5.89) `packages/ai/test/`:
//! `context-overflow`, `empty`, `image-tool-result`, `responseid`, `tokens`,
//! `total-tokens`, `tool-call-id-normalization`, `tool-call-without-result`,
//! `unicode-surrogate`, `xhigh` and `zen`.
//!
//! Every test is `#[ignore]`d and runs only the provider cases whose key is
//! in the environment (TS `describe.skipIf`). Run with e.g.
//! `OPENROUTER_API_KEY=... cargo test -p cortexcode-ai --test live_matrix -- --ignored`.
//!
//! Not covered here: the github-copilot / openai-codex cases (their APIs and
//! OAuth come with 8.4a/8.4b/8.7), Anthropic OAuth tokens from auth storage
//! (`ANTHROPIC_OAUTH_TOKEN` is used instead), and the local Ollama / LM Studio
//! / llama.cpp overflow cases that start or probe local servers.

use cortexcode_ai_registry::{complete_simple, stream_simple};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, AssistantMessageEvent, Content, Context, ImageContent, Message,
    Model, SimpleStreamOptions, StopReason, TextContent, Tool, ToolCallContent, ToolResultMessage,
    Usage, UserMessage,
};
use futures_util::StreamExt;
use serde_json::json;

// ---------------------------------------------------------------------------
// Cases and helpers
// ---------------------------------------------------------------------------

/// One `describe.skipIf(!key)` block: provider, model, key variable and
/// whether the TS test forces `api: "openai-completions"`.
#[derive(Clone, Copy)]
struct Case {
    provider: &'static str,
    model: &'static str,
    key: &'static str,
    completions: bool,
}

const fn case(provider: &'static str, model: &'static str, key: &'static str) -> Case {
    Case {
        provider,
        model,
        key,
        completions: false,
    }
}

const fn completions(provider: &'static str, model: &'static str, key: &'static str) -> Case {
    Case {
        provider,
        model,
        key,
        completions: true,
    }
}

/// `hasAzureOpenAICredentials`.
fn azure_configured() -> bool {
    env("AZURE_OPENAI_API_KEY").is_some()
        && (env("AZURE_OPENAI_BASE_URL").is_some() || env("AZURE_OPENAI_RESOURCE_NAME").is_some())
}

fn env(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

impl Case {
    /// The model and API key, or `None` to skip.
    fn resolve(&self) -> Option<(Model, String)> {
        if self.provider == "azure-openai-responses" && !azure_configured() {
            return None;
        }
        let key = env(self.key)?;
        let mut model = cortexcode_ai_models::get_model(self.provider, self.model)
            .unwrap_or_else(|| panic!("{}/{} in catalog", self.provider, self.model))
            .clone();
        if self.completions {
            // `const { compat: _compat, ...baseModel } = getModel(...)`.
            model.compat = None;
            model.api = "openai-completions".into();
        }
        Some((model, key))
    }
}

/// The API-key providers most suites share (after openai/azure/anthropic).
const SHARED: &[Case] = &[
    case("xai", "grok-code-fast-1", "XAI_API_KEY"),
    case("groq", "openai/gpt-oss-20b", "GROQ_API_KEY"),
    case("cerebras", "gpt-oss-120b", "CEREBRAS_API_KEY"),
    case("nvidia", "meta/llama-3.3-70b-instruct", "NVIDIA_API_KEY"),
    case("huggingface", "moonshotai/Kimi-K2.5", "HF_TOKEN"),
    case("together", "moonshotai/Kimi-K2.6", "TOGETHER_API_KEY"),
    case("zai", "glm-5.2", "ZAI_API_KEY"),
    case("minimax", "MiniMax-M2.7", "MINIMAX_API_KEY"),
    case("xiaomi", "mimo-v2.5-pro", "XIAOMI_API_KEY"),
    case(
        "xiaomi-token-plan-cn",
        "mimo-v2.5-pro",
        "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    ),
    case(
        "xiaomi-token-plan-ams",
        "mimo-v2.5-pro",
        "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    ),
    case(
        "xiaomi-token-plan-sgp",
        "mimo-v2.5-pro",
        "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    ),
    case("kimi-coding", "kimi-for-coding", "KIMI_API_KEY"),
    case(
        "vercel-ai-gateway",
        "google/gemini-2.5-flash",
        "AI_GATEWAY_API_KEY",
    ),
];

fn cases(head: &[Case], tail: &[Case]) -> Vec<Case> {
    head.iter().chain(tail).copied().collect()
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: text.into(),
        timestamp: cortexcode_ai_types::now_ms(),
    })
}

fn options(api_key: &str) -> SimpleStreamOptions {
    SimpleStreamOptions {
        api_key: Some(api_key.to_string()),
        ..Default::default()
    }
}

async fn complete(model: &Model, context: &Context, api_key: &str) -> AssistantMessage {
    complete_simple(model.clone(), context.clone(), options(api_key))
        .await
        .unwrap_or_else(|e| panic!("{}/{}: {e}", model.provider, model.id))
}

fn text_of(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn label(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

/// An assistant turn that called `name` with `{}` (the unicode suites).
fn prior_tool_call(model: &Model, id: &str, name: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::ToolCall(ToolCallContent {
            id: id.into(),
            name: name.into(),
            arguments: json!({}),
            thought_signature: None,
        })],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        stop_reason: StopReason::ToolUse,
        timestamp: cortexcode_ai_types::now_ms(),
        ..Default::default()
    })
}

fn tool_result(id: &str, name: &str, content: Vec<Content>) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: name.into(),
        content,
        details: None,
        is_error: false,
        timestamp: cortexcode_ai_types::now_ms(),
    })
}

fn tool(name: &str, description: &str, parameters: serde_json::Value) -> Tool {
    Tool {
        name: name.into(),
        description: description.into(),
        parameters,
        defer_loading: None,
    }
}

fn empty_schema() -> serde_json::Value {
    json!({"type": "object", "properties": {}})
}

/// An error response must say why; anything else must have content.
fn expect_answered(model: &Model, response: &AssistantMessage) {
    if response.stop_reason == StopReason::Error {
        assert!(response.error_message.is_some(), "{}", label(model));
    }
}

// ---------------------------------------------------------------------------
// context-overflow.test.ts
// ---------------------------------------------------------------------------

const LOREM_IPSUM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum. ";

/// `generateOverflowContent`: 10k tokens past the window (4 chars/token, x1.5).
fn overflow_content(context_window: u64) -> String {
    let target_chars = (context_window + 10_000) as f64 * 4.0 * 1.5;
    LOREM_IPSUM.repeat((target_chars / LOREM_IPSUM.len() as f64).ceil() as usize)
}

/// How a provider reports the overflow.
enum Overflow {
    /// `stopReason: "error"`, with a message matching the regex when given.
    Error(Option<&'static str>),
    /// Xiaomi: `stopReason: "length"` with no output.
    Length,
    /// z.ai: error, or success with `usage.input > contextWindow`.
    Zai,
}

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn context_overflow_is_detected() {
    use Overflow::*;
    let table: Vec<(Case, Overflow)> = vec![
        (
            case("anthropic", "claude-haiku-4-5", "ANTHROPIC_API_KEY"),
            Error(Some("(?i)prompt is too long")),
        ),
        (
            case("anthropic", "claude-sonnet-4-6", "ANTHROPIC_OAUTH_TOKEN"),
            Error(Some("(?i)prompt is too long")),
        ),
        (
            case("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
            Error(Some("(?i)maximum context length")),
        ),
        (
            case("openai", "gpt-4o", "OPENAI_API_KEY"),
            Error(Some("(?i)exceeds the context window")),
        ),
        (
            case(
                "azure-openai-responses",
                "gpt-4o-mini",
                "AZURE_OPENAI_API_KEY",
            ),
            Error(Some("(?i)context|maximum")),
        ),
        (
            case("google", "gemini-2.5-flash", "GEMINI_API_KEY"),
            Error(Some("(?i)input token count.*exceeds the maximum")),
        ),
        (
            case("xai", "grok-code-fast-1", "XAI_API_KEY"),
            Error(Some(r"(?i)maximum prompt length is \d+")),
        ),
        (
            case("groq", "llama-3.3-70b-versatile", "GROQ_API_KEY"),
            Error(Some("(?i)reduce the length of the messages")),
        ),
        (
            case("cerebras", "gpt-oss-120b", "CEREBRAS_API_KEY"),
            Error(Some(r"(?i)4(00|13|29).*\(no body\)")),
        ),
        (
            case("nvidia", "meta/llama-3.3-70b-instruct", "NVIDIA_API_KEY"),
            Error(None),
        ),
        (
            case("huggingface", "moonshotai/Kimi-K2.5", "HF_TOKEN"),
            Error(None),
        ),
        (
            case("together", "moonshotai/Kimi-K2.6", "TOGETHER_API_KEY"),
            Error(None),
        ),
        (case("zai", "glm-5.2", "ZAI_API_KEY"), Zai),
        (
            case("minimax", "MiniMax-M2.7", "MINIMAX_API_KEY"),
            Error(None),
        ),
        (case("xiaomi", "mimo-v2.5-pro", "XIAOMI_API_KEY"), Length),
        (
            case(
                "xiaomi-token-plan-cn",
                "mimo-v2.5-pro",
                "XIAOMI_TOKEN_PLAN_CN_API_KEY",
            ),
            Length,
        ),
        (
            case(
                "xiaomi-token-plan-ams",
                "mimo-v2.5-pro",
                "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
            ),
            Length,
        ),
        (
            case(
                "xiaomi-token-plan-sgp",
                "mimo-v2.5-pro",
                "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
            ),
            Length,
        ),
        (
            case("kimi-coding", "kimi-for-coding", "KIMI_API_KEY"),
            Error(None),
        ),
        (
            case(
                "vercel-ai-gateway",
                "google/gemini-2.5-flash",
                "AI_GATEWAY_API_KEY",
            ),
            Error(None),
        ),
        (
            case(
                "openrouter",
                "anthropic/claude-sonnet-4",
                "OPENROUTER_API_KEY",
            ),
            Error(Some(r"(?i)maximum context length is \d+ tokens")),
        ),
        (
            case("openrouter", "deepseek/deepseek-v3.2", "OPENROUTER_API_KEY"),
            Error(Some(r"(?i)maximum context length is \d+ tokens")),
        ),
        (
            case(
                "openrouter",
                "mistralai/mistral-medium-3.1",
                "OPENROUTER_API_KEY",
            ),
            Error(Some(r"(?i)maximum context length is \d+ tokens")),
        ),
        (
            case(
                "openrouter",
                "google/gemini-2.5-flash",
                "OPENROUTER_API_KEY",
            ),
            Error(Some(r"(?i)maximum context length is \d+ tokens")),
        ),
        (
            case(
                "openrouter",
                "meta-llama/llama-4-scout",
                "OPENROUTER_API_KEY",
            ),
            Error(Some(r"(?i)maximum context length is \d+ tokens")),
        ),
    ];
    for (case, expect) in table {
        let Some((model, key)) = case.resolve() else {
            continue;
        };
        let context = Context::new(
            "You are a helpful assistant.".into(),
            vec![user(&overflow_content(model.context_window))],
            vec![],
        );
        let response = complete(&model, &context, &key).await;
        let overflow =
            cortexcode_ai_util::is_context_overflow(&response, Some(model.context_window));
        match expect {
            Error(pattern) => {
                assert_eq!(response.stop_reason, StopReason::Error, "{}", label(&model));
                if let Some(pattern) = pattern {
                    let message = response.error_message.clone().unwrap_or_default();
                    assert!(
                        regex::Regex::new(pattern).unwrap().is_match(&message),
                        "{}: {message}",
                        label(&model)
                    );
                }
                assert!(overflow, "{}", label(&model));
            }
            Length => {
                assert_eq!(
                    response.stop_reason,
                    StopReason::Length,
                    "{}",
                    label(&model)
                );
                assert_eq!(response.usage.output, 0);
                assert!(overflow, "{}", label(&model));
            }
            Zai => {
                let message = response.error_message.clone().unwrap_or_default();
                let has_usage = response.usage.input > 0 || response.usage.cache_read > 0;
                let reported = (response.stop_reason == StopReason::Error
                    && message
                        .to_lowercase()
                        .contains("model_context_window_exceeded"))
                    || (response.stop_reason == StopReason::Stop
                        && has_usage
                        && response.usage.input > model.context_window);
                if reported {
                    assert!(overflow, "{}", label(&model));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// empty.test.ts
// ---------------------------------------------------------------------------

fn empty_cases() -> Vec<Case> {
    cases(
        &[
            case("google", "gemini-2.5-flash", "GEMINI_API_KEY"),
            case("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
            case("openai", "gpt-5-mini", "OPENAI_API_KEY"),
            case(
                "azure-openai-responses",
                "gpt-4o-mini",
                "AZURE_OPENAI_API_KEY",
            ),
            case("anthropic", "claude-haiku-4-5", "ANTHROPIC_API_KEY"),
            case("anthropic", "claude-haiku-4-5", "ANTHROPIC_OAUTH_TOKEN"),
        ],
        SHARED,
    )
}

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn empty_messages_are_handled() {
    for case in empty_cases() {
        let Some((model, key)) = case.resolve() else {
            continue;
        };
        let empty_content = Message::User(UserMessage {
            content: Vec::new().into(),
            timestamp: cortexcode_ai_types::now_ms(),
        });
        let empty_assistant = Message::Assistant(AssistantMessage {
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            usage: Usage {
                input: 10,
                total_tokens: 10,
                ..Default::default()
            },
            timestamp: cortexcode_ai_types::now_ms(),
            ..Default::default()
        });
        let contexts = [
            vec![empty_content],
            vec![user("")],
            vec![user("   \n\t  ")],
            vec![
                user("Hello, how are you?"),
                empty_assistant,
                user("Please respond this time."),
            ],
        ];
        for messages in contexts {
            let response =
                complete(&model, &Context::new(String::new(), messages, vec![]), &key).await;
            expect_answered(&model, &response);
        }
    }
}

// ---------------------------------------------------------------------------
// image-tool-result.test.ts
// ---------------------------------------------------------------------------

/// `test/data/red-circle.png` from the pinned hoocode.
const RED_CIRCLE_PNG: &[u8] = include_bytes!("data/red-circle.png");

fn red_circle() -> Content {
    use base64::Engine;
    Content::Image(ImageContent {
        data: base64::engine::general_purpose::STANDARD.encode(RED_CIRCLE_PNG),
        media_type: "image/png".into(),
    })
}

/// `handleToolWithImageResult` / `handleToolWithTextAndImageResult`.
async fn tool_with_image_result(model: &Model, key: &str, with_text: bool) {
    if !model.input.iter().any(|i| i == "image") {
        return;
    }
    let (name, description, prompt) = if with_text {
        (
            "get_circle_with_description",
            "Returns a circle image with a text description",
            "Use the get_circle_with_description tool and tell me what you learned. Also say what color the shape is.",
        )
    } else {
        (
            "get_circle",
            "Returns a circle image for visualization",
            "Call the get_circle tool to get an image, and describe what you see, shapes, colors, etc.",
        )
    };
    let mut context = Context::new(
        "You are a helpful assistant that uses tools when asked.".into(),
        vec![user(prompt)],
        vec![tool(
            name,
            description,
            json!({"type": "object", "properties": {}}),
        )],
    );
    let first = complete(model, &context, key).await;
    assert_eq!(
        first.stop_reason,
        StopReason::ToolUse,
        "{}: {:?}",
        label(model),
        first.error_message
    );
    let call = first
        .content
        .iter()
        .find_map(|c| match c {
            Content::ToolCall(call) => Some(call.clone()),
            _ => None,
        })
        .expect("tool call");
    assert_eq!(call.name, name);
    context.messages.push(Message::Assistant(first));
    let mut content = Vec::new();
    if with_text {
        content.push(Content::Text(TextContent::new(
            "This is a geometric shape with specific properties: it has a diameter of 100 pixels.",
        )));
    }
    content.push(red_circle());
    context
        .messages
        .push(tool_result(&call.id, &call.name, content));

    let second = complete(model, &context, key).await;
    assert_eq!(
        second.stop_reason,
        StopReason::Stop,
        "{}: {:?}",
        label(model),
        second.error_message
    );
    let text = text_of(&second).to_lowercase();
    if with_text {
        assert!(
            ["diameter", "100", "pixel"]
                .iter()
                .any(|w| text.contains(w)),
            "{text}"
        );
    }
    assert!(text.contains("red") && text.contains("circle"), "{text}");
}

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn tool_results_with_images_reach_the_model() {
    let table = [
        case("google", "gemini-2.5-flash", "GEMINI_API_KEY"),
        completions("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
        case("openai", "gpt-5-mini", "OPENAI_API_KEY"),
        case(
            "azure-openai-responses",
            "gpt-4o-mini",
            "AZURE_OPENAI_API_KEY",
        ),
        case("anthropic", "claude-haiku-4-5", "ANTHROPIC_API_KEY"),
        case("openrouter", "z-ai/glm-4.5v", "OPENROUTER_API_KEY"),
        case("together", "moonshotai/Kimi-K2.6", "TOGETHER_API_KEY"),
        case("xiaomi", "mimo-v2.5-pro", "XIAOMI_API_KEY"),
        case(
            "xiaomi-token-plan-cn",
            "mimo-v2.5-pro",
            "XIAOMI_TOKEN_PLAN_CN_API_KEY",
        ),
        case(
            "xiaomi-token-plan-ams",
            "mimo-v2.5-pro",
            "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
        ),
        case(
            "xiaomi-token-plan-sgp",
            "mimo-v2.5-pro",
            "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
        ),
        case("kimi-coding", "kimi-for-coding", "KIMI_API_KEY"),
        case(
            "vercel-ai-gateway",
            "google/gemini-2.5-flash",
            "AI_GATEWAY_API_KEY",
        ),
        case("anthropic", "claude-sonnet-4-5", "ANTHROPIC_OAUTH_TOKEN"),
    ];
    for case in table {
        let Some((model, key)) = case.resolve() else {
            continue;
        };
        tool_with_image_result(&model, &key, false).await;
        tool_with_image_result(&model, &key, true).await;
    }
}

// ---------------------------------------------------------------------------
// responseid.test.ts
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn responses_expose_a_response_id() {
    let table = [
        case("google", "gemini-2.5-flash", "GEMINI_API_KEY"),
        case(
            "google-vertex",
            "gemini-3-flash-preview",
            "GOOGLE_CLOUD_API_KEY",
        ),
        completions("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
        case("openai", "gpt-5-mini", "OPENAI_API_KEY"),
        case("anthropic", "claude-sonnet-4-5", "ANTHROPIC_API_KEY"),
        case(
            "azure-openai-responses",
            "gpt-4o-mini",
            "AZURE_OPENAI_API_KEY",
        ),
    ];
    for case in table {
        let Some((model, key)) = case.resolve() else {
            continue;
        };
        let context = Context::new(
            "You are a helpful assistant. Be concise.".into(),
            vec![user("Reply with exactly: response id test")],
            vec![],
        );
        let response = complete(&model, &context, &key).await;
        assert_ne!(
            response.stop_reason,
            StopReason::Error,
            "{}: {:?}",
            label(&model),
            response.error_message
        );
        assert!(
            response.response_id.is_some_and(|id| !id.is_empty()),
            "{}",
            label(&model)
        );
    }
}

// ---------------------------------------------------------------------------
// tokens.test.ts
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn token_usage_on_abort() {
    let table = cases(
        &[
            case("google", "gemini-2.5-flash", "GEMINI_API_KEY"),
            completions("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
            case("openai", "gpt-5.4-mini", "OPENAI_API_KEY"),
            case(
                "azure-openai-responses",
                "gpt-4o-mini",
                "AZURE_OPENAI_API_KEY",
            ),
            case("anthropic", "claude-sonnet-4-6", "ANTHROPIC_API_KEY"),
            case("anthropic", "claude-sonnet-4-6", "ANTHROPIC_OAUTH_TOKEN"),
        ],
        SHARED,
    );
    for case in table {
        let Some((model, key)) = case.resolve() else {
            continue;
        };
        let context = Context::new(
            "You are a helpful assistant.".into(),
            vec![user(
                "Write a long poem with 20 stanzas about the beauty of nature.",
            )],
            vec![],
        );
        let signal = AbortSignal::new();
        let mut stream = stream_simple(
            model.clone(),
            context,
            SimpleStreamOptions {
                signal: Some(signal.clone()),
                ..options(&key)
            },
        )
        .unwrap();
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            if let AssistantMessageEvent::TextDelta { delta, .. }
            | AssistantMessageEvent::ThinkingDelta { delta, .. } = &event
            {
                if !signal.aborted() {
                    text.push_str(delta);
                    if text.len() >= 1000 {
                        signal.abort();
                    }
                }
            }
        }
        let msg = stream.result().await;
        assert_eq!(msg.stop_reason, StopReason::Aborted, "{}", label(&model));
        let final_usage_only = matches!(
            model.api.as_str(),
            "openai-completions"
                | "openai-responses"
                | "azure-openai-responses"
                | "openai-codex-responses"
        ) || matches!(
            model.provider.as_str(),
            "zai" | "vercel-ai-gateway" | "minimax"
        );
        if final_usage_only {
            assert_eq!(
                (msg.usage.input, msg.usage.output),
                (0, 0),
                "{}",
                label(&model)
            );
        } else if model.provider == "kimi-coding" {
            assert!(msg.usage.input > 0);
            assert_eq!(msg.usage.output, 0);
        } else {
            assert!(
                msg.usage.input > 0 && msg.usage.output > 0,
                "{}",
                label(&model)
            );
            if model.cost.input > 0.0 {
                assert!(msg.usage.cost.input > 0.0 && msg.usage.cost.total > 0.0);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// total-tokens.test.ts
// ---------------------------------------------------------------------------

fn long_system_prompt() -> String {
    format!(
        "You are a helpful assistant. Be concise in your responses.\n\nHere is some additional context that makes this system prompt long enough to trigger caching:\n\n{}\n\nRemember: Always be helpful and concise.",
        vec!["Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris."; 50].join("\n\n")
    )
}

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn total_tokens_equal_the_sum_of_components() {
    let table = cases(
        &[
            case("anthropic", "claude-sonnet-4-5", "ANTHROPIC_API_KEY"),
            case("anthropic", "claude-sonnet-4-6", "ANTHROPIC_OAUTH_TOKEN"),
            completions("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
            case("openai", "gpt-4o", "OPENAI_API_KEY"),
            case(
                "azure-openai-responses",
                "gpt-4o-mini",
                "AZURE_OPENAI_API_KEY",
            ),
            case("google", "gemini-2.5-flash", "GEMINI_API_KEY"),
            case("groq", "openai/gpt-oss-120b", "GROQ_API_KEY"),
            case(
                "openrouter",
                "anthropic/claude-sonnet-4",
                "OPENROUTER_API_KEY",
            ),
            case("openrouter", "deepseek/deepseek-chat", "OPENROUTER_API_KEY"),
            case(
                "openrouter",
                "mistralai/mistral-small-3.2-24b-instruct",
                "OPENROUTER_API_KEY",
            ),
            case(
                "openrouter",
                "google/gemini-2.5-flash",
                "OPENROUTER_API_KEY",
            ),
            case(
                "openrouter",
                "meta-llama/llama-4-scout",
                "OPENROUTER_API_KEY",
            ),
        ],
        &SHARED[..1],
    )
    .into_iter()
    .chain(SHARED[2..].iter().copied())
    .collect::<Vec<_>>();
    for case in table {
        let Some((model, key)) = case.resolve() else {
            continue;
        };
        let system = long_system_prompt();
        let first_context = Context::new(
            system.clone(),
            vec![user("What is 2 + 2? Reply with just the number.")],
            vec![],
        );
        let first = complete(&model, &first_context, &key).await;
        assert_eq!(
            first.stop_reason,
            StopReason::Stop,
            "{}: {:?}",
            label(&model),
            first.error_message
        );
        let mut messages = first_context.messages.clone();
        messages.push(Message::Assistant(first.clone()));
        messages.push(user("What is 3 + 3? Reply with just the number."));
        let second = complete(&model, &Context::new(system, messages, vec![]), &key).await;
        assert_eq!(
            second.stop_reason,
            StopReason::Stop,
            "{}: {:?}",
            label(&model),
            second.error_message
        );
        for usage in [&first.usage, &second.usage] {
            assert_eq!(
                usage.total_tokens,
                usage.input + usage.output + usage.cache_read + usage.cache_write,
                "{}",
                label(&model)
            );
        }
        if model.provider == "anthropic" {
            assert!(
                second.usage.cache_read > 0
                    || second.usage.cache_write > 0
                    || first.usage.cache_write > 0,
                "{}: no cache activity",
                label(&model)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// tool-call-without-result.test.ts
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn orphaned_tool_calls_are_filtered() {
    let table = cases(
        &[
            case("google", "gemini-2.5-flash", "GEMINI_API_KEY"),
            completions("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
            case("openai", "gpt-5-mini", "OPENAI_API_KEY"),
            case(
                "azure-openai-responses",
                "gpt-4o-mini",
                "AZURE_OPENAI_API_KEY",
            ),
            case("anthropic", "claude-haiku-4-5", "ANTHROPIC_API_KEY"),
            case("anthropic", "claude-haiku-4-5", "ANTHROPIC_OAUTH_TOKEN"),
        ],
        SHARED,
    );
    let calculate = tool(
        "calculate",
        "Evaluate mathematical expressions",
        json!({"type": "object", "properties": {"expression": {"type": "string", "description": "The mathematical expression to evaluate"}}, "required": ["expression"]}),
    );
    for case in table {
        let Some((model, key)) = case.resolve() else {
            continue;
        };
        let mut context = Context::new(
            "You are a helpful assistant. Use the calculate tool when asked to perform calculations.".into(),
            vec![user("Please calculate 25 * 18 using the calculate tool.")],
            vec![calculate.clone()],
        );
        let first = complete(&model, &context, &key).await;
        assert!(
            first
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall(_))),
            "{}: expected a tool call",
            label(&model)
        );
        context.messages.push(Message::Assistant(first));
        context
            .messages
            .push(user("Never mind, just tell me what is 2+2?"));
        let second = complete(&model, &context, &key).await;
        assert_ne!(
            second.stop_reason,
            StopReason::Error,
            "{}: {:?}",
            label(&model),
            second.error_message
        );
        assert!(!second.content.is_empty());
        let tool_calls = second
            .content
            .iter()
            .filter(|c| matches!(c, Content::ToolCall(_)))
            .count();
        assert!(tool_calls > 0 || !text_of(&second).is_empty());
        assert!(matches!(
            second.stop_reason,
            StopReason::Stop | StopReason::ToolUse
        ));
    }
}

// ---------------------------------------------------------------------------
// unicode-surrogate.test.ts
// ---------------------------------------------------------------------------

const EMOJI_TOOL_RESULT: &str = "Test with emoji 🙈 and other characters:
- Monkey emoji: 🙈
- Thumbs up: 👍
- Heart: ❤️
- Thinking face: 🤔
- Rocket: 🚀
- Mixed text: Mario Zechner wann? Wo? Bin grad äußersr eventuninformiert 🙈
- Japanese: こんにちは
- Chinese: 你好
- Mathematical symbols: ∑∫∂√
- Special quotes: \"curly\" 'quotes'";

const LINKEDIN_TOOL_RESULT: &str =
    "Post: Hab einen \"Generative KI für Nicht-Techniker\" Workshop gebaut.
Unanswered Comments: 2

=> {
  \"comments\": [
    {
      \"author\": \"Matthias Neumayer's  graphic link\",
      \"text\": \"Leider nehmen das viel zu wenige Leute ernst\"
    },
    {
      \"author\": \"Matthias Neumayer's  graphic link\",
      \"text\": \"Mario Zechner wann? Wo? Bin grad äußersr eventuninformiert 🙈\"
    }
  ]
}";

#[tokio::test]
#[ignore = "live: needs provider keys and network"]
async fn unicode_in_tool_results_is_sent_safely() {
    let table = cases(
        &[
            case("google", "gemini-2.5-flash", "GEMINI_API_KEY"),
            case("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
            case("openai", "gpt-5-mini", "OPENAI_API_KEY"),
            case(
                "azure-openai-responses",
                "gpt-4o-mini",
                "AZURE_OPENAI_API_KEY",
            ),
            case("anthropic", "claude-haiku-4-5", "ANTHROPIC_API_KEY"),
            case("anthropic", "claude-haiku-4-5", "ANTHROPIC_OAUTH_TOKEN"),
        ],
        SHARED,
    );
    // A Rust string cannot hold the TS test's lone 0xD83D; sanitizeSurrogates
    // drops it before sending, so the sanitized text is what goes out here.
    let scenarios = [
        (
            "test_1",
            "test_tool",
            "A test tool",
            "Use the test tool",
            EMOJI_TOOL_RESULT.to_string(),
            "Summarize the tool result briefly.",
        ),
        (
            "linkedin_1",
            "linkedin_skill",
            "Get LinkedIn comments",
            "Use the linkedin tool to get comments",
            LINKEDIN_TOOL_RESULT.to_string(),
            "How many comments are there?",
        ),
        (
            "test_2",
            "test_tool",
            "A test tool",
            "Use the test tool",
            "Text with unpaired surrogate:  <- should be sanitized".to_string(),
            "What did the tool return?",
        ),
    ];
    for case in table {
        let Some((model, key)) = case.resolve() else {
            continue;
        };
        for (id, name, description, ask, result, follow_up) in &scenarios {
            let context = Context::new(
                "You are a helpful assistant.".into(),
                vec![
                    user(ask),
                    prior_tool_call(&model, id, name),
                    tool_result(id, name, vec![Content::text(result.clone())]),
                    user(follow_up),
                ],
                vec![tool(name, description, empty_schema())],
            );
            let response = complete(&model, &context, &key).await;
            assert_ne!(
                response.stop_reason,
                StopReason::Error,
                "{}: {:?}",
                label(&model),
                response.error_message
            );
            assert!(response.error_message.is_none());
            assert!(!response.content.is_empty());
        }
    }
}

// ---------------------------------------------------------------------------
// tool-call-id-normalization.test.ts (prefilled context)
// ---------------------------------------------------------------------------

const FAILING_TOOL_CALL_ID: &str = "call_pAYbIr76hXIjncD9UE4eGfnS|t5nnb2qYMFWGSsr13fhCd1CaCu3t3qONEPuOudu4HSVEtA8YJSL6FAZUxvoOoD792VIJWl91g87EdqsCWp9krVsdBysQoDaf9lMCLb8BS4EYi4gQd5kBQBYLlgD71PYwvf+TbMD9J9/5OMD42oxSRj8H+vRf78/l2Xla33LWz4nOgsddBlbvabICRs8GHt5C9PK5keFtzyi3lsyVKNlfduK3iphsZqs4MLv4zyGJnvZo/+QzShyk5xnMSQX/f98+aEoNflEApCdEOXipipgeiNWnpFSHbcwmMkZoJhURNu+JEz3xCh1mrXeYoN5o+trLL3IXJacSsLYXDrYTipZZbJFRPAucgbnjYBC+/ZzJOfkwCs+Gkw7EoZR7ZQgJ8ma+9586n4tT4cI8DEhBSZsWMjrCt8dxKg==";

#[tokio::test]
#[ignore = "live: needs OPENROUTER_API_KEY and network"]
async fn openrouter_handles_long_pipe_separated_tool_call_ids() {
    let Some((model, key)) =
        case("openrouter", "openai/gpt-5.3-codex", "OPENROUTER_API_KEY").resolve()
    else {
        return;
    };
    let echo = tool(
        "echo",
        "Echoes the message back",
        json!({"type": "object", "properties": {"message": {"type": "string", "description": "Message to echo back"}}, "required": ["message"]}),
    );
    let assistant = Message::Assistant(AssistantMessage {
        content: vec![Content::ToolCall(ToolCallContent {
            id: FAILING_TOOL_CALL_ID.into(),
            name: "echo".into(),
            arguments: json!({"message": "hello"}),
            thought_signature: None,
        })],
        api: "openai-responses".into(),
        provider: "github-copilot".into(),
        model: "gpt-5.3-codex".into(),
        usage: Usage {
            input: 100,
            output: 50,
            total_tokens: 150,
            ..Default::default()
        },
        stop_reason: StopReason::ToolUse,
        timestamp: cortexcode_ai_types::now_ms() - 1500,
        ..Default::default()
    });
    let context = Context::new(
        "You are a helpful assistant.".into(),
        vec![
            user("Use the echo tool to echo 'hello'"),
            assistant,
            tool_result(FAILING_TOOL_CALL_ID, "echo", vec![Content::text("hello")]),
            user("Say hi"),
        ],
        vec![echo],
    );
    let response = complete(&model, &context, &key).await;
    assert_ne!(
        response.stop_reason,
        StopReason::Error,
        "{:?}",
        response.error_message
    );
    if let Some(message) = &response.error_message {
        assert!(!message.contains("call_id") && !message.contains("too long"));
    }
}

// ---------------------------------------------------------------------------
// xhigh.test.ts (openai-responses / openai-completions with reasoning xhigh)
// ---------------------------------------------------------------------------

fn arithmetic_context() -> Context {
    let a = cortexcode_ai_types::now_ms() % 100;
    let b = (cortexcode_ai_types::now_ms() / 100) % 100;
    Context::new(
        String::new(),
        vec![user(&format!("What is {a} + {b}? Think step by step."))],
        vec![],
    )
}

/// `stream(model, ctx, { reasoningEffort: "xhigh" })` through the provider's
/// own options, bypassing the simple-options clamp.
async fn xhigh(model: &Model, key: &str) -> AssistantMessage {
    if model.api == "openai-completions" {
        let options = cortexcode_ai_provider_openai::request::CompletionsOptions {
            api_key: Some(key.into()),
            reasoning_effort: Some("xhigh".into()),
            ..Default::default()
        };
        cortexcode_ai_provider_openai::stream_completions(
            model.clone(),
            arithmetic_context(),
            options,
        )
        .result()
        .await
    } else {
        let options = cortexcode_ai_provider_openai_responses::ResponsesOptions {
            api_key: Some(key.into()),
            reasoning_effort: Some("xhigh".into()),
            ..Default::default()
        };
        cortexcode_ai_provider_openai_responses::stream_responses(
            model.clone(),
            arithmetic_context(),
            options,
        )
        .result()
        .await
    }
}

#[tokio::test]
#[ignore = "live: needs OPENAI_API_KEY and network"]
async fn xhigh_reasoning_works_or_is_rejected_by_the_model() {
    let Some((codex_max, key)) = case("openai", "gpt-5.1-codex-max", "OPENAI_API_KEY").resolve()
    else {
        return;
    };
    let response = xhigh(&codex_max, &key).await;
    assert_eq!(
        response.stop_reason,
        StopReason::Stop,
        "{:?}",
        response.error_message
    );
    assert!(response
        .content
        .iter()
        .any(|c| matches!(c, Content::Text(_))));
    assert!(response
        .content
        .iter()
        .any(|c| matches!(c, Content::Thinking(_))));

    for case in [
        case("openai", "gpt-5-mini", "OPENAI_API_KEY"),
        completions("openai", "gpt-5-mini", "OPENAI_API_KEY"),
    ] {
        let (model, key) = case.resolve().unwrap();
        let response = xhigh(&model, &key).await;
        assert_eq!(response.stop_reason, StopReason::Error);
        assert!(response.error_message.unwrap_or_default().contains("xhigh"));
    }
}

// ---------------------------------------------------------------------------
// zen.test.ts
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "live: needs OPENCODE_API_KEY and network"]
async fn opencode_models_say_hello() {
    let Some(key) = env("OPENCODE_API_KEY") else {
        return;
    };
    for provider in ["opencode", "opencode-go"] {
        for model in cortexcode_ai_models::get_models(provider) {
            let context = Context::new(String::new(), vec![user("Say hello.")], vec![]);
            let response = complete(model, &context, &key).await;
            assert!(!response.content.is_empty(), "{}", label(model));
            assert_eq!(
                response.stop_reason,
                StopReason::Stop,
                "{}: {:?}",
                label(model),
                response.error_message
            );
        }
    }
}
