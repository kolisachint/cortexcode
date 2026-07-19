//! Shared types for image-generation providers.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `types.ts` (the
//! `Images*` family of types).

use std::collections::HashMap;

use cortexcode_ai_types::{Content, Cost, ModelCost, StopReason, Usage};

/// An image-generation model definition.
#[derive(Debug, Clone)]
pub struct ImagesModel {
    pub id: String,
    pub api: String,
    pub provider: String,
    pub base_url: String,
    pub headers: Option<HashMap<String, String>>,
    /// Output modalities this model can produce (`"image"`, optionally `"text"`).
    pub output: Vec<String>,
    pub cost: ModelCost,
}

/// Input to an image-generation request: text and/or reference images.
#[derive(Debug, Clone)]
pub struct ImagesContext {
    pub input: Vec<Content>,
}

/// Options for an image-generation request.
#[derive(Debug, Clone, Default)]
pub struct ImagesOptions {
    pub api_key: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

/// Result of an image-generation request.
#[derive(Debug, Clone)]
pub struct AssistantImages {
    pub api: String,
    pub provider: String,
    pub model: String,
    pub output: Vec<Content>,
    pub stop_reason: StopReason,
    pub error_message: Option<String>,
    pub usage: Option<Usage>,
    pub timestamp: i64,
}

pub(crate) fn calculate_image_cost(model_cost: &ModelCost, usage: &Usage) -> Cost {
    let input = (model_cost.input / 1_000_000.0) * usage.input as f64;
    let output = (model_cost.output / 1_000_000.0) * usage.output as f64;
    let cache_read = (model_cost.cache_read / 1_000_000.0) * usage.cache_read as f64;
    let cache_write = (model_cost.cache_write / 1_000_000.0) * usage.cache_write as f64;
    Cost {
        input,
        output,
        cache_read,
        cache_write,
        total: input + output + cache_read + cache_write,
    }
}
