//! Image generation for cortex AI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/images/*`.

mod models;
mod openrouter;
mod types;

pub use models::{get_image_model, get_image_models, get_image_providers};
pub use openrouter::generate_images as generate_images_openrouter;

/// `generateImages`: dispatch on `model.api` (only `openrouter-images` is
/// built in). An unknown api is an error, as the TS registry lookup throws.
pub async fn generate_images(
    model: &ImagesModel,
    context: &ImagesContext,
    options: &ImagesOptions,
) -> Result<AssistantImages, String> {
    match model.api.as_str() {
        "openrouter-images" => Ok(generate_images_openrouter(model, context, options).await),
        api => Err(format!("No API provider registered for api: {api}")),
    }
}
pub use types::{AssistantImages, ImagesContext, ImagesModel, ImagesOptions};
