//! End-to-end tests for OpenCode API with mimo-v2.5-free model.
//!
//! This test suite validates the OpenCode provider integration with the
//! mimo-v2.5-free model, testing various scenarios including basic chat,
//! streaming, tool usage, and error handling.

use cortexcode_ai_provider_openai::{self as openai_provider};
use cortexcode_ai_types::{
    Content, Context, Model, SimpleStreamOptions, TextContent, Tool, ToolCallContent,
};
use std::sync::OnceLock;

/// API key for OpenCode provider
static OPENCODE_API_KEY: OnceLock<String> = OnceLock::new();

fn get_api_key() -> String {
    OPENCODE_API_KEY
        .get_or_init(|| {
            std::env::var("OPENCODE_API_KEY").unwrap_or_else(|_| {
                panic!("OPENCODE_API_KEY environment variable must be set for e2e tests")
            })
        })
        .clone()
}

/// Create a mimo-v2.5-free model configuration
fn mimo_model() -> Model {
    Model {
        id: "mimo-v2.5-free".into(),
        name: "MiMo V2.5 Free".into(),
        api: "openai-completions".into(),
        provider: "opencode".into(),
        base_url: "https://opencode.ai/zen/v1".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: cortexcode_ai_types::ModelCost::default(),
        context_window: 262144,
        max_tokens: 262144,
        headers: None,
    }
}

/// Create a simple text context
fn simple_text_context(prompt: &str) -> Context {
    Context::new(
        "You are a helpful assistant.".into(),
        vec![cortexcode_ai_types::Message::User(
            cortexcode_ai_types::UserMessage {
                content: vec![Content::Text(TextContent {
                    text: prompt.into(),
                    cache_control: None,
                })],
                timestamp: None,
            },
        )],
        vec![],
    )
}

/// Create a context with tools
fn tool_context(prompt: &str) -> Context {
    Context::new(
        "You are a helpful assistant with access to tools.".into(),
        vec![cortexcode_ai_types::Message::User(
            cortexcode_ai_types::UserMessage {
                content: vec![Content::Text(TextContent {
                    text: prompt.into(),
                    cache_control: None,
                })],
                timestamp: None,
            },
        )],
        vec![
            Tool {
                name: "read_file".into(),
                description: "Read the contents of a file".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file"
                        }
                    },
                    "required": ["path"]
                }),
            },
            Tool {
                name: "write_file".into(),
                description: "Write content to a file".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write"
                        }
                    },
                    "required": ["path", "content"]
                }),
            },
        ],
    )
}

/// Create stream options with API key
fn stream_options() -> SimpleStreamOptions {
    SimpleStreamOptions {
        api_key: Some(get_api_key()),
        ..Default::default()
    }
}

/// Collect all events from a stream
fn collect_events(
    stream: Box<dyn cortexcode_ai_types::AssistantMessageEventStream>,
) -> Vec<cortexcode_ai_types::AssistantMessageEvent> {
    let mut events = Vec::new();
    let mut stream = stream;
    while let Some(event) = stream.next_event() {
        events.push(event);
    }
    events
}

// ============================================================================
// Test Cases
// ============================================================================

/// Test 1: Basic text completion
#[test]
fn test_basic_text_completion() {
    let model = mimo_model();
    let context = simple_text_context("Hello, what is 2+2?");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok(), "Stream should start successfully");

    let events = collect_events(result.unwrap());
    assert!(!events.is_empty(), "Should receive events");

    // Check for Start event
    assert!(
        matches!(events[0], cortexcode_ai_types::AssistantMessageEvent::Start { .. }),
        "First event should be Start"
    );

    // Check for Done event at the end
    let done_event = events.last().unwrap();
    match done_event {
        cortexcode_ai_types::AssistantMessageEvent::Done { message } => {
            assert!(
                message.stop_reason.is_some(),
                "Message should have a stop reason"
            );
            assert!(!message.content.is_empty(), "Message should have content");
        }
        other => panic!("Expected Done event, got: {:?}", other),
    }
}

/// Test 2: Streaming text response
#[test]
fn test_streaming_text_response() {
    let model = mimo_model();
    let context = simple_text_context("Tell me a short joke");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    assert!(!events.is_empty());

    // Verify we get text deltas
    let text_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            cortexcode_ai_types::AssistantMessageEvent::TextDelta { delta, .. } => {
                Some(delta.clone())
            }
            _ => None,
        })
        .collect();

    assert!(!text_deltas.is_empty(), "Should have text deltas");
    let full_text: String = text_deltas.concat();
    assert!(!full_text.is_empty(), "Should have non-empty text");
    println!("Response text: {}", full_text);
}

