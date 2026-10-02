//! Port of the pin's `ai/test/images.test.ts` (OpenRouter image generation,
//! `google/gemini-2.5-flash-image`). Live: `#[ignore]`d, and each case also
//! returns early without `OPENROUTER_API_KEY`, like the TS `describe.skipIf`.

use cortexcode_ai_images::{generate_images, get_image_model, ImagesContext, ImagesOptions};
use cortexcode_ai_types::{Content, ImageContent, StopReason};

const MODEL: &str = "google/gemini-2.5-flash-image";

fn api_key() -> Option<String> {
    std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

fn options(key: String) -> ImagesOptions {
    ImagesOptions {
        api_key: Some(key),
        ..Default::default()
    }
}

fn has_image(output: &[Content]) -> bool {
    output.iter().any(|c| matches!(c, Content::Image(_)))
}

#[tokio::test]
#[ignore = "live: needs OPENROUTER_API_KEY and network"]
async fn generates_a_basic_image() {
    let Some(key) = api_key() else { return };
    let model = get_image_model("openrouter", MODEL).unwrap();
    let context = ImagesContext {
        input: vec![Content::text(
            "Generate a simple red circle on a plain white background. No text.",
        )],
    };
    let r = generate_images(model, &context, &options(key))
        .await
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::Stop, "{:?}", r.error_message);
    assert!(r.error_message.is_none());
    assert!(has_image(&r.output));
    assert!(r.timestamp > 0);
}

#[tokio::test]
#[ignore = "live: needs OPENROUTER_API_KEY and network"]
async fn handles_text_plus_image_output() {
    let Some(key) = api_key() else { return };
    let model = get_image_model("openrouter", MODEL).unwrap();
    if !model.output.iter().any(|o| o == "text") {
        return;
    }
    let context = ImagesContext {
        input: vec![Content::text(
            "Generate a red circle and include a brief description of the image.",
        )],
    };
    let r = generate_images(model, &context, &options(key))
        .await
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::Stop, "{:?}", r.error_message);
    assert!(has_image(&r.output));
    assert!(r
        .output
        .iter()
        .any(|c| matches!(c, Content::Text(t) if !t.text.trim().is_empty())));
}

#[tokio::test]
#[ignore = "live: needs OPENROUTER_API_KEY and network"]
async fn handles_image_input() {
    use base64::Engine as _;
    let Some(key) = api_key() else { return };
    let model = get_image_model("openrouter", MODEL).unwrap();
    if !model.input.iter().any(|i| i == "image") {
        return;
    }
    let png = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/red-circle.png"
    ))
    .unwrap();
    let context = ImagesContext {
        input: vec![
            Content::text("Create a variation of this image with a blue background."),
            Content::Image(ImageContent {
                data: base64::engine::general_purpose::STANDARD.encode(png),
                media_type: "image/png".into(),
            }),
        ],
    };
    let r = generate_images(model, &context, &options(key))
        .await
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::Stop, "{:?}", r.error_message);
    assert!(has_image(&r.output));
}
