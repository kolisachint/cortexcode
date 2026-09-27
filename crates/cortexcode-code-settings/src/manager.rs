//! `settings-manager.ts`: `SettingsManager`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cortexcode_ai_types::{ThinkingBudgets, Transport};
use serde_json::{Map, Value};

use crate::storage::{
    Error, FileSettingsStorage, InMemorySettingsStorage, SettingsError, SettingsScope,
    SettingsStorage,
};
use crate::types::*;

/// `deepMergeSettings`: `overrides` win; nested objects merge one level deep.
pub fn deep_merge_settings(base: &Settings, overrides: &Settings) -> Settings {
    let mut result = base.clone();
    for (key, override_value) in overrides {
        match (override_value, base.get(key)) {
            (Value::Object(over), Some(Value::Object(base_nested))) => {
                let mut merged = base_nested.clone();
                for (k, v) in over {
                    merged.insert(k.clone(), v.clone());
                }
                result.insert(key.clone(), Value::Object(merged));
            }
            _ => {
                result.insert(key.clone(), override_value.clone());
            }
        }
    }
    result
}

/// `migrateSettings`: rewrite retired keys into their current form.
pub fn migrate_settings(mut settings: Settings) -> Settings {
    if settings.contains_key("queueMode") && !settings.contains_key("steeringMode") {
        let value = settings.shift_remove("queueMode").unwrap_or(Value::Null);
        settings.insert("steeringMode".into(), value);
    }

    // The tool rename was a clean break, but this key is a user's stored
    // choice: dropping it would silently turn indexing back on.
    if settings.contains_key("enableEmbsearchTools")
        && !settings.contains_key("enableSemanticIndex")
    {
        let value = settings
            .shift_remove("enableEmbsearchTools")
            .unwrap_or(Value::Null);
        settings.insert("enableSemanticIndex".into(), value);
    }

    if !settings.contains_key("transport") {
        if let Some(websockets) = settings.get("websockets").and_then(Value::as_bool) {
            let transport = if websockets { "websocket" } else { "sse" };
            settings.insert("transport".into(), transport.into());
            settings.shift_remove("websockets");
        }
    }

    // Old `skills: { enableSkillCommands, customDirectories }` object.
    if let Some(Value::Object(skills)) = settings.get("skills").cloned() {
        if let Some(enable) = skills.get("enableSkillCommands") {
            if !settings.contains_key("enableSkillCommands") {
                settings.insert("enableSkillCommands".into(), enable.clone());
            }
        }
        match skills.get("customDirectories") {
            Some(Value::Array(dirs)) if !dirs.is_empty() => {
                settings.insert("skills".into(), Value::Array(dirs.clone()));
            }
            _ => {
                settings.shift_remove("skills");
            }
        }
    }

    // retry.maxDelayMs -> retry.provider.maxRetryDelayMs
    if let Some(Value::Object(retry)) = settings.get_mut("retry") {
        let provider = retry.get("provider").and_then(Value::as_object).cloned();
        if let Some(max_delay) = retry.get("maxDelayMs").filter(|v| v.is_number()).cloned() {
            let unset = provider
                .as_ref()
                .and_then(|p| p.get("maxRetryDelayMs"))
                .is_none_or(Value::is_null);
            if unset {
                let mut provider = provider.unwrap_or_default();
                provider.insert("maxRetryDelayMs".into(), max_delay);
                retry.insert("provider".into(), Value::Object(provider));
            }
        }
        retry.shift_remove("maxDelayMs");
    }

    settings
}

fn parse_settings(content: &str) -> Result<Settings, Error> {
    match serde_json::from_str(content)? {
        Value::Object(map) => Ok(migrate_settings(map)),
        _ => Err(Error::NotAnObject),
    }
}

fn load_from_storage(
    storage: &dyn SettingsStorage,
    scope: SettingsScope,
) -> Result<Settings, Error> {
    let mut content = None;
    storage.with_lock(scope, &mut |current| {
        content = current.map(str::to_owned);
        Ok(None)
    })?;
    match content.as_deref() {
        None | Some("") => Ok(Settings::new()),
        Some(content) => parse_settings(content),
    }
}

/// Fields changed this session, in first-change order (`modifiedFields` /
/// `modifiedNestedFields`).
#[derive(Debug, Clone, Default)]
struct Modified {
    fields: Vec<String>,
    nested: Vec<(String, Vec<String>)>,
}

impl Modified {
    fn mark(&mut self, field: &str, nested_key: Option<&str>) {
        if !self.fields.iter().any(|f| f == field) {
            self.fields.push(field.to_owned());
        }
        if let Some(nested_key) = nested_key {
            let keys = match self.nested.iter().position(|(f, _)| f == field) {
                Some(i) => &mut self.nested[i].1,
                None => {
                    self.nested.push((field.to_owned(), Vec::new()));
                    &mut self.nested.last_mut().expect("just pushed").1
                }
            };
            if !keys.iter().any(|k| k == nested_key) {
                keys.push(nested_key.to_owned());
            }
        }
    }

    fn nested_keys(&self, field: &str) -> Option<&[String]> {
        self.nested
            .iter()
            .find(|(f, _)| f == field)
            .map(|(_, keys)| keys.as_slice())
    }

    fn clear(&mut self) {
        self.fields.clear();
        self.nested.clear();
    }
}

fn set_or_remove(map: &mut Settings, key: &str, value: Option<Value>) {
    match value {
        Some(value) => {
            map.insert(key.to_owned(), value);
        }
        None => {
            map.shift_remove(key);
        }
    }
}

