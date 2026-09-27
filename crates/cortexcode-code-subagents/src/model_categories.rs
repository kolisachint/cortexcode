//! `core/model-categories.ts`: `fast` | `standard` | `capable` model
//! categories for subagent model selection.
//!
//! An explicit `modelCategories[category]` setting wins; otherwise, given the
//! available models, the category resolves to a default derived from them
//! (never a hardcoded provider/model); otherwise it is `None` ("no override").

use std::cmp::Ordering;

use cortexcode_ai_types::Model;
use cortexcode_code_settings::{ModelCategories, SettingsManager};

/// `ModelCategory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCategory {
    Fast,
    Standard,
    Capable,
}

impl ModelCategory {
    /// `isModelCategory`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fast" => Some(Self::Fast),
            "standard" => Some(Self::Standard),
            "capable" => Some(Self::Capable),
            _ => None,
        }
    }
}

/// `isModelCategory`.
pub fn is_model_category(value: &str) -> bool {
    ModelCategory::parse(value).is_some()
}

/// The settings the resolution reads (`Settings` fields).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CategorySettings {
    pub model_categories: Option<ModelCategories>,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
}

impl CategorySettings {
    /// From a settings object (`modelCategories`, `defaultProvider`,
    /// `defaultModel`).
    pub fn from_settings(settings: &serde_json::Map<String, serde_json::Value>) -> Self {
        let text = |value: Option<&serde_json::Value>| {
            value
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        let model_categories = settings
            .get("modelCategories")
            .and_then(serde_json::Value::as_object)
            .map(|c| ModelCategories {
                fast: text(c.get("fast")),
                standard: text(c.get("standard")),
                capable: text(c.get("capable")),
            });
        Self {
            model_categories,
            default_provider: text(settings.get("defaultProvider")),
            default_model: text(settings.get("defaultModel")),
        }
    }

    pub fn from_manager(settings: &SettingsManager) -> Self {
        Self {
            model_categories: settings.model_categories(),
            default_provider: settings.default_provider(),
            default_model: settings.default_model(),
        }
    }
}

/// `deriveDefaultModelCategories` result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DerivedCategories {
    pub fast: Option<String>,
    pub standard: Option<String>,
    pub capable: Option<String>,
}

impl DerivedCategories {
    fn get(&self, category: ModelCategory) -> Option<String> {
        match category {
            ModelCategory::Fast => self.fast.clone(),
            ModelCategory::Standard => self.standard.clone(),
            ModelCategory::Capable => self.capable.clone(),
        }
    }
}

fn model_ref(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

/// Combined per-token price, a capability/cost proxy.
fn combined_price(model: &Model) -> f64 {
    model.cost.input + model.cost.output
}

fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// Derive a default per tier from the available models:
/// `capable` is the configured default model when available, else the
/// priciest (ties: larger context window, then id); `fast` and `standard` are
/// the cheapest and the upper median of the models priced at or below
/// `capable` (ties: smaller context window, then id).
pub fn derive_default_model_categories(
    available: &[Model],
    settings: Option<&CategorySettings>,
) -> DerivedCategories {
    if available.is_empty() {
        return DerivedCategories::default();
    }
    let configured = settings.and_then(|s| match (&s.default_provider, &s.default_model) {
        (Some(provider), Some(model)) => available
            .iter()
            .find(|m| &m.provider == provider && &m.id == model),
        _ => None,
    });
    let capable = configured.unwrap_or_else(|| {
        let mut sorted: Vec<&Model> = available.iter().collect();
        sorted.sort_by(|a, b| {
            cmp_f64(combined_price(b), combined_price(a))
                .then(b.context_window.cmp(&a.context_window))
                .then(a.id.cmp(&b.id))
        });
        sorted[0]
    });
    let capable_price = combined_price(capable);
    let mut candidates: Vec<&Model> = available
        .iter()
        .filter(|m| combined_price(m) <= capable_price)
        .collect();
    candidates.sort_by(|a, b| {
        cmp_f64(combined_price(a), combined_price(b))
            .then(a.context_window.cmp(&b.context_window))
            .then(a.id.cmp(&b.id))
    });
    let fast = candidates.first().copied().unwrap_or(capable);
    let standard = candidates
        .get(candidates.len() / 2)
        .copied()
        .unwrap_or(capable);
    DerivedCategories {
        fast: Some(model_ref(fast)),
        standard: Some(model_ref(standard)),
        capable: Some(model_ref(capable)),
    }
}

/// `resolveModelCategory`.
pub fn resolve_model_category(
    category: ModelCategory,
    settings: Option<&CategorySettings>,
    available: Option<&[Model]>,
) -> Option<String> {
    let explicit = settings
        .and_then(|s| s.model_categories.as_ref())
        .and_then(|c| match category {
            ModelCategory::Fast => c.fast.clone(),
            ModelCategory::Standard => c.standard.clone(),
            ModelCategory::Capable => c.capable.clone(),
        })
        .filter(|m| !m.is_empty());
    if explicit.is_some() {
        return explicit;
    }
    match available {
        Some(models) if !models.is_empty() => {
            derive_default_model_categories(models, settings).get(category)
        }
        _ => None,
    }
}

/// `resolveModelReference`: a category resolves (or is `None`); anything else
/// is a concrete model id or alias and passes through.
pub fn resolve_model_reference(
    model: &str,
    settings: Option<&CategorySettings>,
    available: Option<&[Model]>,
) -> Option<String> {
    match ModelCategory::parse(model) {
        Some(category) => resolve_model_category(category, settings, available),
        None => Some(model.to_string()),
    }
}
