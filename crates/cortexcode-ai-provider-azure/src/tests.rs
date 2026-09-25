//! Ported from `azure-openai-base-url.test.ts` (hoocode v0.5.89), plus the
//! request shape and streaming of `azure-openai-responses.ts`.

use super::*;
use cortexcode_ai_stream::testing::serve_script;
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, Content, Message, StopReason, TextContent, ThinkingContent,
    ToolCallContent, ToolResultMessage, UserMessage,
};
use std::sync::{Mutex, MutexGuard};

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const AZURE_VARS: [&str; 5] = [
    "AZURE_OPENAI_BASE_URL",
    "AZURE_OPENAI_RESOURCE_NAME",
    "AZURE_OPENAI_API_VERSION",
    "AZURE_OPENAI_API_KEY",
    "AZURE_OPENAI_DEPLOYMENT_NAME_MAP",
];

/// Run `f` with the Azure variables cleared except `set`, restoring them.
fn with_env<T>(set: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
    let _lock = env_lock();
    let saved: Vec<(&str, Option<String>)> = AZURE_VARS
        .iter()
        .map(|k| (*k, std::env::var(k).ok()))
        .collect();
    for k in AZURE_VARS {
        std::env::remove_var(k);
    }
    for (k, v) in set {
        std::env::set_var(k, v);
    }
    let out = f();
    for (k, v) in saved {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    out
}

fn azure_model() -> Model {
    cortexcode_ai_models::get_model("azure-openai-responses", "gpt-4o-mini")
        .expect("catalog model")
        .clone()
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: vec![Content::Text(TextContent::new(text))].into(),
        timestamp: 0,
    })
}

fn hello() -> Context {
    Context::new(String::new(), vec![user("hello")], vec![])
}

fn base_url_for(env_base_url: &str) -> String {
    with_env(&[("AZURE_OPENAI_BASE_URL", env_base_url)], || {
        resolve_azure_config(&azure_model(), &AzureOptions::default())
            .unwrap()
            .0
    })
}

// --- azure-openai-base-url.test.ts ---

#[test]
fn normalizes_cognitive_services_root_endpoints() {
    assert_eq!(
        base_url_for("https://marc-quicktests-resource.cognitiveservices.azure.com"),
        "https://marc-quicktests-resource.cognitiveservices.azure.com/openai/v1"
    );
}

#[test]
fn normalizes_azure_openai_root_endpoints() {
    assert_eq!(
        base_url_for("https://my-resource.openai.azure.com"),
        "https://my-resource.openai.azure.com/openai/v1"
    );
}

#[test]
fn normalizes_openai_path_to_openai_v1() {
    assert_eq!(
        base_url_for("https://my-resource.cognitiveservices.azure.com/openai"),
        "https://my-resource.cognitiveservices.azure.com/openai/v1"
    );
}

#[test]
fn preserves_openai_v1_endpoints() {
    assert_eq!(
        base_url_for("https://my-resource.cognitiveservices.azure.com/openai/v1"),
        "https://my-resource.cognitiveservices.azure.com/openai/v1"
    );
}

#[test]
fn preserves_explicit_non_azure_proxy_paths() {
    assert_eq!(
        base_url_for("https://my-proxy.example.com/v1"),
        "https://my-proxy.example.com/v1"
    );
}

#[test]
fn strips_query_params_when_normalizing_azure_hosts() {
    assert_eq!(
        base_url_for("https://my-resource.openai.azure.com/openai?api-version=2024-12-01"),
        "https://my-resource.openai.azure.com/openai/v1"
    );
}

#[test]
fn preserves_query_params_on_non_azure_proxies() {
    assert_eq!(
        base_url_for("https://my-proxy.example.com/v1?custom=true"),
        "https://my-proxy.example.com/v1?custom=true"
    );
}