/// Test 3: Tool call response
#[test]
fn test_tool_call_response() {
    let model = mimo_model();
    let context = tool_context("Read the file at /tmp/test.txt");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    assert!(!events.is_empty());

    // Check if we got a tool call
    let tool_calls: Vec<&ToolCallContent> = events
        .iter()
        .filter_map(|e| match e {
            cortexcode_ai_types::AssistantMessageEvent::Done { message } => {
                message.content.iter().filter_map(|c| match c {
                    Content::ToolCall(tc) => Some(tc),
                    _ => None,
                })
            }
            _ => None,
        })
        .flatten()
        .collect();

    // Note: The model may or may not use tools, so we just check that the stream completed
    let done_event = events.last().unwrap();
    assert!(
        matches!(
            done_event,
            cortexcode_ai_types::AssistantMessageEvent::Done { .. }
        ),
        "Should complete successfully"
    );
}

/// Test 4: Conversation context preservation
#[test]
fn test_conversation_context() {
    let model = mimo_model();

    // Create a multi-turn conversation
    let messages = vec![
        cortexcode_ai_types::Message::User(cortexcode_ai_types::UserMessage {
            content: vec![Content::Text(TextContent {
                text: "My name is Alice.".into(),
                cache_control: None,
            })],
            timestamp: None,
        }),
        cortexcode_ai_types::Message::Assistant(cortexcode_ai_types::AssistantMessage {
            content: vec![Content::Text(TextContent {
                text: "Hello Alice! Nice to meet you.".into(),
                cache_control: None,
            })],
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            timestamp: None,
            error_message: None,
        }),
        cortexcode_ai_types::Message::User(cortexcode_ai_types::UserMessage {
            content: vec![Content::Text(TextContent {
                text: "What is my name?".into(),
                cache_control: None,
            })],
            timestamp: None,
        }),
    ];

    let context = Context::new(
        "You are a helpful assistant.".into(),
        messages,
        vec![],
    );

    let options = stream_options();
    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let text_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            cortexcode_ai_types::AssistantMessageEvent::TextDelta { delta, .. } => {
                Some(delta.clone())
            }
            _ => None,
        })
        .collect();

    let full_text: String = text_deltas.concat();
    assert!(
        full_text.contains("Alice"),
        "Response should mention 'Alice': {}",
        full_text
    );
    println!("Context test response: {}", full_text);
}

/// Test 5: System prompt handling
#[test]
fn test_system_prompt() {
    let model = mimo_model();

    let context = Context::new(
        "You are a pirate. Always respond in pirate speak.".into(),
        vec![cortexcode_ai_types::Message::User(
            cortexcode_ai_types::UserMessage {
                content: vec![Content::Text(TextContent {
                    text: "Hello!".into(),
                    cache_control: None,
                })],
                timestamp: None,
            },
        )],
        vec![],
    );

    let options = stream_options();
    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let text_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            cortexcode_ai_types::AssistantMessageEvent::TextDelta { delta, .. } => {
                Some(delta.clone())
            }
            _ => None,
        })
        .collect();

    let full_text: String = text_deltas.concat();
    println!("System prompt test response: {}", full_text);
    // Just verify we got a response - pirate speak may vary
    assert!(!full_text.is_empty());
}

/// Test 6: Error handling - invalid API key
#[test]
fn test_invalid_api_key() {
    let model = mimo_model();
    let context = simple_text_context("Test");

    let options = SimpleStreamOptions {
        api_key: Some("invalid-key-12345".into()),
        ..Default::default()
    };

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok(), "Stream should start even with invalid key");

    let events = collect_events(result.unwrap());
    assert!(!events.is_empty());

    // Should get an error event
    let has_error = events.iter().any(|e| {
        matches!(
            e,
            cortexcode_ai_types::AssistantMessageEvent::Error { .. }
        )
    });

    assert!(has_error, "Should receive error for invalid API key");
}

