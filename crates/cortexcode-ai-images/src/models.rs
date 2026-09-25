//! Image-model registry: port of hoocode `packages/ai/src/image-models.ts`
//! over `cortexcode-ai-models-catalog` (`image-models.generated.ts` at the pin).
//! Providers and models keep the catalog's order.

use std::sync::OnceLock;

use crate::types::ImagesModel;

fn registry() -> &'static Vec<(String, Vec<ImagesModel>)> {
    static REGISTRY: OnceLock<Vec<(String, Vec<ImagesModel>)>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let models: Vec<ImagesModel> =
            serde_json::from_str(cortexcode_ai_models_catalog::IMAGE_MODELS_JSON)
                .expect("the embedded image-model catalog matches the ImagesModel shape");
        let mut providers: Vec<(String, Vec<ImagesModel>)> = Vec::new();
        for model in models {
            match providers.iter_mut().find(|(p, _)| *p == model.provider) {
                Some((_, list)) => match list.iter_mut().find(|m| m.id == model.id) {
                    Some(existing) => *existing = model,
                    None => list.push(model),
                },
                None => providers.push((model.provider.clone(), vec![model])),
            }
        }
        providers
    })
}

/// `getImageModel(provider, modelId)`.
pub fn get_image_model(provider: &str, model_id: &str) -> Option<&'static ImagesModel> {
    registry()
        .iter()
        .find(|(p, _)| p == provider)?
        .1
        .iter()
        .find(|m| m.id == model_id)
}

/// `getImageProviders()`.
pub fn get_image_providers() -> Vec<&'static str> {
    registry().iter().map(|(p, _)| p.as_str()).collect()
}

/// `getImageModels(provider)`.
pub fn get_image_models(provider: &str) -> Vec<&'static ImagesModel> {
    registry()
        .iter()
        .find(|(p, _)| p == provider)
        .map(|(_, models)| models.iter().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_catalog_matches_the_pin() {
        assert_eq!(get_image_providers(), ["openrouter"]);
        assert_eq!(get_image_models("openrouter").len(), 57);
        let flux = get_image_model("openrouter", "black-forest-labs/flux.2-flex").unwrap();
        assert_eq!(flux.name, "Black Forest Labs: FLUX.2 Flex");
        assert_eq!(flux.api, "openrouter-images");
        assert_eq!(flux.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(flux.input, ["text", "image"]);
        assert_eq!(flux.output, ["image"]);
        assert!(get_image_model("openrouter", "missing").is_none());
        assert!(get_image_models("nobody").is_empty());
    }
}
