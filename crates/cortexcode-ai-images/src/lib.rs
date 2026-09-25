//! Image generation for cortex AI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/images/*`.

mod models;
mod openrouter;
mod types;

pub use models::{get_image_model, get_image_models, get_image_providers};
pub use openrouter::generate_images as generate_images_openrouter;
pub use types::{AssistantImages, ImagesContext, ImagesModel, ImagesOptions};
