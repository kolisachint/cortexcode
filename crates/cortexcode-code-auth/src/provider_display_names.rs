//! `core/provider-display-names.ts`: how `/login` names the built-in API-key
//! providers.

/// `BUILT_IN_PROVIDER_DISPLAY_NAMES`.
pub const BUILT_IN_PROVIDER_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("anthropic", "Anthropic"),
    ("azure-openai-responses", "Azure OpenAI Responses"),
    ("cerebras", "Cerebras"),
    ("deepseek", "DeepSeek"),
    ("fireworks", "Fireworks"),
    ("google", "Google Gemini"),
    ("google-vertex", "Google Vertex AI"),
    ("groq", "Groq"),
    ("huggingface", "Hugging Face"),
    ("kimi-coding", "Kimi For Coding"),
    ("minimax", "MiniMax"),
    ("minimax-cn", "MiniMax (China)"),
    ("moonshotai", "Moonshot AI"),
    ("moonshotai-cn", "Moonshot AI (China)"),
    ("opencode", "OpenCode Zen"),
    ("opencode-go", "OpenCode Go"),
    ("openai", "OpenAI"),
    ("openrouter", "OpenRouter"),
    ("together", "Together AI"),
    ("vercel-ai-gateway", "Vercel AI Gateway"),
    ("xai", "xAI"),
    ("zai", "ZAI"),
    ("xiaomi", "Xiaomi MiMo"),
    ("xiaomi-token-plan-cn", "Xiaomi MiMo Token Plan (China)"),
    (
        "xiaomi-token-plan-ams",
        "Xiaomi MiMo Token Plan (Amsterdam)",
    ),
    (
        "xiaomi-token-plan-sgp",
        "Xiaomi MiMo Token Plan (Singapore)",
    ),
    ("nvidia", "NVIDIA"),
];

/// The display name of a built-in API-key provider.
pub fn built_in_provider_display_name(provider: &str) -> Option<&'static str> {
    BUILT_IN_PROVIDER_DISPLAY_NAMES
        .iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, name)| *name)
}

/// `ModelRegistry.getProviderDisplayName`: an OAuth provider's name, else
/// the built-in display name, else the id. (Extension-registered provider
/// names join this once `registerProvider` is ported.)
pub fn provider_display_name(auth: &crate::AuthStorage, provider: &str) -> String {
    auth.get_oauth_providers()
        .iter()
        .find(|p| p.id() == provider)
        .map(|p| p.name().to_string())
        .or_else(|| built_in_provider_display_name(provider).map(String::from))
        .unwrap_or_else(|| provider.to_string())
}

/// `ModelRegistry.getProviderAuthStatus`: the storage's status, else the
/// request auth models.json configures (never running a `!command`).
pub fn provider_auth_status(
    auth: &crate::AuthStorage,
    registry: &cortexcode_code_models::ModelRegistry,
    provider: &str,
) -> crate::AuthStatus {
    use crate::{AuthSource, AuthStatus};
    let status = auth.get_auth_status(provider);
    if status.source.is_some() {
        return status;
    }
    let Some(api_key) = registry.provider_api_key_config(provider) else {
        return status;
    };
    if api_key.starts_with('!') {
        return AuthStatus {
            configured: true,
            source: Some(AuthSource::ModelsJsonCommand),
            label: None,
        };
    }
    if std::env::var(api_key).is_ok_and(|v| !v.is_empty()) {
        return AuthStatus {
            configured: true,
            source: Some(AuthSource::Environment),
            label: Some(api_key.to_string()),
        };
    }
    AuthStatus {
        configured: true,
        source: Some(AuthSource::ModelsJsonKey),
        label: None,
    }
}
