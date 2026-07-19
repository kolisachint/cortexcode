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

fn resolve_credentials(options: &ImagesOptions) -> Result<String, String> {
    if let Some(key) = &options.api_key {
        if !key.is_empty() {
            return Ok(key.clone());
        }
    }
    if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err("no OpenRouter credentials found (set OPENROUTER_API_KEY)".into())
}

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
    let rest = uri.strip_prefix("data:")?;
    let (media_type, data) = rest.split_once(";base64,")?;
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

/// Generate images via OpenRouter's Chat Completions API.
pub fn generate_images(
    model: &ImagesModel,
    context: &ImagesContext,
    options: &ImagesOptions,
) -> AssistantImages {
    let mut output = AssistantImages {
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        output: vec![],
        stop_reason: StopReason::EndTurn,
        error_message: None,
        usage: None,
        timestamp: now_millis(),
    };

    let api_key = match resolve_credentials(options) {
        Ok(k) => k,
        Err(e) => {
            output.stop_reason = StopReason::Error;
            output.error_message = Some(e);
            return output;
        }
    };

    let result = (|| -> Result<(), String> {
        let client = reqwest::blocking::Client::builder()
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        let url = format!("{}/chat/completions", model.base_url.trim_end_matches('/'));
        let mut request = client
            .post(&url)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {api_key}"));
        if let Some(extra) = &model.headers {
            for (k, v) in extra {
                request = request.header(k, v);
            }
        }
        if let Some(extra) = &options.headers {
            for (k, v) in extra {
                request = request.header(k, v);
            }
        }

        let body = build_request_body(model, context);
        let response = request
            .json(&body)
            .send()
            .map_err(|e| format!("request to OpenRouter API failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            return Err(format!("OpenRouter API returned {status}: {text}"));
        }

        let text = response
            .text()
            .map_err(|e| format!("failed to read response body: {e}"))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("invalid JSON response: {e}; body={text}"))?;

        if let Some(usage) = value.get("usage") {
            output.usage = Some(parse_usage(usage, model));
        }

        let Some(choice) = value["choices"].get(0) else {
            return Ok(());
        };
        let message = &choice["message"];

        if let Some(text) = message["content"].as_str() {
            if !text.is_empty() {
                output.output.push(Content::Text(TextContent {
                    text: text.to_string(),
                    cache_control: None,
                }));
            }
        }

        if let Some(images) = message["images"].as_array() {
            for image in images {
                let image_url = image["image_url"]
                    .as_str()
                    .or_else(|| image["image_url"]["url"].as_str());
                let Some(image_url) = image_url else { continue };
                let Some((media_type, data)) = parse_data_uri(image_url) else {
                    continue;
                };
                output.output.push(Content::Image(ImageContent {
                    media_type,
                    data,
                    cache_control: None,
                }));
            }
        }

        Ok(())
    })();

    if let Err(e) = result {
        output.stop_reason = StopReason::Error;
        output.error_message = Some(e);
    }

    output
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
                    text: "a cat".into(),
                    cache_control: None,
                }),
                Content::Image(ImageContent {
                    data: "ref123".into(),
                    media_type: "image/png".into(),
                    cache_control: None,
                }),
            ],
        };
        let body = build_request_body(&model, &context);
        assert_eq!(body["modalities"], serde_json::json!(["image"]));
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
    }

    #[test]
    fn test_generate_images_missing_credentials() {
        std::env::remove_var("OPENROUTER_API_KEY");
        let model = test_model("http://127.0.0.1:0".into());
        let context = ImagesContext { input: vec![] };
        let result = generate_images(&model, &context, &ImagesOptions::default());
        assert_eq!(result.stop_reason, StopReason::Error);
    }

    #[test]
    fn test_generate_images_success() {
        let body = "{\"choices\":[{\"message\":{\"content\":\"here is a cat\",\"images\":[{\"image_url\":\"data:image/png;base64,abc123\"}]}}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}";
        let base_url = spawn_mock_server("HTTP/1.1 200 OK", body);
        let model = test_model(base_url);
        let context = ImagesContext {
            input: vec![Content::Text(TextContent {
                text: "a cat".into(),
                cache_control: None,
            })],
        };
        let options = ImagesOptions {
            api_key: Some("or-key".into()),
            ..Default::default()
        };

        let result = generate_images(&model, &context, &options);
        assert_eq!(result.stop_reason, StopReason::EndTurn);
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

    #[test]
    fn test_generate_images_http_error() {
        let base_url = spawn_mock_server(
            "HTTP/1.1 429 Too Many Requests",
            "{\"error\":\"rate limited\"}",
        );
        let model = test_model(base_url);
        let context = ImagesContext { input: vec![] };
        let options = ImagesOptions {
            api_key: Some("or-key".into()),
            ..Default::default()
        };

        let result = generate_images(&model, &context, &options);
        assert_eq!(result.stop_reason, StopReason::Error);
        assert!(result.error_message.unwrap().contains("429"));
    }
}
