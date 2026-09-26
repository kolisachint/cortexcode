//! OpenRouter image-generation provider.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `providers/images/openrouter.ts`. Uses OpenRouter's Chat Completions
//! endpoint with `modalities: ["image", ...]` — a single non-streaming
//! request/response, unlike the text providers' SSE streams.

use cortexcode_ai_types::{Content, Cost, ImageContent, StopReason, TextContent, Usage};

use crate::types::{
    calculate_image_cost, AssistantImages, ImagesContext, ImagesModel, ImagesOptions,
};

fn build_request_body(model: &ImagesModel, context: &ImagesContext) -> serde_json::Value {
    let content: Vec<serde_json::Value> = context
        .input
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(serde_json::json!({"type": "text", "text": t.text})),
            Content::Image(img) => Some(serde_json::json!({
                "type": "image_url",
                "image_url": {"url": format!("data:{};base64,{}", img.media_type, img.data)},
            })),
            Content::Thinking(_) | Content::ToolCall(_) => None,
        })
        .collect();

    let modalities = if model.output.iter().any(|s| s == "text") {
        serde_json::json!(["image", "text"])
    } else {
        serde_json::json!(["image"])
    };

    serde_json::json!({
        "model": model.id,
        "messages": [{"role": "user", "content": content}],
        "stream": false,
        "modalities": modalities,
    })
}

/// Parse a `data:` URI into `(media_type, base64_data)`.
fn parse_data_uri(uri: &str) -> Option<(String, String)> {
    // /^data:([^;]+);base64,(.+)$/
    let rest = uri.strip_prefix("data:")?;
    let (media_type, data) = rest.split_once(';')?;
    let data = data.strip_prefix("base64,")?;
    if media_type.is_empty() || data.is_empty() || data.contains('\n') {
        return None;
    }
    Some((media_type.to_string(), data.to_string()))
}

