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

/// A non-empty environment variable (JS truthiness of `process.env[name]`).
fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Whether Vertex AI Application Default Credentials exist: the file named by
/// `GOOGLE_APPLICATION_CREDENTIALS` when set, else the default
/// `~/.config/gcloud/application_default_credentials.json`. Cached for the
/// process, as in TS.
fn has_vertex_adc_credentials() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(check_vertex_adc_credentials)
}

fn check_vertex_adc_credentials() -> bool {
    if let Some(gac_path) = env_value("GOOGLE_APPLICATION_CREDENTIALS") {
        return std::path::Path::new(&gac_path).exists();
    }
    dirs::home_dir().is_some_and(|home| {
        home.join(".config")
            .join("gcloud")
            .join("application_default_credentials.json")
            .exists()
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// `findEnvKeys`: the configured environment variables that can provide an
/// API key for `provider`, in precedence order; `None` when none is set (an
/// empty value counts as unset) or the provider has no key variable.
///
/// Only actual API key variables are reported, never ambient credential
/// sources such as Google Application Default Credentials.
pub fn find_env_keys(provider: &str) -> Option<Vec<String>> {
    let found: Vec<String> = get_api_key_env_vars(provider)?
        .iter()
        .filter(|var| env_value(var).is_some())
        .map(|s| s.to_string())
        .collect();
    (!found.is_empty()).then_some(found)
}

/// `getEnvApiKey`: the API key for `provider` from its environment variable
/// (e.g. `OPENAI_API_KEY`).
///
/// For `google-vertex` without `GOOGLE_CLOUD_API_KEY`, returns
/// `"<authenticated>"` when Application Default Credentials exist and
/// `GOOGLE_CLOUD_PROJECT` (or `GCLOUD_PROJECT`) and `GOOGLE_CLOUD_LOCATION`
/// are set.
pub fn get_env_api_key(provider: &str) -> Option<String> {
    if let Some(keys) = find_env_keys(provider) {
        return env_value(&keys[0]);
    }

    if provider == "google-vertex" {
        let has_credentials = has_vertex_adc_credentials();
        let has_project =
            env_value("GOOGLE_CLOUD_PROJECT").is_some() || env_value("GCLOUD_PROJECT").is_some();
        let has_location = env_value("GOOGLE_CLOUD_LOCATION").is_some();
        if has_credentials && has_project && has_location {
            return Some("<authenticated>".into());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// Serializes tests that modify environment variables.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` with `vars` set (`None` = removed), restoring them afterwards.
    fn with_vars<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
        let _lock = env_lock();
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let out = f();
        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
        out
    }

    const COPILOT_CLEAR: [(&str, Option<&str>); 3] = [
        ("COPILOT_GITHUB_TOKEN", None),
        ("GH_TOKEN", None),
        ("GITHUB_TOKEN", None),
    ];

    // --- env-api-keys.test.ts ---

    #[test]
    fn does_not_detect_copilot_from_gh_token_alone() {
        let mut vars = COPILOT_CLEAR.to_vec();
        vars.push(("GH_TOKEN", Some("gh-repo-token")));
        with_vars(&vars, || {
            assert_eq!(find_env_keys("github-copilot"), None);
            assert_eq!(get_env_api_key("github-copilot"), None);
        });
    }

    #[test]
    fn does_not_detect_copilot_from_github_token_alone() {
        let mut vars = COPILOT_CLEAR.to_vec();
        vars.push(("GITHUB_TOKEN", Some("ci-token")));
        with_vars(&vars, || assert_eq!(find_env_keys("github-copilot"), None));
    }

    #[test]
    fn detects_copilot_from_explicit_copilot_github_token() {
        let mut vars = COPILOT_CLEAR.to_vec();
        vars.push(("COPILOT_GITHUB_TOKEN", Some("copilot-token")));
        with_vars(&vars, || {
            assert_eq!(
                find_env_keys("github-copilot"),
                Some(vec!["COPILOT_GITHUB_TOKEN".to_string()])
            );
            assert_eq!(
                get_env_api_key("github-copilot").as_deref(),
                Some("copilot-token")
            );
        });
    }

    // --- fireworks-models.test.ts / together-models.test.ts (env halves) ---

    #[test]
    fn resolves_fireworks_and_together_keys() {
        with_vars(
            &[
                ("FIREWORKS_API_KEY", Some("test-fireworks-key")),
                ("TOGETHER_API_KEY", Some("test-together-key")),
            ],
            || {
                assert_eq!(
                    find_env_keys("fireworks"),
                    Some(vec!["FIREWORKS_API_KEY".to_string()])
                );
                assert_eq!(
                    get_env_api_key("fireworks").as_deref(),
                    Some("test-fireworks-key")
                );
                assert_eq!(
                    find_env_keys("together"),
                    Some(vec!["TOGETHER_API_KEY".to_string()])
                );
                assert_eq!(
                    get_env_api_key("together").as_deref(),
                    Some("test-together-key")
                );
            },
        );
    }

    // --- env-api-keys.ts behaviour ---

    /// Every `KnownProvider` and its key variables, as in `getApiKeyEnvVars`.
    /// OAuth-only providers have none.
    #[test]
    fn every_known_provider_maps_like_hoocode() {
        let table: &[(&str, &[&str])] = &[
            ("anthropic", &["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]),
            ("google", &["GEMINI_API_KEY"]),
            ("google-gemini-cli", &[]),
            ("google-antigravity", &[]),
            ("google-vertex", &["GOOGLE_CLOUD_API_KEY"]),
            ("openai", &["OPENAI_API_KEY"]),
            ("azure-openai-responses", &["AZURE_OPENAI_API_KEY"]),
            ("openai-codex", &[]),
            ("deepseek", &["DEEPSEEK_API_KEY"]),
            ("github-copilot", &["COPILOT_GITHUB_TOKEN"]),
            ("xai", &["XAI_API_KEY"]),
            ("groq", &["GROQ_API_KEY"]),
            ("cerebras", &["CEREBRAS_API_KEY"]),
            ("openrouter", &["OPENROUTER_API_KEY"]),
            ("vercel-ai-gateway", &["AI_GATEWAY_API_KEY"]),
            ("zai", &["ZAI_API_KEY"]),
            ("minimax", &["MINIMAX_API_KEY"]),
            ("minimax-cn", &["MINIMAX_CN_API_KEY"]),
            ("moonshotai", &["MOONSHOT_API_KEY"]),
            ("moonshotai-cn", &["MOONSHOT_API_KEY"]),
            ("huggingface", &["HF_TOKEN"]),
            ("fireworks", &["FIREWORKS_API_KEY"]),
            ("together", &["TOGETHER_API_KEY"]),
            ("opencode", &["OPENCODE_API_KEY"]),
            ("opencode-go", &["OPENCODE_API_KEY"]),
            ("kimi-coding", &["KIMI_API_KEY"]),
            ("xiaomi", &["XIAOMI_API_KEY"]),
            ("xiaomi-token-plan-cn", &["XIAOMI_TOKEN_PLAN_CN_API_KEY"]),
            ("xiaomi-token-plan-ams", &["XIAOMI_TOKEN_PLAN_AMS_API_KEY"]),
            ("xiaomi-token-plan-sgp", &["XIAOMI_TOKEN_PLAN_SGP_API_KEY"]),
            ("nvidia", &["NVIDIA_API_KEY"]),
        ];
        assert_eq!(table.len(), 31);
        for (provider, vars) in table {
            let expected = (!vars.is_empty()).then_some(*vars);
            assert_eq!(get_api_key_env_vars(provider), expected, "{provider}");
        }
        assert_eq!(get_api_key_env_vars("mistral"), None);
        assert_eq!(get_api_key_env_vars("nonexistent-provider"), None);
    }

    #[test]
    fn anthropic_oauth_token_takes_precedence() {
        with_vars(
            &[
                ("ANTHROPIC_OAUTH_TOKEN", Some("oauth")),
                ("ANTHROPIC_API_KEY", Some("key")),
            ],
            || {
                assert_eq!(
                    find_env_keys("anthropic"),
                    Some(vec![
                        "ANTHROPIC_OAUTH_TOKEN".to_string(),
                        "ANTHROPIC_API_KEY".to_string()
                    ])
                );
                assert_eq!(get_env_api_key("anthropic").as_deref(), Some("oauth"));
            },
        );
    }

    #[test]
    fn empty_values_count_as_unset() {
        with_vars(
            &[
                ("ANTHROPIC_OAUTH_TOKEN", Some("")),
                ("ANTHROPIC_API_KEY", Some("key")),
                ("OPENAI_API_KEY", Some("")),
            ],
            || {
                assert_eq!(
                    find_env_keys("anthropic"),
                    Some(vec!["ANTHROPIC_API_KEY".to_string()])
                );
                assert_eq!(get_env_api_key("anthropic").as_deref(), Some("key"));
                assert_eq!(find_env_keys("openai"), None);
                assert_eq!(get_env_api_key("openai"), None);
            },
        );
    }

    #[test]
    fn google_vertex_needs_project_and_location_for_adc() {
        with_vars(
            &[
                ("GOOGLE_CLOUD_API_KEY", None),
                ("GOOGLE_CLOUD_PROJECT", None),
                ("GCLOUD_PROJECT", None),
                ("GOOGLE_CLOUD_LOCATION", None),
            ],
            || assert_eq!(get_env_api_key("google-vertex"), None),
        );
        with_vars(&[("GOOGLE_CLOUD_API_KEY", Some("vertex-key"))], || {
            assert_eq!(
                get_env_api_key("google-vertex").as_deref(),
                Some("vertex-key")
            )
        });
    }

    #[test]
    fn unknown_provider_has_no_key() {
        assert_eq!(find_env_keys("nonexistent"), None);
        assert_eq!(get_env_api_key("nonexistent"), None);
    }
}
