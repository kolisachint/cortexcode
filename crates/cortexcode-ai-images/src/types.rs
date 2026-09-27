//! Shared types for image-generation providers.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `types.ts` (the
//! `Images*` family of types).

use std::collections::HashMap;

use cortexcode_ai_types::{
    AbortSignal, Content, Cost, ModelCost, OnPayload, OnResponse, StopReason, Usage,
};

/// An image-generation model definition (hoocode `ImagesModel`, same JSON shape).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub api: String,
    pub provider: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// Input modalities (`"text"`, optionally `"image"`).
    #[serde(default)]
    pub input: Vec<String>,
    /// Output modalities this model can produce (`"image"`, optionally `"text"`).
    pub output: Vec<String>,
    #[serde(default)]
    pub cost: ModelCost,
}

/// Input to an image-generation request: text and/or reference images.
#[derive(Debug, Clone)]
pub struct ImagesContext {
    pub input: Vec<Content>,
}

/// Options for an image-generation request (`ProviderImagesOptions`).
#[derive(Debug, Clone, Default)]
pub struct ImagesOptions {
    pub api_key: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub signal: Option<AbortSignal>,
    pub timeout_ms: Option<u64>,
    /// SDK client retries (default 2).
    pub max_retries: Option<u32>,
    pub on_payload: Option<OnPayload<ImagesModel>>,
    pub on_response: Option<OnResponse<ImagesModel>>,
}

/// Result of an image-generation request.
#[derive(Debug, Clone)]
pub struct AssistantImages {
    pub api: String,
    pub provider: String,
    pub model: String,
    pub output: Vec<Content>,
    pub response_id: Option<String>,
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