fn parse_usage(usage: &serde_json::Value, model: &ImagesModel) -> Usage {
    let prompt_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
    let reported_cached = usage["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cache_write = usage["prompt_tokens_details"]["cache_write_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cache_read = if cache_write > 0 {
        reported_cached.saturating_sub(cache_write)
    } else {
        reported_cached
    };
    let input = prompt_tokens
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
    let output = usage["completion_tokens"].as_u64().unwrap_or(0);

    let mut u = Usage {
        input,
        output,
        cache_read,
        cache_write,
        total_tokens: input + output + cache_read + cache_write,
        cost: Cost::default(),
    };
    u.cost = calculate_image_cost(&model.cost, &u);
    u
}

/// `generateImagesOpenRouter`: every failure is reported on the result
/// (`stopReason` `error`, or `aborted` when the signal fired).
pub async fn generate_images(
    model: &ImagesModel,
    context: &ImagesContext,
    options: &ImagesOptions,
) -> AssistantImages {
    let mut output = AssistantImages {
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        output: vec![],
        response_id: None,
        stop_reason: StopReason::Stop,
        error_message: None,
        usage: None,
        timestamp: now_millis(),
    };
    let result = match &options.signal {
        Some(signal) => tokio::select! {
            biased;
            _ = signal.cancelled() => Err("Request was aborted".to_string()),
            r = request(model, context, options, &mut output) => r,
        },
        None => request(model, context, options, &mut output).await,
    };
    if let Err(message) = result {
        output.stop_reason = if options.signal.as_ref().is_some_and(|s| s.aborted()) {
            StopReason::Aborted
        } else {
            StopReason::Error
        };
        output.error_message = Some(message);
    }
    output
}

async fn request(
    model: &ImagesModel,
    context: &ImagesContext,
    options: &ImagesOptions,
    output: &mut AssistantImages,
) -> Result<(), String> {
    let api_key = options
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .or_else(|| cortexcode_ai_env::get_env_api_key(&model.provider))
        .ok_or_else(|| format!("No API key available for provider: {}", model.provider))?;

    let mut builder = reqwest::Client::builder();
    if let Some(ms) = options.timeout_ms {
        builder = builder.timeout(std::time::Duration::from_millis(ms));
    }
    let client = builder
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let url = format!("{}/chat/completions", model.base_url.trim_end_matches('/'));
    let mut headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("authorization".to_string(), format!("Bearer {api_key}")),
    ];
    for extra in [&model.headers, &options.headers].into_iter().flatten() {
        headers.extend(extra.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let body = build_request_body(model, context);
    let response = cortexcode_ai_util::post_json_with_sdk_retries(
        &client,
        &url,
        &headers,
        &body,
        options.max_retries,
        None,
    )
    .await
    .map_err(|failure| failure.message().to_string())?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|e| format!("failed to read response body: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(cortexcode_ai_util::openai_api_error_message(status, &text));
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid JSON response: {e}; body={text}"))?;

    output.response_id = value["id"].as_str().map(str::to_string);
    if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
        output.usage = Some(parse_usage(usage, model));
    }
    let Some(choice) = value["choices"].get(0) else {
        return Ok(());
    };
    let message = &choice["message"];
    if let Some(text) = message["content"].as_str().filter(|t| !t.is_empty()) {
        output.output.push(Content::Text(TextContent {
            text_signature: None,
            text: text.to_string(),
        }));
    }
    for image in message["images"].as_array().into_iter().flatten() {
        let image_url = image["image_url"]
            .as_str()
            .or_else(|| image["image_url"]["url"].as_str());
        let Some((media_type, data)) = image_url.and_then(parse_data_uri) else {
            continue;
        };
        output
            .output
            .push(Content::Image(ImageContent { media_type, data }));
    }
    Ok(())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn test_model(base_url: String) -> ImagesModel {
        ImagesModel {
            id: "openrouter/some-image-model".into(),
            name: "Some Image Model".into(),
            input: vec!["text".into()],
            api: "openrouter-images".into(),
            provider: "openrouter".into(),
            base_url,
            headers: None,
            output: vec!["image".into()],
            cost: cortexcode_ai_types::ModelCost::default(),
        }
    }

    fn spawn_mock_server(status_line: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "{}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    status_line,
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn test_parse_data_uri() {
        let (mime, data) = parse_data_uri("data:image/png;base64,abc123").unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(data, "abc123");
    }

    #[test]
    fn test_parse_data_uri_invalid() {
        assert!(parse_data_uri("not-a-data-uri").is_none());
    }

    #[test]
    fn test_build_request_body_text_and_image_input() {
        let model = test_model("https://openrouter.ai/api/v1".into());
        let context = ImagesContext {
            input: vec![
                Content::Text(TextContent {
                    text_signature: None,
                    text: "a cat".into(),
                }),
                Content::Image(ImageContent {
                    data: "ref123".into(),
                    media_type: "image/png".into(),
                }),
            ],
        };
        let body = build_request_body(&model, &context);
        assert_eq!(body["modalities"], serde_json::json!(["image"]));
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
    }

    #[tokio::test]
    async fn test_generate_images_missing_credentials() {
        std::env::remove_var("OPENROUTER_API_KEY");
        let model = test_model("http://127.0.0.1:0".into());
        let context = ImagesContext { input: vec![] };
        let saved = std::env::var("OPENROUTER_API_KEY").ok();
        let result = generate_images(&model, &context, &ImagesOptions::default()).await;
        if let Some(v) = saved {
            std::env::set_var("OPENROUTER_API_KEY", v);
        }
        assert_eq!(result.stop_reason, StopReason::Error);
        assert_eq!(
            result.error_message.as_deref(),
            Some("No API key available for provider: openrouter")
        );
    }

    // --- openrouter-images.test.ts ---

    #[tokio::test]
    async fn returns_text_plus_images_and_the_response_id() {
        let base_url = spawn_mock_server(
            "HTTP/1.1 200 OK",
            r#"{"id":"img-1","usage":{"prompt_tokens":12,"completion_tokens":34,"prompt_tokens_details":{"cached_tokens":0}},"choices":[{"message":{"content":"Here is your image.","images":[{"image_url":"data:image/png;base64,ZmFrZS1wbmc="}]}}]}"#,
        );
        let mut model = test_model(base_url);
        model.output = vec!["text".into(), "image".into()];
        model.headers = Some(
            [(
                "HTTP-Referer".to_string(),
                "https://example.com".to_string(),
            )]
            .into(),
        );
        let context = ImagesContext {
            input: vec![Content::text("Generate a dog")],
        };
        let options = ImagesOptions {
            api_key: Some("test".into()),
            ..Default::default()
        };
        let output = crate::generate_images(&model, &context, &options)
            .await
            .unwrap();
        assert_eq!(output.stop_reason, StopReason::Stop);
        assert_eq!(output.response_id.as_deref(), Some("img-1"));
        assert_eq!(output.output[0], Content::text("Here is your image."));
        assert_eq!(
            output.output[1],
            Content::Image(ImageContent {
                media_type: "image/png".into(),
                data: "ZmFrZS1wbmc=".into(),
            })
        );
        let body = build_request_body(&model, &context);
        assert_eq!(body["stream"], false);
        assert_eq!(body["modalities"], serde_json::json!(["image", "text"]));
        assert_eq!(
            body["messages"][0]["content"][0],
            serde_json::json!({"type": "text", "text": "Generate a dog"})
        );
    }

    #[tokio::test]
    async fn an_aborted_signal_returns_an_aborted_result() {
        let signal = cortexcode_ai_types::AbortSignal::new();
        signal.abort();
        let options = ImagesOptions {
            api_key: Some("test".into()),
            signal: Some(signal),
            ..Default::default()
        };
        let model = test_model("http://127.0.0.1:9".into());
        let output = generate_images(&model, &ImagesContext { input: vec![] }, &options).await;
        assert_eq!(output.stop_reason, StopReason::Aborted);
        assert_eq!(output.error_message.as_deref(), Some("Request was aborted"));
    }

    #[tokio::test]
    async fn unknown_image_apis_are_not_registered() {
        let mut model = test_model("http://127.0.0.1:9".into());
        model.api = "other-images".into();
        let err = crate::generate_images(
            &model,
            &ImagesContext { input: vec![] },
            &ImagesOptions::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, "No API provider registered for api: other-images");
    }

    #[tokio::test]
    async fn test_generate_images_success() {
        let body = "{\"choices\":[{\"message\":{\"content\":\"here is a cat\",\"images\":[{\"image_url\":\"data:image/png;base64,abc123\"}]}}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}";
        let base_url = spawn_mock_server("HTTP/1.1 200 OK", body);
        let model = test_model(base_url);
        let context = ImagesContext {
            input: vec![Content::Text(TextContent {
                text_signature: None,
                text: "a cat".into(),
            })],
        };
        let options = ImagesOptions {
            api_key: Some("or-key".into()),
            ..Default::default()
        };

        let result = generate_images(&model, &context, &options).await;
        assert_eq!(result.stop_reason, StopReason::Stop);
        assert_eq!(result.output.len(), 2);
        match &result.output[0] {
            Content::Text(t) => assert_eq!(t.text, "here is a cat"),
            other => panic!("expected text, got {other:?}"),
        }
        match &result.output[1] {
            Content::Image(img) => {
                assert_eq!(img.media_type, "image/png");
                assert_eq!(img.data, "abc123");
            }
            other => panic!("expected image, got {other:?}"),
        }
        let usage = result.usage.unwrap();
        assert_eq!(usage.input, 10);
        assert_eq!(usage.output, 5);
    }

    #[tokio::test]
    async fn test_generate_images_http_error() {
        let base_url = spawn_mock_server(
            "HTTP/1.1 429 Too Many Requests",
            "{\"error\":\"rate limited\"}",
        );
        let model = test_model(base_url);
        let context = ImagesContext { input: vec![] };
        let options = ImagesOptions {
            api_key: Some("or-key".into()),
            max_retries: Some(0),
            ..Default::default()
        };

        let result = generate_images(&model, &context, &options).await;
        assert_eq!(result.stop_reason, StopReason::Error);
        assert!(result.error_message.unwrap().starts_with("429 "));
    }
}
