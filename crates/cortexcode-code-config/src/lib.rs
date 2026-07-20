//! Configuration loading, merging, and persistence for the cortex CLI.
//!
//! Mirrors the config handling in the TypeScript `packages/coding-agent` package.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level configuration for the cortex CLI.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Default model identifier (e.g. `claude-sonnet-4`).
    pub model: Option<String>,
    /// Explicit API key (overrides environment detection).
    pub api_key: Option<String>,
    /// Default thinking/reasoning level.
    pub reasoning: Option<String>,
    /// Maximum number of turns before the agent stops.
    pub max_turns: Option<u32>,
    /// Whether to enable the TUI interactive mode by default.
    pub interactive: Option<bool>,
    /// Whether to approve dangerous tools automatically (default false).
    pub auto_approve_dangerous: Option<bool>,
    /// Whether to approve read-only tools automatically (default true).
    pub auto_approve_read_only: Option<bool>,
    /// Per-provider settings.
    pub providers: HashMap<String, ProviderConfig>,
    /// Extra key/value pairs preserved for forward compatibility.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Per-provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct ProviderConfig {
    /// Provider-specific base URL.
    pub base_url: Option<String>,
    /// Provider-specific API key.
    pub api_key: Option<String>,
    /// Extra provider settings.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Config {
    /// Create a default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge another config into this one. Non-None values from `other` win.
    pub fn merge(&mut self, other: Config) {
        if other.model.is_some() {
            self.model = other.model;
        }
        if other.api_key.is_some() {
            self.api_key = other.api_key;
        }
        if other.reasoning.is_some() {
            self.reasoning = other.reasoning;
        }
        if other.max_turns.is_some() {
            self.max_turns = other.max_turns;
        }
        if other.interactive.is_some() {
            self.interactive = other.interactive;
        }
        if other.auto_approve_dangerous.is_some() {
            self.auto_approve_dangerous = other.auto_approve_dangerous;
        }
        if other.auto_approve_read_only.is_some() {
            self.auto_approve_read_only = other.auto_approve_read_only;
        }
        for (key, value) in other.providers {
            self.providers.insert(key, value);
        }
        for (key, value) in other.extra {
            self.extra.insert(key, value);
        }
    }

    /// Load configuration from a JSON file. Missing fields use defaults.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&text)?;
        Ok(config)
    }

    /// Save configuration to a JSON file.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Resolve effective auto-approve flags.
    pub fn auto_approve_dangerous(&self) -> bool {
        self.auto_approve_dangerous.unwrap_or(false)
    }

    /// Resolve effective auto-approve-read-only flag.
    pub fn auto_approve_read_only(&self) -> bool {
        self.auto_approve_read_only.unwrap_or(true)
    }
}

/// Error type for config operations.
#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "io error: {}", e),
            ConfigError::Json(e) => write!(f, "json error: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io(e) => Some(e),
            ConfigError::Json(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io(e)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self {
        ConfigError::Json(e)
    }
}

/// Return the default configuration directory.
///
/// Uses `$HOME/.cortexcode` on Unix-like systems, falls back to `./.cortexcode`.
pub fn default_config_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join(".cortexcode"))
        .unwrap_or_else(|| PathBuf::from(".cortexcode"))
}

/// Return the default config file path.
pub fn default_config_path() -> PathBuf {
    default_config_dir().join("config.json")
}

/// Load the default config file if it exists, otherwise return defaults.
pub fn load_default() -> Result<Config, ConfigError> {
    let path = default_config_path();
    if path.exists() {
        Config::from_file(&path)
    } else {
        Ok(Config::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::new();
        assert!(config.model.is_none());
        assert_eq!(config.auto_approve_read_only(), true);
        assert_eq!(config.auto_approve_dangerous(), false);
    }

    #[test]
    fn test_merge_overrides() {
        let mut base = Config::new();
        base.model = Some("base".into());
        let other = Config {
            model: Some("other".into()),
            ..Default::default()
        };
        base.merge(other);
        assert_eq!(base.model.as_deref(), Some("other"));
    }

    #[test]
    fn test_roundtrip_file() {
        let dir = std::env::temp_dir().join(format!("cortex-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.json");
        let config = Config {
            model: Some("claude".into()),
            interactive: Some(true),
            ..Default::default()
        };
        config.save(&path).unwrap();
        let loaded = Config::from_file(&path).unwrap();
        assert_eq!(config, loaded);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_default_missing() {
        // This test is environment-dependent; it simply verifies no panic.
        let _ = load_default();
    }
}
