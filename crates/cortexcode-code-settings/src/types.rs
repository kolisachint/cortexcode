//! `settings-types.ts` and `settings-defaults.ts`.
//!
//! The on-disk `Settings` stays a raw JSON object (see the crate docs); these
//! are the typed values the manager hands out.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The raw settings object (`Settings`), unknown keys included.
pub type Settings = Map<String, Value>;

macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

str_enum!(
    /// `steeringMode` / `followUpMode`.
    QueueMode { All => "all", OneAtATime => "one-at-a-time" }
);

str_enum!(
    /// `defaultThinkingLevel`.
    ThinkingLevelSetting {
        Off => "off",
        Minimal => "minimal",
        Low => "low",
        Medium => "medium",
        High => "high",
        XHigh => "xhigh",
    }
);

impl From<ThinkingLevelSetting> for cortexcode_ai_types::ThinkingLevel {
    fn from(level: ThinkingLevelSetting) -> Self {
        use cortexcode_ai_types::ThinkingLevel as T;
        match level {
            ThinkingLevelSetting::Off => T::Off,
            ThinkingLevelSetting::Minimal => T::Minimal,
            ThinkingLevelSetting::Low => T::Low,
            ThinkingLevelSetting::Medium => T::Medium,
            ThinkingLevelSetting::High => T::High,
            ThinkingLevelSetting::XHigh => T::XHigh,
        }
    }
}

impl From<cortexcode_ai_types::ThinkingLevel> for ThinkingLevelSetting {
    fn from(level: cortexcode_ai_types::ThinkingLevel) -> Self {
        use cortexcode_ai_types::ThinkingLevel as T;
        match level {
            T::Off => Self::Off,
            T::Minimal => Self::Minimal,
            T::Low => Self::Low,
            T::Medium => Self::Medium,
            T::High => Self::High,
            T::XHigh => Self::XHigh,
        }
    }
}

str_enum!(
    /// `toolOutputView` (tool-output-view.ts).
    ToolOutputView { Radar => "radar", Peek => "peek", Full => "full" }
);

impl ToolOutputView {
    /// `LEGACY_TOOL_OUTPUT_VIEWS`: the retired `toolOutputDisplay` values.
    pub fn from_legacy(value: &str) -> Option<Self> {
        match value {
            "collapsed" => Some(Self::Radar),
            "glance" => Some(Self::Peek),
            "standard" => Some(Self::Full),
            _ => None,
        }
    }
}

str_enum!(
    /// `chromeDensity` (chrome-density.ts).
    ChromeDensity { Full => "full", Compact => "compact", Bare => "bare" }
);

str_enum!(
    /// `pluginInstallScope`.
    PluginInstallScope { User => "user", Project => "project" }
);

str_enum!(
    /// `doubleEscapeAction`.
    DoubleEscapeAction { Fork => "fork", Tree => "tree", None => "none" }
);

str_enum!(
    /// `treeFilterMode`.
    TreeFilterMode {
        Default => "default",
        NoTools => "no-tools",
        UserOnly => "user-only",
        LabeledOnly => "labeled-only",
        All => "all",
    }
);

str_enum!(
    /// `editorBorder`.
    EditorBorder { Rule => "rule", Box => "box" }
);

str_enum!(
    /// `thinkingDisplay`.
    ThinkingDisplay { Summarized => "summarized", Omitted => "omitted" }
);

str_enum!(
    /// `LearnSettingKey`: the flat `/learn` thresholds.
    LearnSettingKey {
        MaxSessions => "learnMaxSessions",
        MaxAgeDays => "learnMaxAgeDays",
        MinRepeats => "learnMinRepeats",
        MinRequestRepeats => "learnMinRequestRepeats",
        MaxProposals => "learnMaxProposals",
    }
);

/// `PackageSource`: a package, optionally filtered to some resources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageSource {
    Source(String),
    Filtered(PackageFilter),
}

impl PackageSource {
    pub fn source(&self) -> &str {
        match self {
            Self::Source(source) => source,
            Self::Filtered(filter) => &filter.source,
        }
    }
}

/// The object form of [`PackageSource`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackageFilter {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub themes: Option<Vec<String>>,
    /// Keys this version does not know, kept as written.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `Required<CompactionSettings>`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
    pub max_context_ratio: f64,
}

