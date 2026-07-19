//! Environment and API key handling for cortex AI.
//!
//! Provides functions to discover API keys for various LLM providers
//! from environment variables and Application Default Credentials (ADC).
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `env-api-keys.ts`.

// ---------------------------------------------------------------------------
// Env-var mapping
// ---------------------------------------------------------------------------

/// Returns the environment variable name(s) that can provide an API key for
/// the given `provider`.
///
/// Returns `None` if the provider is not recognized (callers may still attempt
/// a generic lookup or return `None`).
fn get_api_key_env_vars(provider: &str) -> Option<&'static [&'static str]> {
    match provider {
        // github-copilot: only the explicit COPILOT_GITHUB_TOKEN opts a GitHub
        // token into Copilot inference. GH_TOKEN / GITHUB_TOKEN are ambient in
        // CI and GitHub-integrated environments for *repository* access.
        "github-copilot" => Some(&["COPILOT_GITHUB_TOKEN"]),

        // ANTHROPIC_OAUTH_TOKEN takes precedence over ANTHROPIC_API_KEY.
        "anthropic" => Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]),

        "openai" => Some(&["OPENAI_API_KEY"]),
        "azure-openai-responses" => Some(&["AZURE_OPENAI_API_KEY"]),
        "deepseek" => Some(&["DEEPSEEK_API_KEY"]),
        "google" => Some(&["GEMINI_API_KEY"]),
        "google-vertex" => Some(&["GOOGLE_CLOUD_API_KEY"]),
        "groq" => Some(&["GROQ_API_KEY"]),
        "cerebras" => Some(&["CEREBRAS_API_KEY"]),
        "xai" => Some(&["XAI_API_KEY"]),
        "openrouter" => Some(&["OPENROUTER_API_KEY"]),
        "vercel-ai-gateway" => Some(&["AI_GATEWAY_API_KEY"]),
        "zai" => Some(&["ZAI_API_KEY"]),
        "mistral" => Some(&["MISTRAL_API_KEY"]),
        "minimax" => Some(&["MINIMAX_API_KEY"]),
        "minimax-cn" => Some(&["MINIMAX_CN_API_KEY"]),
        "moonshotai" | "moonshotai-cn" => Some(&["MOONSHOT_API_KEY"]),
        "huggingface" => Some(&["HF_TOKEN"]),
        "fireworks" => Some(&["FIREWORKS_API_KEY"]),
        "together" => Some(&["TOGETHER_API_KEY"]),
        "opencode" | "opencode-go" => Some(&["OPENCODE_API_KEY"]),
        "kimi-coding" => Some(&["KIMI_API_KEY"]),
        "xiaomi" => Some(&["XIAOMI_API_KEY"]),
        "xiaomi-token-plan-cn" => Some(&["XIAOMI_TOKEN_PLAN_CN_API_KEY"]),
        "xiaomi-token-plan-ams" => Some(&["XIAOMI_TOKEN_PLAN_AMS_API_KEY"]),
        "xiaomi-token-plan-sgp" => Some(&["XIAOMI_TOKEN_PLAN_SGP_API_KEY"]),
        "nvidia" => Some(&["NVIDIA_API_KEY"]),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Cached ADC check
// ---------------------------------------------------------------------------

static mut VERTEX_ADC_CACHE: Option<bool> = None;

/// Check whether Vertex AI Application Default Credentials are available.
///
/// Checks `GOOGLE_APPLICATION_CREDENTIALS` env var first (standard gcloud
/// auth file), then falls back to the default ADC path
/// `~/.config/gcloud/application_default_credentials.json`.
fn has_vertex_adc_credentials() -> bool {
    // SAFETY: single-threaded idempotent write — safe in practice.
    unsafe {
        if let Some(cached) = VERTEX_ADC_CACHE {
            return cached;
        }
    }

    let result = check_vertex_adc_credentials();

    unsafe {
        VERTEX_ADC_CACHE = Some(result);
    }

    result
}