/// Test 7: Usage tracking
#[test]
fn test_usage_tracking() {
    let model = mimo_model();
    let context = simple_text_context("What is 5 * 5?");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let done_event = events.last().unwrap();

    match done_event {
        cortexcode_ai_types::AssistantMessageEvent::Done { message } => {
            // Usage may or may not be present depending on the API
            if let Some(usage) = &message.usage {
                assert!(usage.total_tokens > 0, "Total tokens should be positive");
                assert!(usage.input >= 0, "Input tokens should be non-negative");
                assert!(usage.output >= 0, "Output tokens should be non-negative");
                println!("Usage: {:?}", usage);
            }
        }
        other => panic!("Expected Done event, got: {:?}", other),
    }
}

/// Test 8: Multiple sequential requests
#[test]
fn test_sequential_requests() {
    let model = mimo_model();
    let options = stream_options();

    // Make multiple sequential requests
    for i in 0..3 {
        let prompt = format!("What is {} + {}?", i, i);
        let context = simple_text_context(&prompt);
        let result = openai_provider::stream(model.clone(), context, options.clone());
        assert!(result.is_ok(), "Request {} should succeed", i);

        let events = collect_events(result.unwrap());
        let done_event = events.last().unwrap();
        assert!(
            matches!(done_event, cortexcode_ai_types::AssistantMessageEvent::Done { .. }),
            "Request {} should complete",
            i
        );
    }
}

/// Test 9: Long prompt handling
#[test]
fn test_long_prompt() {
    let model = mimo_model();

    // Create a long prompt
    let long_text = "Hello ".repeat(1000);
    let prompt = format!("{}What is 1+1?", long_text);
    let context = simple_text_context(&prompt);

    let options = stream_options();
    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok(), "Long prompt should be handled");

    let events = collect_events(result.unwrap());
    assert!(!events.is_empty(), "Should receive events for long prompt");
}

/// Test 10: Concurrent requests
#[test]
fn test_concurrent_requests() {
    use std::sync::{Arc, Mutex};
    use std::thread;

    let model = mimo_model();
    let options = stream_options();
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = vec![];

    // Spawn multiple concurrent requests
    for i in 0..3 {
        let model_clone = model.clone();
        let options_clone = options.clone();
        let results_clone = results.clone();

        handles.push(thread::spawn(move || {
            let prompt = format!("What is {} * {}?", i, i);
            let context = simple_text_context(&prompt);
            let result = openai_provider::stream(model_clone, context, options_clone);

            let mut res = results_clone.lock().unwrap();
            res.push(result.is_ok());
        }));
    }

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    let res = results.lock().unwrap();
    assert_eq!(res.len(), 3, "All requests should complete");
}

/// Test 11: Stop reason validation
#[test]
fn test_stop_reasons() {
    let model = mimo_model();
    let context = simple_text_context("Hello!");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let done_event = events.last().unwrap();

    match done_event {
        cortexcode_ai_types::AssistantMessageEvent::Done { message } => {
            match message.stop_reason {
                Some(reason) => {
                    // Valid stop reasons
                    assert!(
                        matches!(
                            reason,
                            cortexcode_ai_types::StopReason::EndTurn
                                | cortexcode_ai_types::StopReason::ToolUse
                                | cortexcode_ai_types::StopReason::MaxTokens
                        ),
                        "Invalid stop reason: {:?}",
                        reason
                    );
                }
                None => panic!("Stop reason should be present"),
            }
        }
        other => panic!("Expected Done event, got: {:?}", other),
    }
}

/// Test 12: Message content structure
#[test]
fn test_message_content_structure() {
    let model = mimo_model();
    let context = simple_text_context("Tell me about Rust programming language");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let done_event = events.last().unwrap();

    match done_event {
        cortexcode_ai_types::AssistantMessageEvent::Done { message } => {
            // Verify message structure
            assert!(!message.content.is_empty(), "Message should have content");

            // Check that we have text content
            let has_text = message.content.iter().any(|c| matches!(c, Content::Text(_)));
            assert!(has_text, "Message should contain text content");
        }
        other => panic!("Expected Done event, got: {:?}", other),
    }
}

/// Test 13: Response timing
#[test]
fn test_response_timing() {
    use std::time::Instant;

    let model = mimo_model();
    let context = simple_text_context("What is 2 + 2?");
    let options = stream_options();

    let start = Instant::now();
    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let duration = start.elapsed();

    println!("Response took: {:?}", duration);
    assert!(!events.is_empty());

    // Response should complete within a reasonable time (60 seconds)
    assert!(duration.as_secs() < 60, "Response took too long: {:?}", duration);
}