/// `Required<BranchSummarySettings>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchSummarySettings {
    pub reserve_tokens: u64,
    pub skip_prompt: bool,
}

/// `getRetrySettings()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetrySettings {
    pub enabled: bool,
    pub max_retries: u64,
    pub base_delay_ms: u64,
}

/// `getProviderRetrySettings()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRetrySettings {
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u64>,
    pub max_retry_delay_ms: u64,
}

/// `getLearnSettings()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LearnSettings {
    pub max_sessions: u64,
    pub max_age_days: u64,
    pub min_repeats: u64,
    pub min_request_repeats: u64,
    pub max_proposals: u64,
}

/// `ModelCategories`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelCategories {
    pub fast: Option<String>,
    pub standard: Option<String>,
    pub capable: Option<String>,
}

/// `WarningSettings` (`getWarnings` fills the defaults in).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WarningSettings {
    pub anthropic_extra_usage: Option<bool>,
    pub websearch_api_key: Option<bool>,
}

/// A persisted extension flag value (`flags`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagValue {
    Bool(bool),
    String(String),
}

impl FlagValue {
    pub(crate) fn to_json(&self) -> Value {
        match self {
            Self::Bool(b) => Value::Bool(*b),
            Self::String(s) => Value::String(s.clone()),
        }
    }
}

/// `DEFAULT_SETTINGS` as JSON.
const DEFAULT_SETTINGS_JSON: &str = r#"{
    "transport": "auto",
    "steeringMode": "one-at-a-time",
    "followUpMode": "one-at-a-time",
    "compaction": {
        "enabled": true,
        "reserveTokens": 16384,
        "keepRecentTokens": 20000,
        "maxContextRatio": 0.75
    },
    "toolOutput": {
        "maxBytes": 32768,
        "maxLines": 800
    },
    "contextGc": {
        "enabled": true
    },
    "voice": {
        "silenceMs": 800
    },
    "webtools": {
        "timeoutSecs": 15
    },
    "disabledTools": [],
    "toolOutputView": "peek",
    "branchSummary": {
        "reserveTokens": 16384,
        "skipPrompt": false
    },
    "retry": {
        "enabled": true,
        "maxRetries": 3,
        "baseDelayMs": 2000,
        "provider": {
            "maxRetryDelayMs": 60000
        }
    },
    "hideThinkingBlock": false,
    "quietStartup": false,
    "collapseChangelog": false,
    "enableInstallTelemetry": true,
    "enableSkillCommands": true,
    "enableSubagent": true,
    "warmSubagents": false,
    "maxSubagentDepth": 2,
    "nestedSubagentConcurrency": 2,
    "enableTodoWrite": true,
    "enablePluginTools": false,
    "pluginInstallScope": "user",
    "deferMcpSchemas": true,
    "enableWebTools": false,
    "enableSemanticIndex": true,
    "embsearchThresholdBytes": 0,
    "learnMaxSessions": 20,
    "learnMaxAgeDays": 30,
    "learnMinRepeats": 2,
    "learnMinRequestRepeats": 3,
    "learnMaxProposals": 8,
    "light": false,
    "terminal": {
        "showImages": true,
        "imageWidthCells": 60,
        "clearOnShrink": false,
        "showTerminalProgress": false,
        "chimeOnTurnComplete": false
    },
    "images": {
        "autoResize": true,
        "blockImages": false
    },
    "tips": {
        "enabled": true,
        "seen": [],
        "starNudges": 0
    },
    "doubleEscapeAction": "tree",
    "treeFilterMode": "default",
    "editorBorder": "box",
    "editorPaddingX": 1,
    "autocompleteMaxVisible": 5,
    "markdown": {
        "codeBlockIndent": "  "
    },
    "warnings": {
        "anthropicExtraUsage": true,
        "websearchApiKey": true
    },
    "packages": [],
    "extensions": [],
    "skills": [],
    "prompts": [],
    "slashCommands": [],
    "themes": []
}"#;

/// `DEFAULT_SETTINGS`.
pub fn default_settings() -> Settings {
    serde_json::from_str(DEFAULT_SETTINGS_JSON).expect("DEFAULT_SETTINGS is a JSON object")
}