/// Non-null value (`??` treats `null` like `undefined`).
fn defined(value: Option<&Value>) -> Option<&Value> {
    value.filter(|v| !v.is_null())
}

fn finite(value: Option<&Value>) -> Option<f64> {
    defined(value)?.as_f64().filter(|v| v.is_finite())
}

fn whole(value: Option<&Value>) -> Option<u64> {
    finite(value)
        .filter(|v| *v >= 0.0)
        .map(|v| v.floor() as u64)
}

fn string_list(value: Option<&Value>) -> Option<Vec<String>> {
    Some(
        defined(value)?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
    )
}

fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().cloned().map(Value::String).collect())
}

/// Default numbers from `DEFAULT_SETTINGS`, for the getters.
mod defaults {
    pub const COMPACTION_RESERVE_TOKENS: u64 = 16384;
    pub const COMPACTION_KEEP_RECENT_TOKENS: u64 = 20000;
    pub const COMPACTION_MAX_CONTEXT_RATIO: f64 = 0.75;
    pub const TOOL_OUTPUT_MAX_BYTES: u64 = 32 * 1024;
    pub const TOOL_OUTPUT_MAX_LINES: u64 = 800;
    pub const VOICE_SILENCE_MS: u64 = 800;
    pub const WEBTOOLS_TIMEOUT_SECS: u64 = 15;
    pub const BRANCH_SUMMARY_RESERVE_TOKENS: u64 = 16384;
    pub const RETRY_MAX_RETRIES: u64 = 3;
    pub const RETRY_BASE_DELAY_MS: u64 = 2000;
    pub const PROVIDER_MAX_RETRY_DELAY_MS: u64 = 60000;
    pub const MAX_SUBAGENT_DEPTH: u64 = 2;
    pub const NESTED_SUBAGENT_CONCURRENCY: u64 = 2;
    pub const IMAGE_WIDTH_CELLS: u64 = 60;
    pub const EDITOR_PADDING_X: u64 = 1;
    pub const AUTOCOMPLETE_MAX_VISIBLE: u64 = 5;
    pub const CODE_BLOCK_INDENT: &str = "  ";

    pub fn learn(key: super::LearnSettingKey) -> u64 {
        use super::LearnSettingKey::*;
        match key {
            MaxSessions => 20,
            MaxAgeDays => 30,
            MinRepeats => 2,
            MinRequestRepeats => 3,
            MaxProposals => 8,
        }
    }
}

/// `SettingsManager`: global + project settings, merged (project wins).
///
/// Setters change the global scope (the `set_project_*` ones the project
/// scope) and persist at once, rewriting only the fields changed this session
/// so edits made to the file meanwhile survive. hoocode queues these writes
/// on a promise chain; here they run synchronously, so [`flush`] has nothing
/// to wait for. A scope whose file failed to parse at load is never written.
///
/// [`flush`]: SettingsManager::flush
pub struct SettingsManager {
    storage: Arc<dyn SettingsStorage>,
    global: Settings,
    project: Settings,
    settings: Settings,
    modified: Modified,
    modified_project: Modified,
    global_load_failed: bool,
    project_load_failed: bool,
    errors: Vec<SettingsError>,
}

impl std::fmt::Debug for SettingsManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsManager")
            .field("global", &self.global)
            .field("project", &self.project)
            .finish_non_exhaustive()
    }
}

impl SettingsManager {
    /// `create`: file-backed settings for `cwd` with the given agent dir.
    pub fn create(cwd: impl AsRef<Path>, agent_dir: impl AsRef<Path>) -> Self {
        Self::from_storage(Arc::new(FileSettingsStorage::new(cwd, agent_dir)))
    }

    /// `create` with the default agent dir (`getAgentDir()`).
    pub fn create_default(cwd: impl AsRef<Path>) -> Self {
        Self::create(cwd, cortexcode_code_paths::agent_dir())
    }

    /// `fromStorage`.
    pub fn from_storage(storage: Arc<dyn SettingsStorage>) -> Self {
        let mut errors = Vec::new();
        let mut load = |scope| match load_from_storage(storage.as_ref(), scope) {
            Ok(settings) => (settings, false),
            Err(error) => {
                errors.push(SettingsError { scope, error });
                (Settings::new(), true)
            }
        };
        let (global, global_load_failed) = load(SettingsScope::Global);
        let (project, project_load_failed) = load(SettingsScope::Project);
        let settings = deep_merge_settings(&global, &project);
        Self {
            storage,
            global,
            project,
            settings,
            modified: Modified::default(),
            modified_project: Modified::default(),
            global_load_failed,
            project_load_failed,
            errors,
        }
    }

    /// `inMemory`: no file I/O; `settings` become the global scope.
    pub fn in_memory(settings: Settings) -> Self {
        let storage = InMemorySettingsStorage::new();
        let initial = migrate_settings(settings);
        let content =
            serde_json::to_string_pretty(&Value::Object(initial)).expect("settings serialize");
        let _ = storage.with_lock(SettingsScope::Global, &mut |_| Ok(Some(content.clone())));
        Self::from_storage(Arc::new(storage))
    }

    pub fn global_settings(&self) -> Settings {
        self.global.clone()
    }

    pub fn project_settings(&self) -> Settings {
        self.project.clone()
    }