#[test]
fn invalid_urls_end_the_stream_with_an_error() {
    let result = with_env(&[("AZURE_OPENAI_BASE_URL", "not-a-url")], || {
        let options = AzureOptions {
            base: ResponsesOptions {
                api_key: Some("test-api-key".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        stream_azure(azure_model(), hello(), options).result_blocking()
    });
    assert_eq!(result.stop_reason, StopReason::Error);
    assert!(result
        .error_message
        .unwrap()
        .contains("Invalid Azure OpenAI base URL"));
}

#[test]
fn builds_default_url_from_resource_name() {
    let base = with_env(&[("AZURE_OPENAI_RESOURCE_NAME", "my-resource")], || {
        resolve_azure_config(&azure_model(), &AzureOptions::default()).unwrap()
    });
    assert_eq!(
        base,
        (
            "https://my-resource.openai.azure.com/openai/v1".to_string(),
            "v1".to_string()
        )
    );
}

// --- config and request ---

#[test]
fn missing_base_url_is_reported() {
    let err = with_env(&[], || {
        resolve_azure_config(&azure_model(), &AzureOptions::default()).unwrap_err()
    });
    assert!(err.starts_with("Azure OpenAI base URL is required."));
}

#[test]
fn option_and_env_precedence() {
    let options = AzureOptions {
        azure_base_url: Some(" https://opt.openai.azure.com ".into()),
        azure_api_version: Some("2025-01-01".into()),
        ..Default::default()
    };
    let resolved = with_env(
        &[
            ("AZURE_OPENAI_BASE_URL", "https://env.example.com/v1"),
            ("AZURE_OPENAI_API_VERSION", "preview"),
        ],
        || resolve_azure_config(&azure_model(), &options).unwrap(),
    );
    assert_eq!(
        resolved,
        (
            "https://opt.openai.azure.com/openai/v1".to_string(),
            "2025-01-01".to_string()
        )
    );
    let mut model = azure_model();
    model.base_url = "https://model.example.com/v1".into();
    let resolved = with_env(&[], || {
        resolve_azure_config(&model, &AzureOptions::default()).unwrap()
    });
    assert_eq!(resolved.0, "https://model.example.com/v1");
}

#[test]
fn deployment_names_come_from_option_then_map_then_model() {
    let model = azure_model();
    with_env(
        &[(
            "AZURE_OPENAI_DEPLOYMENT_NAME_MAP",
            " other=x , gpt-4o-mini=my-deploy=extra,",
        )],
        || {
            assert_eq!(
                resolve_deployment_name(&model, &AzureOptions::default()),
                "my-deploy"
            );
            let options = AzureOptions {
                azure_deployment_name: Some("explicit".into()),
                ..Default::default()
            };
            assert_eq!(resolve_deployment_name(&model, &options), "explicit");
        },
    );
    with_env(&[], || {
        assert_eq!(
            resolve_deployment_name(&model, &AzureOptions::default()),
            "gpt-4o-mini"
        )
    });
}

#[test]
fn request_url_replaces_the_query_with_api_version() {
    assert_eq!(
        request_url("https://r.openai.azure.com/openai/v1", "v1"),
        "https://r.openai.azure.com/openai/v1/responses?api-version=v1"
    );
    assert_eq!(
        request_url("http://127.0.0.1:9/v1?custom=true", "v1"),
        "http://127.0.0.1:9/v1?api-version=v1"
    );
}

#[test]
fn build_params_uses_deployment_and_cache_key() {
    let mut model = azure_model();
    model.reasoning = true;
    let options = ResponsesOptions {
        session_id: Some("s1".into()),
        max_tokens: Some(100),
        ..Default::default()
    };
    let body = build_params(&model, &hello(), &options, "deploy-1");
    assert_eq!(
        body,
        json!({
            "model": "deploy-1",
            "input": [{"role": "user", "content": [{"type": "input_text", "text": "hello"}]}],
            "stream": true,
            "prompt_cache_key": "s1",
            "max_output_tokens": 100,
            "reasoning": {"effort": "none"}
        })
    );
}

#[test]
fn replays_reasoning_items_paired_with_tool_calls() {
    let model = azure_model();
    let reasoning = json!({"type": "reasoning", "id": "rs_1", "summary": []});
    let context = Context::new(
        String::new(),
        vec![
            user("go"),
            Message::Assistant(AssistantMessage {
                content: vec![
                    Content::Thinking(ThinkingContent {
                        thinking: String::new(),
                        signature: Some(reasoning.to_string()),
                        redacted: false,
                    }),
                    Content::ToolCall(ToolCallContent {
                        id: "call_1|fc_1".into(),
                        name: "read".into(),
                        arguments: json!({"path": "a"}),
                        thought_signature: None,
                    }),
                ],
                api: "azure-openai-responses".into(),
                provider: "azure-openai-responses".into(),
                model: "gpt-4o-mini".into(),
                stop_reason: StopReason::ToolUse,
                ..Default::default()
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_1|fc_1".into(),
                tool_name: "read".into(),
                content: vec![Content::Text(TextContent::new("data"))],
                details: None,
                is_error: false,
                timestamp: 0,
            }),
        ],
        vec![],
    );
    let body = build_params(&model, &context, &ResponsesOptions::default(), "d");
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[1], reasoning);
    assert_eq!(input[2]["id"], "fc_1");
    assert_eq!(input[2]["call_id"], "call_1");
    assert_eq!(
        input[3],
        json!({"type": "function_call_output", "call_id": "call_1", "output": "data"})
    );
}

// --- streaming ---

#[test]
fn streams_over_http_with_api_key_header() {
    let events = [
        json!({"type": "response.output_item.added", "item": {"type": "message", "id": "m", "content": []}}),
        json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": ""}}),
        json!({"type": "response.output_text.delta", "delta": "Hi"}),
        json!({"type": "response.output_item.done", "item": {"type": "message", "id": "m", "content": [{"type": "output_text", "text": "Hi"}]}}),
        json!({"type": "response.completed", "response": {"status": "completed", "usage": {"input_tokens": 3, "output_tokens": 1, "total_tokens": 4}}}),
    ];
    let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    let server = serve_script(vec![(
        "HTTP/1.1 200 OK",
        "text/event-stream",
        Box::leak(body.into_boxed_str()),
    )]);
    let message = with_env(&[("AZURE_OPENAI_BASE_URL", &server.base_url)], || {
        stream(
            azure_model(),
            hello(),
            SimpleStreamOptions {
                api_key: Some("az-key".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .result_blocking()
    });
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(
        message.content[0],
        Content::Text(TextContent {
            text: "Hi".into(),
            text_signature: Some(r#"{"v":1,"id":"m"}"#.into()),
        })
    );
    assert_eq!(message.usage.total_tokens, 4);
    let req = &server.requests()[0];
    assert_eq!(req.path, "/responses?api-version=v1");
    assert_eq!(req.header("api-key"), Some("az-key"));
    assert_eq!(req.header("authorization"), None);
    assert_eq!(req.json()["model"], "gpt-4o-mini");
}

#[test]
fn missing_api_key_errors_immediately() {
    let err = with_env(&[], || {
        stream(azure_model(), hello(), SimpleStreamOptions::default())
            .err()
            .unwrap()
    });
    assert_eq!(
        err.to_string(),
        "No API key for provider: azure-openai-responses"
    );
}

#[test]
fn immediate_abort_ends_aborted() {
    let signal = AbortSignal::new();
    signal.abort();
    let options = AzureOptions {
        base: ResponsesOptions {
            api_key: Some("k".into()),
            signal: Some(signal),
            ..Default::default()
        },
        azure_base_url: Some("http://127.0.0.1:9/v1".into()),
        ..Default::default()
    };
    let m = stream_azure(azure_model(), hello(), options).result_blocking();
    assert_eq!(m.stop_reason, StopReason::Aborted);
}
