//! Image generation for cortex AI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/images/*`.

mod openrouter;
mod types;

pub use openrouter::generate_images as generate_images_openrouter;
pub use types::{AssistantImages, ImagesContext, ImagesModel, ImagesOptions};