    /// The merged view (global, project, then any overrides).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn default_settings(&self) -> Settings {
        default_settings()
    }

    /// `reload`: re-read both scopes. A scope that fails to parse keeps its
    /// previous values and records the error.
    pub fn reload(&mut self) {
        match load_from_storage(self.storage.as_ref(), SettingsScope::Global) {
            Ok(settings) => {
                self.global = settings;
                self.global_load_failed = false;
            }
            Err(error) => {
                self.global_load_failed = true;
                self.record_error(SettingsScope::Global, error);
            }
        }
        self.modified.clear();
        self.modified_project.clear();
        match load_from_storage(self.storage.as_ref(), SettingsScope::Project) {
            Ok(settings) => {
                self.project = settings;
                self.project_load_failed = false;
            }
            Err(error) => {
                self.project_load_failed = true;
                self.record_error(SettingsScope::Project, error);
            }
        }
        self.settings = deep_merge_settings(&self.global, &self.project);
    }

    /// `applyOverrides`: layer `overrides` on the merged view (until the
    /// next save rebuilds it).
    pub fn apply_overrides(&mut self, overrides: &Settings) {
        self.settings = deep_merge_settings(&self.settings, overrides);
    }

    /// `flush`: writes are synchronous, so there is nothing pending.
    pub fn flush(&self) {}

    /// `drainErrors`.
    pub fn drain_errors(&mut self) -> Vec<SettingsError> {
        std::mem::take(&mut self.errors)
    }

    fn record_error(&mut self, scope: SettingsScope, error: Error) {
        self.errors.push(SettingsError { scope, error });
    }

    fn persist_scoped(
        storage: &dyn SettingsStorage,
        scope: SettingsScope,
        snapshot: &Settings,
        modified: &Modified,
    ) -> Result<(), Error> {
        storage.with_lock(scope, &mut |current| {
            let file = match current {
                Some(content) if !content.is_empty() => parse_settings(content)?,
                _ => Settings::new(),
            };
            let mut merged = file.clone();
            for field in &modified.fields {
                let value = snapshot.get(field);
                match (modified.nested_keys(field), value) {
                    (Some(keys), Some(Value::Object(in_memory))) => {
                        let mut nested = file
                            .get(field)
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        for key in keys {
                            set_or_remove(&mut nested, key, in_memory.get(key).cloned());
                        }
                        merged.insert(field.clone(), Value::Object(nested));
                    }
                    _ => set_or_remove(&mut merged, field, value.cloned()),
                }
            }
            Ok(Some(serde_json::to_string_pretty(&Value::Object(merged))?))
        })
    }

    fn save(&mut self) {
        self.settings = deep_merge_settings(&self.global, &self.project);
        if self.global_load_failed {
            return;
        }
        match Self::persist_scoped(
            self.storage.as_ref(),
            SettingsScope::Global,
            &self.global,
            &self.modified,
        ) {
            Ok(()) => self.modified.clear(),
            Err(error) => self.record_error(SettingsScope::Global, error),
        }
    }

    fn save_project_settings(&mut self, settings: Settings) {
        self.project = settings;
        self.settings = deep_merge_settings(&self.global, &self.project);
        if self.project_load_failed {
            return;
        }
        match Self::persist_scoped(
            self.storage.as_ref(),
            SettingsScope::Project,
            &self.project,
            &self.modified_project,
        ) {
            Ok(()) => self.modified_project.clear(),
            Err(error) => self.record_error(SettingsScope::Project, error),
        }
    }

    // --- helpers -----------------------------------------------------------

    fn get(&self, key: &str) -> Option<&Value> {
        defined(self.settings.get(key))
    }

    fn get_in(&self, key: &str, nested: &str) -> Option<&Value> {
        defined(self.settings.get(key)?.as_object()?.get(nested))
    }

    fn bool_or(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    fn nested_bool_or(&self, key: &str, nested: &str, default: bool) -> bool {
        self.get_in(key, nested)
            .and_then(Value::as_bool)
            .unwrap_or(default)
    }

    fn string(&self, key: &str) -> Option<String> {
        self.get(key).and_then(Value::as_str).map(str::to_owned)
    }

    fn set(&mut self, key: &str, value: Option<Value>) {
        set_or_remove(&mut self.global, key, value);
        self.modified.mark(key, None);
        self.save();
    }

    fn set_nested(&mut self, key: &str, nested: &str, value: Value) {
        let entry = self
            .global
            .entry(key.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        if let Value::Object(map) = entry {
            map.insert(nested.to_owned(), value);
        }
        self.modified.mark(key, Some(nested));
        self.save();
    }

    fn set_project(&mut self, key: &str, value: Value) {
        let mut project = self.project.clone();
        project.insert(key.to_owned(), value);
        self.modified_project.mark(key, None);
        self.save_project_settings(project);
    }

    fn path_list(&self, key: &str) -> Vec<String> {
        string_list(self.get(key)).unwrap_or_default()
    }

    // --- accessors ---------------------------------------------------------

    pub fn last_changelog_version(&self) -> Option<String> {
        self.string("lastChangelogVersion")
    }

    pub fn set_last_changelog_version(&mut self, version: &str) {
        self.set("lastChangelogVersion", Some(version.into()));
    }

    /// `getSessionDir`, with `~` expanded.
    pub fn session_dir(&self) -> Option<PathBuf> {
        let dir = self.string("sessionDir").filter(|d| !d.is_empty())?;
        Some(cortexcode_code_paths::expand_tilde_path(&dir))
    }

    pub fn default_provider(&self) -> Option<String> {
        self.string("defaultProvider")
    }

    pub fn default_model(&self) -> Option<String> {
        self.string("defaultModel")
    }

    pub fn set_default_provider(&mut self, provider: &str) {
        self.set("defaultProvider", Some(provider.into()));
    }

    pub fn set_default_model(&mut self, model_id: &str) {
        self.set("defaultModel", Some(model_id.into()));
    }

    pub fn set_default_model_and_provider(&mut self, provider: &str, model_id: &str) {
        self.global
            .insert("defaultProvider".into(), provider.into());
        self.global.insert("defaultModel".into(), model_id.into());
        self.modified.mark("defaultProvider", None);
        self.modified.mark("defaultModel", None);
        self.save();
    }

    pub fn steering_mode(&self) -> QueueMode {
        self.get("steeringMode")
            .and_then(Value::as_str)
            .and_then(QueueMode::parse)
            .unwrap_or(QueueMode::OneAtATime)
    }

    pub fn set_steering_mode(&mut self, mode: QueueMode) {
        self.set("steeringMode", Some(mode.as_str().into()));
    }

    pub fn follow_up_mode(&self) -> QueueMode {
        self.get("followUpMode")
            .and_then(Value::as_str)
            .and_then(QueueMode::parse)
            .unwrap_or(QueueMode::OneAtATime)
    }

    pub fn set_follow_up_mode(&mut self, mode: QueueMode) {
        self.set("followUpMode", Some(mode.as_str().into()));
    }

    pub fn theme(&self) -> Option<String> {
        self.string("theme")
    }

    pub fn set_theme(&mut self, theme: &str) {
        self.set("theme", Some(theme.into()));
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevelSetting> {
        self.get("defaultThinkingLevel")
            .and_then(Value::as_str)
            .and_then(ThinkingLevelSetting::parse)
    }

    pub fn set_default_thinking_level(&mut self, level: ThinkingLevelSetting) {
        self.set("defaultThinkingLevel", Some(level.as_str().into()));
    }

    pub fn transport(&self) -> Transport {
        self.get("transport")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn set_transport(&mut self, transport: Transport) {
        self.set(
            "transport",
            Some(serde_json::to_value(transport).expect("transport serializes")),
        );
    }

    pub fn compaction_enabled(&self) -> bool {
        self.nested_bool_or("compaction", "enabled", true)
    }

    pub fn set_compaction_enabled(&mut self, enabled: bool) {
        self.set_nested("compaction", "enabled", enabled.into());
    }

    pub fn compaction_reserve_tokens(&self) -> u64 {
        whole(self.get_in("compaction", "reserveTokens"))
            .unwrap_or(defaults::COMPACTION_RESERVE_TOKENS)
    }

    pub fn compaction_keep_recent_tokens(&self) -> u64 {
        whole(self.get_in("compaction", "keepRecentTokens"))
            .unwrap_or(defaults::COMPACTION_KEEP_RECENT_TOKENS)
    }

    pub fn compaction_settings(&self) -> CompactionSettings {
        CompactionSettings {
            enabled: self.compaction_enabled(),
            reserve_tokens: self.compaction_reserve_tokens(),
            keep_recent_tokens: self.compaction_keep_recent_tokens(),
            max_context_ratio: finite(self.get_in("compaction", "maxContextRatio"))
                .unwrap_or(defaults::COMPACTION_MAX_CONTEXT_RATIO),
        }
    }

    /// `getLearnSettings`: each value falls back to its default unless it is
    /// a finite positive number.
    pub fn learn_settings(&self) -> LearnSettings {
        let positive = |key: LearnSettingKey| {
            finite(self.get(key.as_str()))
                .filter(|v| *v > 0.0)
                .map(|v| v.floor() as u64)
                .unwrap_or_else(|| defaults::learn(key))
        };
        LearnSettings {
            max_sessions: positive(LearnSettingKey::MaxSessions),
            max_age_days: positive(LearnSettingKey::MaxAgeDays),
            min_repeats: positive(LearnSettingKey::MinRepeats),
            min_request_repeats: positive(LearnSettingKey::MinRequestRepeats),
            max_proposals: positive(LearnSettingKey::MaxProposals),
        }
    }

    pub fn model_categories(&self) -> Option<ModelCategories> {
        let categories = self.get("modelCategories")?.as_object()?;
        let tier = |name: &str| {
            defined(categories.get(name))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        Some(ModelCategories {
            fast: tier("fast"),
            standard: tier("standard"),
            capable: tier("capable"),
        })
    }

    /// `setLearnSetting`: floored to at least 1.
    pub fn set_learn_setting(&mut self, key: LearnSettingKey, value: i64) {
        self.set(key.as_str(), Some(value.max(1).into()));
    }

    /// Byte cap on one read/bash result; at least 1024.
    pub fn tool_output_max_bytes(&self) -> u64 {
        finite(self.get_in("toolOutput", "maxBytes"))
            .filter(|v| *v >= 1024.0)
            .map(|v| v.floor() as u64)
            .unwrap_or(defaults::TOOL_OUTPUT_MAX_BYTES)
    }

    pub fn set_tool_output_max_bytes(&mut self, bytes: i64) {
        self.set_nested("toolOutput", "maxBytes", bytes.max(1024).into());
    }

    /// Line cap on one read/bash result; at least 1.
    pub fn tool_output_max_lines(&self) -> u64 {
        finite(self.get_in("toolOutput", "maxLines"))
            .filter(|v| *v >= 1.0)
            .map(|v| v.floor() as u64)
            .unwrap_or(defaults::TOOL_OUTPUT_MAX_LINES)
    }

    pub fn set_tool_output_max_lines(&mut self, lines: i64) {
        self.set_nested("toolOutput", "maxLines", lines.max(1).into());
    }

    pub fn context_gc_enabled(&self) -> bool {
        self.nested_bool_or("contextGc", "enabled", true)
    }

    pub fn set_context_gc_enabled(&mut self, enabled: bool) {
        self.set_nested("contextGc", "enabled", enabled.into());
    }

    /// Trailing-silence window for voice capture, clamped to 300..=10000 ms.
    pub fn voice_silence_ms(&self) -> u64 {
        finite(self.get_in("voice", "silenceMs"))
            .map(|v| v.floor().clamp(300.0, 10000.0) as u64)
            .unwrap_or(defaults::VOICE_SILENCE_MS)
    }

    pub fn set_voice_silence_ms(&mut self, ms: i64) {
        self.set_nested("voice", "silenceMs", ms.clamp(300, 10000).into());
    }

    /// Webfetch/websearch timeout: the setting, else `*_WEBTOOLS_TIMEOUT`,
    /// else 15; clamped to 1..=120.
    pub fn webtools_timeout_secs(&self) -> u64 {
        let clamp = |n: f64| n.floor().clamp(1.0, 120.0) as u64;
        if let Some(configured) = finite(self.get_in("webtools", "timeoutSecs")) {
            return clamp(configured);
        }
        if let Some(env) = cortexcode_code_paths::env_override("WEBTOOLS_TIMEOUT") {
            if let Ok(value) = env.trim().parse::<f64>() {
                if value.is_finite() && value > 0.0 {
                    return clamp(value);
                }
            }
        }
        defaults::WEBTOOLS_TIMEOUT_SECS
    }

    /// `getWebtoolsSearch`: the user-level `webtools.search` block only,
    /// since that is the file the webtools binary reads.
    pub fn webtools_search(&self) -> Option<Value> {
        defined(self.global.get("webtools")?.as_object()?.get("search")).cloned()
    }

    pub fn set_webtools_timeout_secs(&mut self, secs: i64) {
        self.set_nested("webtools", "timeoutSecs", secs.clamp(1, 120).into());
    }

    pub fn disabled_tools(&self) -> Vec<String> {
        self.path_list("disabledTools")
    }

    pub fn set_disabled_tools(&mut self, names: &[String]) {
        self.set("disabledTools", Some(strings(names)));
    }

    /// `getToolOutputView`, reading the retired `toolOutputDisplay` values.
    pub fn tool_output_view(&self) -> ToolOutputView {
        if let Some(view) = self
            .get("toolOutputView")
            .and_then(Value::as_str)
            .and_then(ToolOutputView::parse)
        {
            return view;
        }
        self.get("toolOutputDisplay")
            .and_then(Value::as_str)
            .and_then(ToolOutputView::from_legacy)
            .unwrap_or(ToolOutputView::Peek)
    }

    pub fn set_tool_output_view(&mut self, view: ToolOutputView) {
        self.set("toolOutputView", Some(view.as_str().into()));
    }

    /// The chrome dial's stop, or `None` when never set (the caller picks a
    /// default from the terminal size).
    pub fn chrome_density(&self) -> Option<ChromeDensity> {
        self.get("chromeDensity")
            .and_then(Value::as_str)
            .and_then(ChromeDensity::parse)
    }

    pub fn set_chrome_density(&mut self, density: ChromeDensity) {
        self.set("chromeDensity", Some(density.as_str().into()));
    }

    /// Persisted extension flag overrides (name -> value).
    pub fn flag_overrides(&self) -> Vec<(String, FlagValue)> {
        let Some(flags) = self.get("flags").and_then(Value::as_object) else {
            return Vec::new();
        };
        flags
            .iter()
            .filter_map(|(name, value)| {
                let value = match value {
                    Value::Bool(b) => FlagValue::Bool(*b),
                    Value::String(s) => FlagValue::String(s.clone()),
                    _ => return None,
                };
                Some((name.clone(), value))
            })
            .collect()
    }

    pub fn set_flag_override(&mut self, name: &str, value: FlagValue) {
        self.set_nested("flags", name, value.to_json());
    }

    pub fn clear_flag_override(&mut self, name: &str) {
        let removed = match self.global.get_mut("flags") {
            Some(Value::Object(flags)) => flags.shift_remove(name).is_some(),
            _ => false,
        };
        if removed {
            self.modified.mark("flags", Some(name));
            self.save();
        }
    }

    pub fn branch_summary_settings(&self) -> BranchSummarySettings {
        BranchSummarySettings {
            reserve_tokens: whole(self.get_in("branchSummary", "reserveTokens"))
                .unwrap_or(defaults::BRANCH_SUMMARY_RESERVE_TOKENS),
            skip_prompt: self.branch_summary_skip_prompt(),
        }
    }

    pub fn branch_summary_skip_prompt(&self) -> bool {
        self.nested_bool_or("branchSummary", "skipPrompt", false)
    }

    pub fn retry_enabled(&self) -> bool {
        self.nested_bool_or("retry", "enabled", true)
    }

    pub fn set_retry_enabled(&mut self, enabled: bool) {
        self.set_nested("retry", "enabled", enabled.into());
    }

    pub fn retry_settings(&self) -> RetrySettings {
        RetrySettings {
            enabled: self.retry_enabled(),
            max_retries: whole(self.get_in("retry", "maxRetries"))
                .unwrap_or(defaults::RETRY_MAX_RETRIES),
            base_delay_ms: whole(self.get_in("retry", "baseDelayMs"))
                .unwrap_or(defaults::RETRY_BASE_DELAY_MS),
        }
    }

    pub fn provider_retry_settings(&self) -> ProviderRetrySettings {
        let provider = self.get_in("retry", "provider").and_then(Value::as_object);
        let field = |name: &str| whole(provider.and_then(|p| p.get(name)));
        ProviderRetrySettings {
            timeout_ms: field("timeoutMs"),
            max_retries: field("maxRetries"),
            max_retry_delay_ms: field("maxRetryDelayMs")
                .unwrap_or(defaults::PROVIDER_MAX_RETRY_DELAY_MS),
        }
    }

    pub fn hide_thinking_block(&self) -> bool {
        self.bool_or("hideThinkingBlock", false)
    }

    pub fn set_hide_thinking_block(&mut self, hide: bool) {
        self.set("hideThinkingBlock", Some(hide.into()));
    }

    pub fn shell_path(&self) -> Option<String> {
        self.string("shellPath")
    }

    pub fn set_shell_path(&mut self, path: Option<&str>) {
        self.set("shellPath", path.map(Value::from));
    }

    pub fn quiet_startup(&self) -> bool {
        self.bool_or("quietStartup", false)
    }

    pub fn set_quiet_startup(&mut self, quiet: bool) {
        self.set("quietStartup", Some(quiet.into()));
    }

    pub fn shell_command_prefix(&self) -> Option<String> {
        self.string("shellCommandPrefix")
    }

    pub fn set_shell_command_prefix(&mut self, prefix: Option<&str>) {
        self.set("shellCommandPrefix", prefix.map(Value::from));
    }

    pub fn npm_command(&self) -> Option<Vec<String>> {
        string_list(self.get("npmCommand"))
    }

    pub fn set_npm_command(&mut self, command: Option<&[String]>) {
        self.set("npmCommand", command.map(strings));
    }

    pub fn collapse_changelog(&self) -> bool {
        self.bool_or("collapseChangelog", false)
    }

    pub fn set_collapse_changelog(&mut self, collapse: bool) {
        self.set("collapseChangelog", Some(collapse.into()));
    }

    pub fn enable_install_telemetry(&self) -> bool {
        self.bool_or("enableInstallTelemetry", true)
    }

    pub fn set_enable_install_telemetry(&mut self, enabled: bool) {
        self.set("enableInstallTelemetry", Some(enabled.into()));
    }

    pub fn packages(&self) -> Vec<PackageSource> {
        self.get("packages")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn packages_json(packages: &[PackageSource]) -> Value {
        serde_json::to_value(packages).expect("packages serialize")
    }

    pub fn set_packages(&mut self, packages: &[PackageSource]) {
        self.set("packages", Some(Self::packages_json(packages)));
    }

    pub fn set_project_packages(&mut self, packages: &[PackageSource]) {
        self.set_project("packages", Self::packages_json(packages));
    }

    pub fn extension_paths(&self) -> Vec<String> {
        self.path_list("extensions")
    }

    pub fn set_extension_paths(&mut self, paths: &[String]) {
        self.set("extensions", Some(strings(paths)));
    }

    pub fn set_project_extension_paths(&mut self, paths: &[String]) {
        self.set_project("extensions", strings(paths));
    }

    pub fn skill_paths(&self) -> Vec<String> {
        self.path_list("skills")
    }

    pub fn set_skill_paths(&mut self, paths: &[String]) {
        self.set("skills", Some(strings(paths)));
    }

    pub fn set_project_skill_paths(&mut self, paths: &[String]) {
        self.set_project("skills", strings(paths));
    }

    pub fn prompt_template_paths(&self) -> Vec<String> {
        self.path_list("prompts")
    }

    pub fn set_prompt_template_paths(&mut self, paths: &[String]) {
        self.set("prompts", Some(strings(paths)));
    }

    pub fn set_project_prompt_template_paths(&mut self, paths: &[String]) {
        self.set_project("prompts", strings(paths));
    }

    pub fn slash_command_paths(&self) -> Vec<String> {
        self.path_list("slashCommands")
    }

    pub fn set_slash_command_paths(&mut self, paths: &[String]) {
        self.set("slashCommands", Some(strings(paths)));
    }

    pub fn set_project_slash_command_paths(&mut self, paths: &[String]) {
        self.set_project("slashCommands", strings(paths));
    }

    pub fn theme_paths(&self) -> Vec<String> {
        self.path_list("themes")
    }

    pub fn set_theme_paths(&mut self, paths: &[String]) {
        self.set("themes", Some(strings(paths)));
    }

    pub fn set_project_theme_paths(&mut self, paths: &[String]) {
        self.set_project("themes", strings(paths));
    }

    pub fn enable_skill_commands(&self) -> bool {
        self.bool_or("enableSkillCommands", true)
    }

    pub fn set_enable_skill_commands(&mut self, enabled: bool) {
        self.set("enableSkillCommands", Some(enabled.into()));
    }

    pub fn enable_subagent(&self) -> bool {
        self.bool_or("enableSubagent", true)
    }

    pub fn set_enable_subagent(&mut self, enabled: bool) {
        self.set("enableSubagent", Some(enabled.into()));
    }

    pub fn warm_subagents(&self) -> bool {
        self.bool_or("warmSubagents", false)
    }

    pub fn set_warm_subagents(&mut self, enabled: bool) {
        self.set("warmSubagents", Some(enabled.into()));
    }

    /// Tree-wide subagent nesting cap; at least 1.
    pub fn max_subagent_depth(&self) -> u64 {
        match self.get("maxSubagentDepth") {
            None => defaults::MAX_SUBAGENT_DEPTH,
            Some(v) => v
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 1.0)
                .map(|v| v.floor() as u64)
                .unwrap_or(1),
        }
    }

    /// Concurrent subagents per pool at nesting depth >= 1; at least 1.
    pub fn nested_subagent_concurrency(&self) -> u64 {
        finite(self.get("nestedSubagentConcurrency"))
            .filter(|v| *v >= 1.0)
            .map(|v| v.floor() as u64)
            .unwrap_or(defaults::NESTED_SUBAGENT_CONCURRENCY)
    }

    pub fn enable_todo_write(&self) -> bool {
        self.bool_or("enableTodoWrite", true)
    }

    pub fn set_enable_todo_write(&mut self, enabled: bool) {
        self.set("enableTodoWrite", Some(enabled.into()));
    }

    /// Raw `platform` tokens (a string becomes a one-item list).
    pub fn platform(&self) -> Option<Vec<String>> {
        match self.get("platform")? {
            Value::String(s) => Some(vec![s.clone()]),
            other => string_list(Some(other)),
        }
    }

    /// Stored as a list; an empty or missing list removes the key.
    pub fn set_platform(&mut self, platforms: Option<&[String]>) {
        let value = platforms.filter(|p| !p.is_empty()).map(strings);
        self.set("platform", value);
    }

    pub fn enable_plugin_tools(&self) -> bool {
        self.bool_or("enablePluginTools", false)
    }

    pub fn set_enable_plugin_tools(&mut self, enabled: bool) {
        self.set("enablePluginTools", Some(enabled.into()));
    }

    /// Where an autonomous InstallPlugin puts a plugin; anything but
    /// `"project"` is the user scope.
    pub fn plugin_install_scope(&self) -> PluginInstallScope {
        match self.get("pluginInstallScope").and_then(Value::as_str) {
            Some("project") => PluginInstallScope::Project,
            _ => PluginInstallScope::User,
        }
    }

    pub fn set_plugin_install_scope(&mut self, scope: PluginInstallScope) {
        self.set("pluginInstallScope", Some(scope.as_str().into()));
    }

    pub fn defer_mcp_schemas(&self) -> bool {
        self.bool_or("deferMcpSchemas", true)
    }

    pub fn set_defer_mcp_schemas(&mut self, enabled: bool) {
        self.set("deferMcpSchemas", Some(enabled.into()));
    }

    pub fn enable_web_tools(&self) -> bool {
        self.bool_or("enableWebTools", false)
    }

    pub fn set_enable_web_tools(&mut self, enabled: bool) {
        self.set("enableWebTools", Some(enabled.into()));
    }

    pub fn enable_semantic_index(&self) -> bool {
        self.bool_or("enableSemanticIndex", true)
    }

    pub fn set_enable_semantic_index(&mut self, enabled: bool) {
        self.set("enableSemanticIndex", Some(enabled.into()));
    }

    pub fn embsearch_binary_path(&self) -> Option<String> {
        self.string("embsearchBinaryPath")
    }

    pub fn embsearch_threshold_bytes(&self) -> u64 {
        whole(self.get("embsearchThresholdBytes")).unwrap_or(0)
    }

    pub fn light(&self) -> bool {
        self.bool_or("light", false)
    }

    pub fn set_light(&mut self, enabled: bool) {
        self.set("light", Some(enabled.into()));
    }

    pub fn thinking_budgets(&self) -> Option<ThinkingBudgets> {
        let budgets = self.get("thinkingBudgets")?.as_object()?;
        let level = |name: &str| whole(budgets.get(name));
        Some(ThinkingBudgets {
            minimal: level("minimal"),
            low: level("low"),
            medium: level("medium"),
            high: level("high"),
            xhigh: level("xhigh"),
        })
    }

    pub fn thinking_display(&self) -> Option<ThinkingDisplay> {
        self.get("thinkingDisplay")
            .and_then(Value::as_str)
            .and_then(ThinkingDisplay::parse)
    }

    pub fn show_images(&self) -> bool {
        self.nested_bool_or("terminal", "showImages", true)
    }

    pub fn set_show_images(&mut self, show: bool) {
        self.set_nested("terminal", "showImages", show.into());
    }

    pub fn image_width_cells(&self) -> u64 {
        finite(self.get_in("terminal", "imageWidthCells"))
            .map(|v| v.floor().max(1.0) as u64)
            .unwrap_or(defaults::IMAGE_WIDTH_CELLS)
    }

    pub fn set_image_width_cells(&mut self, width: i64) {
        self.set_nested("terminal", "imageWidthCells", width.max(1).into());
    }

    /// The setting, else `*_CLEAR_ON_SHRINK=1`.
    pub fn clear_on_shrink(&self) -> bool {
        let terminal = self.settings.get("terminal").and_then(Value::as_object);
        match terminal.and_then(|t| t.get("clearOnShrink")) {
            Some(value) => value.as_bool().unwrap_or(false),
            None => cortexcode_code_paths::env_override("CLEAR_ON_SHRINK").as_deref() == Some("1"),
        }
    }

    pub fn set_clear_on_shrink(&mut self, enabled: bool) {
        self.set_nested("terminal", "clearOnShrink", enabled.into());
    }

    pub fn show_terminal_progress(&self) -> bool {
        self.nested_bool_or("terminal", "showTerminalProgress", false)
    }

    pub fn set_show_terminal_progress(&mut self, enabled: bool) {
        self.set_nested("terminal", "showTerminalProgress", enabled.into());
    }

    pub fn chime_on_turn_complete(&self) -> bool {
        self.nested_bool_or("terminal", "chimeOnTurnComplete", false)
    }

    pub fn set_chime_on_turn_complete(&mut self, enabled: bool) {
        self.set_nested("terminal", "chimeOnTurnComplete", enabled.into());
    }

    pub fn tips_enabled(&self) -> bool {
        self.nested_bool_or("tips", "enabled", true)
    }

    pub fn set_tips_enabled(&mut self, enabled: bool) {
        self.set_nested("tips", "enabled", enabled.into());
    }

    pub fn seen_tips(&self) -> Vec<String> {
        string_list(self.get_in("tips", "seen")).unwrap_or_default()
    }

    /// Remember that a tip was shown (idempotent).
    pub fn mark_tip_seen(&mut self, id: &str) {
        let tips = self.global.get("tips").and_then(Value::as_object);
        let mut seen = string_list(tips.and_then(|t| t.get("seen"))).unwrap_or_default();
        if seen.iter().any(|s| s == id) {
            return;
        }
        seen.push(id.to_owned());
        self.set_nested("tips", "seen", strings(&seen));
    }

    pub fn star_nudge_count(&self) -> u64 {
        whole(self.get_in("tips", "starNudges")).unwrap_or(0)
    }

    pub fn record_star_nudge(&mut self) {
        let tips = self.global.get("tips").and_then(Value::as_object);
        let count = whole(tips.and_then(|t| t.get("starNudges"))).unwrap_or(0);
        self.set_nested("tips", "starNudges", (count + 1).into());
    }

    pub fn image_auto_resize(&self) -> bool {
        self.nested_bool_or("images", "autoResize", true)
    }

    pub fn set_image_auto_resize(&mut self, enabled: bool) {
        self.set_nested("images", "autoResize", enabled.into());
    }

    pub fn block_images(&self) -> bool {
        self.nested_bool_or("images", "blockImages", false)
    }

    pub fn set_block_images(&mut self, blocked: bool) {
        self.set_nested("images", "blockImages", blocked.into());
    }

    pub fn enabled_models(&self) -> Option<Vec<String>> {
        string_list(self.get("enabledModels"))
    }

    pub fn set_enabled_models(&mut self, patterns: Option<&[String]>) {
        self.set("enabledModels", patterns.map(strings));
    }

    pub fn double_escape_action(&self) -> DoubleEscapeAction {
        self.get("doubleEscapeAction")
            .and_then(Value::as_str)
            .and_then(DoubleEscapeAction::parse)
            .unwrap_or(DoubleEscapeAction::Tree)
    }

    pub fn set_double_escape_action(&mut self, action: DoubleEscapeAction) {
        self.set("doubleEscapeAction", Some(action.as_str().into()));
    }

    pub fn tree_filter_mode(&self) -> TreeFilterMode {
        self.get("treeFilterMode")
            .and_then(Value::as_str)
            .and_then(TreeFilterMode::parse)
            .unwrap_or(TreeFilterMode::Default)
    }

    pub fn set_tree_filter_mode(&mut self, mode: TreeFilterMode) {
        self.set("treeFilterMode", Some(mode.as_str().into()));
    }

    /// The setting, else `*_HARDWARE_CURSOR=1`.
    pub fn show_hardware_cursor(&self) -> bool {
        match self.get("showHardwareCursor") {
            Some(value) => value.as_bool().unwrap_or(false),
            None => cortexcode_code_paths::env_override("HARDWARE_CURSOR").as_deref() == Some("1"),
        }
    }

    pub fn set_show_hardware_cursor(&mut self, enabled: bool) {
        self.set("showHardwareCursor", Some(enabled.into()));
    }

    pub fn editor_border(&self) -> EditorBorder {
        self.get("editorBorder")
            .and_then(Value::as_str)
            .and_then(EditorBorder::parse)
            .unwrap_or(EditorBorder::Box)
    }

    pub fn set_editor_border(&mut self, border: EditorBorder) {
        self.set("editorBorder", Some(border.as_str().into()));
    }

    pub fn editor_padding_x(&self) -> u64 {
        whole(self.get("editorPaddingX")).unwrap_or(defaults::EDITOR_PADDING_X)
    }

    /// Clamped to 0..=3.
    pub fn set_editor_padding_x(&mut self, padding: i64) {
        self.set("editorPaddingX", Some(padding.clamp(0, 3).into()));
    }

    pub fn autocomplete_max_visible(&self) -> u64 {
        whole(self.get("autocompleteMaxVisible")).unwrap_or(defaults::AUTOCOMPLETE_MAX_VISIBLE)
    }

    /// Clamped to 3..=20.
    pub fn set_autocomplete_max_visible(&mut self, max_visible: i64) {
        self.set(
            "autocompleteMaxVisible",
            Some(max_visible.clamp(3, 20).into()),
        );
    }

    pub fn code_block_indent(&self) -> String {
        self.get_in("markdown", "codeBlockIndent")
            .and_then(Value::as_str)
            .unwrap_or(defaults::CODE_BLOCK_INDENT)
            .to_owned()
    }

    /// `getWarnings`, defaults filled in.
    pub fn warnings(&self) -> WarningSettings {
        let flag = |name: &str| {
            Some(
                self.get_in("warnings", name)
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            )
        };
        WarningSettings {
            anthropic_extra_usage: flag("anthropicExtraUsage"),
            websearch_api_key: flag("websearchApiKey"),
        }
    }

    /// Replaces the whole `warnings` object.
    pub fn set_warnings(&mut self, warnings: WarningSettings) {
        let mut map = Map::new();
        if let Some(v) = warnings.anthropic_extra_usage {
            map.insert("anthropicExtraUsage".into(), v.into());
        }
        if let Some(v) = warnings.websearch_api_key {
            map.insert("websearchApiKey".into(), v.into());
        }
        self.set("warnings", Some(Value::Object(map)));
    }
}
