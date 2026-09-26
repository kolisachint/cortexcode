//! Umbrella crate for the cortex AI namespace.
//!
//! Re-exports every `cortexcode-ai-*` leaf crate so callers can depend on a
//! single `cortexcode-ai` crate instead of naming each leaf individually.

pub use cortexcode_ai_env as env;
pub use cortexcode_ai_images as images;
pub use cortexcode_ai_models as models;
pub use cortexcode_ai_oauth as oauth;
pub use cortexcode_ai_oauth_anthropic as oauth_anthropic;
pub use cortexcode_ai_oauth_github_copilot as oauth_github_copilot;
pub use cortexcode_ai_oauth_google as oauth_google;
pub use cortexcode_ai_oauth_openai_codex as oauth_openai_codex;
pub use cortexcode_ai_provider_anthropic as provider_anthropic;
pub use cortexcode_ai_provider_azure as provider_azure;
pub use cortexcode_ai_provider_faux as provider_faux;
pub use cortexcode_ai_provider_google as provider_google;
pub use cortexcode_ai_provider_google_gemini_cli as provider_google_gemini_cli;
pub use cortexcode_ai_provider_openai as provider_openai;
pub use cortexcode_ai_provider_openai_codex as provider_openai_codex;
pub use cortexcode_ai_provider_openai_responses as provider_openai_responses;
pub use cortexcode_ai_registry as registry;
pub use cortexcode_ai_stream as stream;
pub use cortexcode_ai_types as types;
pub use cortexcode_ai_util as util;

use std::sync::Arc;

/// `BUILT_IN_OAUTH_PROVIDERS` of `utils/oauth/index.ts`, in its order.
pub fn builtin_oauth_providers() -> Vec<Arc<dyn oauth::OAuthProvider>> {
    vec![
        Arc::new(oauth_anthropic::AnthropicOAuthProvider::default()),
        Arc::new(oauth_github_copilot::GitHubCopilotOAuthProvider::default()),
        Arc::new(oauth_google::GeminiCliOAuthProvider::default()),
        Arc::new(oauth_google::AntigravityOAuthProvider::default()),
        Arc::new(oauth_openai_codex::OpenAICodexOAuthProvider::default()),
    ]
}

/// Register the built-in OAuth providers (hoocode's registry starts with
/// them; the leaf crates cannot register themselves).
pub fn install_builtin_oauth_providers() {
    oauth::install_builtin_oauth_providers(builtin_oauth_providers());
}