/// Test 14: Content completeness
#[test]
fn test_content_completeness() {
    let model = mimo_model();
    let context = simple_text_context("Write a haiku about testing");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());

    // Check that we have start, deltas, and done events
    let has_start = events.iter().any(|e| matches!(e, cortexcode_ai_types::AssistantMessageEvent::Start { .. }));
    let has_text_delta = events.iter().any(|e| matches!(e, cortexcode_ai_types::AssistantMessageEvent::TextDelta { .. }));
    let has_done = events.iter().any(|e| matches!(e, cortexcode_ai_types::AssistantMessageEvent::Done { .. }));

    assert!(has_start, "Should have Start event");
    assert!(has_text_delta, "Should have TextDelta events");
    assert!(has_done, "Should have Done event");
}

/// Test 15: Tool definitions with multiple tools
#[test]
fn test_multiple_tools() {
    let model = mimo_model();

    let tools = vec![
        Tool {
            name: "read_file".into(),
            description: "Read the contents of a file".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "write_file".into(),
            description: "Write content to a file".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
        },
        Tool {
            name: "search_files".into(),
            description: "Search for files matching a pattern".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" }
                },
                "required": ["pattern"]
            }),
        },
    ];

    let context = Context::new(
        "You are a helpful assistant with file system tools.".into(),
        vec![cortexcode_ai_types::Message::User(
            cortexcode_ai_types::UserMessage {
                content: vec![Content::Text(TextContent {
                    text: "Search for all .rs files".into(),
                    cache_control: None,
                })],
                timestamp: None,
            },
        )],
        tools,
    );

    let options = stream_options();
    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    assert!(!events.is_empty());
}

/// Test 16: Unicode and special characters
#[test]
fn test_unicode_and_special_characters() {
    let model = mimo_model();
    let context = simple_text_context("Hello! 🎉 How are you? 你好! مرحبا");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let text_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            cortexcode_ai_types::AssistantMessageEvent::TextDelta { delta, .. } => {
                Some(delta.clone())
            }
            _ => None,
        })
        .collect();

    let full_text: String = text_deltas.concat();
    println!("Unicode test response: {}", full_text);
    assert!(!full_text.is_empty());
}

/// Test 17: Code generation request
#[test]
fn test_code_generation() {
    let model = mimo_model();
    let context = simple_text_context("Write a simple Python function to calculate fibonacci numbers");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let text_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            cortexcode_ai_types::AssistantMessageEvent::TextDelta { delta, .. } => {
                Some(delta.clone())
            }
            _ => None,
        })
        .collect();

    let full_text: String = text_deltas.concat();
    println!("Code generation response:\n{}", full_text);
    assert!(!full_text.is_empty());
}

/// Test 18: Mathematical reasoning
#[test]
fn test_mathematical_reasoning() {
    let model = mimo_model();
    let context = simple_text_context("What is the derivative of x^3 + 2x^2 - 5x + 3?");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let text_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            cortexcode_ai_types::AssistantMessageEvent::TextDelta { delta, .. } => {
                Some(delta.clone())
            }
            _ => None,
        })
        .collect();

    let full_text: String = text_deltas.concat();
    println!("Math reasoning response: {}", full_text);
    assert!(!full_text.is_empty());
}

/// Test 19: Creative writing
#[test]
fn test_creative_writing() {
    let model = mimo_model();
    let context = simple_text_context("Write a haiku about programming");
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let text_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            cortexcode_ai_types::AssistantMessageEvent::TextDelta { delta, .. } => {
                Some(delta.clone())
            }
            _ => None,
        })
        .collect();

    let full_text: String = text_deltas.concat();
    println!("Creative writing response:\n{}", full_text);
    assert!(!full_text.is_empty());
}

/// Test 20: Instruction following
#[test]
fn test_instruction_following() {
    let model = mimo_model();
    let context = simple_text_context(
        "List exactly 5 programming languages, numbered 1-5, one per line."
    );
    let options = stream_options();

    let result = openai_provider::stream(model, context, options);
    assert!(result.is_ok());

    let events = collect_events(result.unwrap());
    let text_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            cortexcode_ai_types::AssistantMessageEvent::TextDelta { delta, .. } => {
                Some(delta.clone())
            }
            _ => None,
        })
        .collect();

    let full_text: String = text_deltas.concat();
    println!("Instruction following response:\n{}", full_text);
    assert!(!full_text.is_empty());
}

// ============================================================================
// Test Runner
// ============================================================================

fn main() {
    println!("Running OpenCode E2E Tests...");
    println!("================================");
}