fn check_vertex_adc_credentials() -> bool {
    // Check GOOGLE_APPLICATION_CREDENTIALS env var first.
    if let Ok(gac_path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        if std::path::Path::new(&gac_path).exists() {
            return true;
        }
    }

    // Fall back to default ADC path.
    let home = dirs::home_dir();
    if let Some(home) = home {
        let default_adc = home
            .join(".config")
            .join("gcloud")
            .join("application_default_credentials.json");
        if default_adc.exists() {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Find which environment variables are set for the given provider's API key.
///
/// Returns `Some(vec)` of env-var names that are currently set in the
/// environment, or `None` when no recognized env vars are set for this
/// provider or the provider is unknown.
///
/// This only reports actual API key variables. It intentionally excludes
/// ambient credential sources such as AWS profiles, AWS IAM credentials,
/// and Google Application Default Credentials.
pub fn find_env_keys(provider: &str) -> Option<Vec<String>> {
    let env_vars = get_api_key_env_vars(provider)?;

    let found: Vec<String> = env_vars
        .iter()
        .filter(|var| std::env::var(var).is_ok())
        .map(|s| s.to_string())
        .collect();

    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// Get the API key for a provider from known environment variables.
///
/// Returns `Some(key)` if a matching env var is found, or `None` otherwise.
///
/// Special cases:
/// - For `"google-vertex"`, returns `Some("<authenticated>")` when ADC
///   credentials are available AND both `GOOGLE_CLOUD_PROJECT` (or
///   `GCLOUD_PROJECT`) and `GOOGLE_CLOUD_LOCATION` env vars are set.
pub fn get_env_api_key(provider: &str) -> Option<String> {
    // Try direct env-var lookup first.
    let env_vars = get_api_key_env_vars(provider);
    if let Some(vars) = env_vars {
        for var in vars {
            if let Ok(val) = std::env::var(var) {
                return Some(val);
            }
        }
    }

    // Vertex AI: support Application Default Credentials.
    if provider == "google-vertex" {
        let has_creds = has_vertex_adc_credentials();
        let has_project = std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GCLOUD_PROJECT"))
            .is_ok();
        let has_location = std::env::var("GOOGLE_CLOUD_LOCATION").is_ok();

        if has_creds && has_project && has_location {
            return Some("<authenticated>".into());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{LazyLock, Mutex};

    /// Serializes tests that modify environment variables to prevent races.
    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn test_anthropic_keys() {
        assert_eq!(
            get_api_key_env_vars("anthropic"),
            Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"][..])
        );
    }

    #[test]
    fn test_openai_key() {
        assert_eq!(
            get_api_key_env_vars("openai"),
            Some(&["OPENAI_API_KEY"][..])
        );
    }

    #[test]
    fn test_unknown_provider() {
        assert_eq!(get_api_key_env_vars("nonexistent-provider"), None);
    }

    #[test]
    fn test_find_env_keys_none_set() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Temporarily remove any known key for openai
        let saved = std::env::var("OPENAI_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        assert_eq!(find_env_keys("openai"), None);
        if let Ok(val) = saved {
            std::env::set_var("OPENAI_API_KEY", val);
        }
    }

    #[test]
    fn test_find_env_keys_found() {
        let saved = std::env::var("OPENAI_API_KEY");
        std::env::set_var("OPENAI_API_KEY", "sk-test123");
        let keys = find_env_keys("openai");
        assert_eq!(keys, Some(vec!["OPENAI_API_KEY".to_string()]));
        // Restore
        match saved {
            Ok(val) => std::env::set_var("OPENAI_API_KEY", val),
            Err(_) => std::env::remove_var("OPENAI_API_KEY"),
        }
    }

    #[test]
    fn test_get_env_api_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("OPENAI_API_KEY");
        std::env::set_var("OPENAI_API_KEY", "sk-test456");
        assert_eq!(get_env_api_key("openai"), Some("sk-test456".into()));
        match saved {
            Ok(val) => std::env::set_var("OPENAI_API_KEY", val),
            Err(_) => std::env::remove_var("OPENAI_API_KEY"),
        }
    }

    #[test]
    fn test_get_env_api_key_unknown() {
        assert_eq!(get_env_api_key("nonexistent"), None);
    }

    #[test]
    fn test_copilot_not_from_gh_token() {
        let _lock = ENV_LOCK.lock().unwrap();
        // GH_TOKEN alone must NOT detect Copilot
        let saved_gh = std::env::var("GH_TOKEN");
        let saved_copilot = std::env::var("COPILOT_GITHUB_TOKEN");
        std::env::remove_var("COPILOT_GITHUB_TOKEN");
        std::env::set_var("GH_TOKEN", "gh-repo-token");

        assert_eq!(find_env_keys("github-copilot"), None);
        assert_eq!(get_env_api_key("github-copilot"), None);

        // Restore
        match saved_copilot {
            Ok(v) => std::env::set_var("COPILOT_GITHUB_TOKEN", v),
            Err(_) => std::env::remove_var("COPILOT_GITHUB_TOKEN"),
        }
        match saved_gh {
            Ok(v) => std::env::set_var("GH_TOKEN", v),
            Err(_) => std::env::remove_var("GH_TOKEN"),
        }
    }

    #[test]
    fn test_copilot_from_explicit_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("COPILOT_GITHUB_TOKEN");
        std::env::set_var("COPILOT_GITHUB_TOKEN", "copilot-token");

        let keys = find_env_keys("github-copilot");
        assert_eq!(keys, Some(vec!["COPILOT_GITHUB_TOKEN".to_string()]));
        assert_eq!(
            get_env_api_key("github-copilot"),
            Some("copilot-token".into())
        );

        match saved {
            Ok(v) => std::env::set_var("COPILOT_GITHUB_TOKEN", v),
            Err(_) => std::env::remove_var("COPILOT_GITHUB_TOKEN"),
        }
    }

    #[test]
    fn test_google_vertex_no_creds() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Without ADC or env vars, should return None
        let saved_project = std::env::var("GOOGLE_CLOUD_PROJECT");
        let saved_location = std::env::var("GOOGLE_CLOUD_LOCATION");
        std::env::remove_var("GOOGLE_CLOUD_PROJECT");
        std::env::remove_var("GOOGLE_CLOUD_LOCATION");

        // get_env_api_key won't find GOOGLE_CLOUD_API_KEY either
        assert_eq!(get_env_api_key("google-vertex"), None);

        match saved_project {
            Ok(v) => std::env::set_var("GOOGLE_CLOUD_PROJECT", v),
            Err(_) => std::env::remove_var("GOOGLE_CLOUD_PROJECT"),
        }
        match saved_location {
            Ok(v) => std::env::set_var("GOOGLE_CLOUD_LOCATION", v),
            Err(_) => std::env::remove_var("GOOGLE_CLOUD_LOCATION"),
        }
    }

    #[test]
    fn test_github_tokens_not_copilot() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Regression: GITHUB_TOKEN alone must NOT detect Copilot
        let saved_gh = std::env::var("GITHUB_TOKEN");
        let saved_copilot = std::env::var("COPILOT_GITHUB_TOKEN");
        std::env::remove_var("COPILOT_GITHUB_TOKEN");
        std::env::set_var("GITHUB_TOKEN", "ci-token");

        assert_eq!(find_env_keys("github-copilot"), None);

        match saved_copilot {
            Ok(v) => std::env::set_var("COPILOT_GITHUB_TOKEN", v),
            Err(_) => std::env::remove_var("COPILOT_GITHUB_TOKEN"),
        }
        match saved_gh {
            Ok(v) => std::env::set_var("GITHUB_TOKEN", v),
            Err(_) => std::env::remove_var("GITHUB_TOKEN"),
        }
    }

    #[test]
    fn test_provider_coverage() {
        // Verify all known providers have an env-var mapping (spot-check a few)
        let providers = [
            "anthropic",
            "openai",
            "google",
            "google-vertex",
            "deepseek",
            "groq",
            "cerebras",
            "xai",
            "openrouter",
            "fireworks",
            "together",
            "huggingface",
            "nvidia",
            "mistral",
            "minimax",
            "moonshotai",
            "kimi-coding",
            "github-copilot",
        ];
        for p in providers {
            assert!(
                get_api_key_env_vars(p).is_some(),
                "provider '{p}' missing env-var mapping"
            );
        }
    }
}
