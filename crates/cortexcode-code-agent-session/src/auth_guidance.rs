//! `core/auth-guidance.ts`: what to tell the user when no model or key is set.

const UNKNOWN_PROVIDER: &str = "unknown";

fn provider_login_help() -> String {
    let docs = cortexcode_code_paths::docs_path();
    [
        "Use /login to log into a provider via OAuth or API key. See:".to_string(),
        format!("  {}", docs.join("providers.md").display()),
        format!("  {}", docs.join("models.md").display()),
    ]
    .join("\n")
}

/// `formatNoModelsAvailableMessage`.
pub fn format_no_models_available_message() -> String {
    format!("No models available. {}", provider_login_help())
}

/// `formatNoModelSelectedMessage`.
pub fn format_no_model_selected_message() -> String {
    format!(
        "No model selected.\n\n{}\n\nThen use /model to select a model.",
        provider_login_help()
    )
}

/// `formatNoApiKeyFoundMessage`.
pub fn format_no_api_key_found_message(provider: &str) -> String {
    let provider_display = if provider == UNKNOWN_PROVIDER {
        "the selected model"
    } else {
        provider
    };
    format!(
        "No API key found for {provider_display}.\n\n{}",
        provider_login_help()
    )
}
