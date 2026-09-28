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
