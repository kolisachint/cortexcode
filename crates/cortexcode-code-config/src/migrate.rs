//! One-time data migration from the legacy TypeScript `hoocode` CLI's
//! `~/.hoocode/settings.json` into cortexcode's `~/.cortexcode/config.json`.
//!
//! HooCode's global settings file uses a much larger schema (see
//! `packages/coding-agent/src/core/settings-types.ts` in the hoocode repo).
//! Fields that map directly onto [`Config`] are converted to their typed
//! equivalents; every other field is preserved verbatim in [`Config::extra`]
//! so no user configuration is silently dropped.

use crate::{Config, ConfigError};
use std::path::{Path, PathBuf};

/// Return the legacy hoocode config directory (`$HOME/.hoocode`).
pub fn legacy_config_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join(".hoocode"))
        .unwrap_or_else(|| PathBuf::from(".hoocode"))
}

/// Return the legacy hoocode global settings file (`$HOME/.hoocode/settings.json`).
pub fn legacy_settings_path() -> PathBuf {
    legacy_config_dir().join("settings.json")
}

/// Convert a parsed legacy `settings.json` document into a [`Config`].
///
/// Known fields (`defaultProvider`, `defaultModel`, `defaultThinkingLevel`)
/// are mapped onto their typed `Config` counterparts. Every other top-level
/// key is copied into `Config::extra` unchanged, so settings this crate does
/// not yet understand survive the migration and remain visible on disk.
pub fn convert_legacy_settings(mut raw: serde_json::Value) -> Config {
    let mut config = Config::default();

    let Some(map) = raw.as_object_mut() else {
        return config;
    };

    if let Some(provider) = map.remove("defaultProvider").and_then(as_string) {
        config.provider = Some(provider);
    }
    if let Some(model) = map.remove("defaultModel").and_then(as_string) {
        config.model = Some(model);
    }
    if let Some(reasoning) = map.remove("defaultThinkingLevel").and_then(as_string) {
        config.reasoning = Some(reasoning);
    }

    for (key, value) in map.iter() {
        config.extra.insert(key.clone(), value.clone());
    }

    config
}

fn as_string(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s),
        _ => None,
    }
}

/// Read and convert the legacy settings file at `path`, if it exists.
///
/// Returns `Ok(None)` when the file does not exist. Malformed JSON is
/// reported as a [`ConfigError`] rather than silently ignored, so migration
/// failures are visible instead of masking user data.
pub fn read_legacy_settings(path: &Path) -> Result<Option<Config>, ConfigError> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let raw: serde_json::Value = serde_json::from_str(&text)?;
    Ok(Some(convert_legacy_settings(raw)))
}

/// Load the effective configuration, auto-migrating from the legacy hoocode
/// settings file on first run.
///
/// Resolution order:
/// 1. If `~/.cortexcode/config.json` already exists, load it (no migration).
/// 2. Otherwise, if `~/.hoocode/settings.json` exists, convert it, persist it
///    to `~/.cortexcode/config.json`, and return the result.
/// 3. Otherwise, return [`Config::default`].
pub fn auto_migrate() -> Result<Config, ConfigError> {
    auto_migrate_from(&crate::default_config_path(), &legacy_settings_path())
}

/// Same as [`auto_migrate`] but with explicit paths, for testing.
pub fn auto_migrate_from(
    new_config_path: &Path,
    legacy_settings_path: &Path,
) -> Result<Config, ConfigError> {
    if new_config_path.exists() {
        return Config::from_file(new_config_path);
    }

    match read_legacy_settings(legacy_settings_path)? {
        Some(config) => {
            config.save(new_config_path)?;
            Ok(config)
        }
        None => Ok(Config::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cortex-migrate-test-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_convert_legacy_settings_maps_known_fields() {
        let raw = serde_json::json!({
            "defaultProvider": "anthropic",
            "defaultModel": "claude-sonnet-5",
            "defaultThinkingLevel": "high",
            "enableSubagent": true,
            "terminal": { "chimeOnTurnComplete": true }
        });
        let config = convert_legacy_settings(raw);
        assert_eq!(config.provider.as_deref(), Some("anthropic"));
        assert_eq!(config.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(config.reasoning.as_deref(), Some("high"));
        assert_eq!(
            config.extra.get("enableSubagent"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            config.extra.get("terminal"),
            Some(&serde_json::json!({ "chimeOnTurnComplete": true }))
        );
        // Mapped fields are not duplicated into `extra`.
        assert!(!config.extra.contains_key("defaultProvider"));
        assert!(!config.extra.contains_key("defaultModel"));
        assert!(!config.extra.contains_key("defaultThinkingLevel"));
    }

    #[test]
    fn test_convert_legacy_settings_non_object_is_default() {
        let config = convert_legacy_settings(serde_json::json!("not an object"));
        assert_eq!(config, Config::default());
    }

    #[test]
    fn test_read_legacy_settings_missing_file() {
        let dir = temp_dir("missing");
        let result = read_legacy_settings(&dir.join("settings.json")).unwrap();
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_auto_migrate_from_creates_new_config_from_legacy() {
        let dir = temp_dir("migrate");
        let legacy_path = dir.join("hoocode-settings.json");
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "defaultProvider": "anthropic",
                "defaultModel": "claude-sonnet-5"
            })
            .to_string(),
        )
        .unwrap();
        let new_path = dir.join("cortexcode-config.json");

        let config = auto_migrate_from(&new_path, &legacy_path).unwrap();
        assert_eq!(config.provider.as_deref(), Some("anthropic"));
        assert!(new_path.exists());

        // Persisted file round-trips.
        let reloaded = Config::from_file(&new_path).unwrap();
        assert_eq!(reloaded, config);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_auto_migrate_from_prefers_existing_new_config() {
        let dir = temp_dir("prefer-existing");
        let legacy_path = dir.join("hoocode-settings.json");
        std::fs::write(
            &legacy_path,
            serde_json::json!({ "defaultModel": "legacy-model" }).to_string(),
        )
        .unwrap();

        let new_path = dir.join("cortexcode-config.json");
        let existing = Config {
            model: Some("already-migrated-model".into()),
            ..Default::default()
        };
        existing.save(&new_path).unwrap();

        let config = auto_migrate_from(&new_path, &legacy_path).unwrap();
        assert_eq!(config.model.as_deref(), Some("already-migrated-model"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_auto_migrate_from_no_legacy_no_new_returns_default() {
        let dir = temp_dir("no-files");
        let config = auto_migrate_from(
            &dir.join("cortexcode-config.json"),
            &dir.join("hoocode-settings.json"),
        )
        .unwrap();
        assert_eq!(config, Config::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
