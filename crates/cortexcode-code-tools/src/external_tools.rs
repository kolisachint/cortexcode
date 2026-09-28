//! The external (Rust) binaries hoocode can use and what each one is worth,
//! hoocode `core/external-tools.ts`.
//!
//! The agent works without any of them; they are an expansion layer. This is
//! the one description of that layer: the `/settings` pane renders it, and the
//! same table says which pane rows are gated on a binary.
//!
//! [`describe_external_tools`] resolves each binary on this machine with the
//! never-downloads half of `utils/tools-manager.ts` ([`get_tool_path`],
//! [`get_tool_status`]); fetching a missing binary is not ported.

use std::collections::HashMap;

/// How a missing binary is acquired (`Acquisition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquisition {
    /// Fetched in the background at startup.
    Startup,
    /// Fetched the first time the feature is used.
    OnDemand,
    /// Never fetched implicitly.
    Manual,
}

/// `ExternalToolDoc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolDoc {
    pub tool: &'static str,
    /// Row label in the pane.
    pub label: &'static str,
    /// What the binary does, in one line.
    pub summary: &'static str,
    /// What it turns on, feature by feature.
    pub enables: &'static [&'static str],
    /// What happens instead when it is missing.
    pub fallback: &'static str,
    pub acquisition: Acquisition,
    /// Env vars that change how the binary is resolved or driven.
    pub env: &'static [&'static str],
    /// `/settings` row ids whose effect depends on this binary.
    pub dependent_rows: &'static [&'static str],
    /// settings.json keys this binary gates.
    pub settings_keys: &'static [&'static str],
}

/// `EXTERNAL_TOOLS`: the two that make things faster first, then the three
/// that add capability.
pub const EXTERNAL_TOOLS: &[ExternalToolDoc] = &[
    ExternalToolDoc {
        tool: "rg",
        label: "ripgrep (rg)",
        summary: "Fast path for content search.",
        enables: &["the lexical half of search runs rg instead of the JS scanner"],
        fallback: "A pure-JS scanner produces the same match shape, so results are identical - it is materially slower on large trees and respects fewer ignore-file edge cases.",
        acquisition: Acquisition::Startup,
        env: &["HOOCODE_RG_BINARY", "HOOCODE_NATIVE_SEARCH=1 forces the JS path even when rg is present"],
        dependent_rows: &[],
        settings_keys: &[],
    },
    ExternalToolDoc {
        tool: "fd",
        label: "fd",
        summary: "Fast path for filename search.",
        enables: &["@-file autocomplete lists paths with fd instead of the JS directory walker"],
        fallback: "A JS walker produces the same result shape - slower on large trees, and glob/ignore handling is the JS approximation rather than fd's.",
        acquisition: Acquisition::Startup,
        env: &["HOOCODE_FD_BINARY", "HOOCODE_NATIVE_SEARCH=1 forces the JS path even when fd is present"],
        dependent_rows: &[],
        settings_keys: &[],
    },
    ExternalToolDoc {
        tool: "embsearch",
        label: "embsearch (semantic index)",
        summary: "Local embedding index. The only source of semantic ranking in hoocode.",
        enables: &[
            "search fuses semantic hits with its lexical hits",
            "MCP/capability deferral ranks tools by meaning rather than keyword",
        ],
        fallback: "search is lexical-only and capability lookup ranks lexically. Nothing errors; queries phrased by intent rather than by token simply rank worse. Requires the ONNX build - the mock build is rejected on purpose, because it would rank at random while looking healthy.",
        acquisition: Acquisition::OnDemand,
        env: &["HOOCODE_EMBSEARCH_BINARY"],
        dependent_rows: &["group:embsearch"],
        settings_keys: &["enableSemanticIndex", "embsearchBinaryPath", "embsearchThresholdBytes"],
    },
    ExternalToolDoc {
        tool: "webtools",
        label: "webtools (webfetch/websearch)",
        summary: "The network layer. Without it hoocode has no way to reach the internet.",
        enables: &["the webfetch tool", "the websearch tool"],
        fallback: "Both tools return an error when called. The web tool group is off by default, so a missing binary is invisible until you turn the group on.",
        acquisition: Acquisition::OnDemand,
        env: &["HOOCODE_WEBTOOLS_BINARY", "HOOCODE_WEBTOOLS_TIMEOUT"],
        dependent_rows: &["group:web", "webtools-timeout-secs"],
        settings_keys: &["enableWebTools", "webtools.timeoutSecs"],
    },
    ExternalToolDoc {
        tool: "voicetools",
        label: "voicetools (voice input)",
        summary: "Microphone capture and transcription for the TUI.",
        enables: &["push-to-talk voice input in the editor"],
        fallback: "Voice capture reports an error and never starts. Typing is unaffected.",
        acquisition: Acquisition::OnDemand,
        env: &["VOICETOOLS_BIN", "HOOCODE_VOICETOOLS_BINARY", "VOICETOOLS_SILENCE_MS"],
        dependent_rows: &["voice-silence-ms"],
        settings_keys: &["voice.silenceMs"],
    },
];

/// How a present binary was resolved (`ManagedToolSource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedToolSource {
    /// The env var names it.
    Override,
    /// The agent's own downloaded copy.
    Managed,
    /// Found on PATH.
    Path,
}

/// `ExternalToolStatus`: the doc plus how (and whether) it resolved here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolStatus {
    pub doc: ExternalToolDoc,
    /// Upstream display name (`ManagedToolStatus.name`).
    pub name: String,
    /// GitHub repo the release archives come from.
    pub repo: String,
    /// Env var that overrides resolution with an explicit path.
    pub override_env: String,
    pub path: Option<String>,
    pub source: Option<ManagedToolSource>,
    pub installed: bool,
    /// Whether it would be fetched if the feature were used now.
    pub downloadable: bool,
}

/// Short status word for the pane's value column (`statusLabel`).
pub fn status_label(status: &ExternalToolStatus) -> &'static str {
    if !status.installed {
        return if status.downloadable {
            "not installed"
        } else {
            "unavailable"
        };
    }
    match status.source {
        Some(ManagedToolSource::Override) => "env override",
        Some(ManagedToolSource::Path) => "system",
        Some(ManagedToolSource::Managed) | None => "installed",
    }
}

/// Pane-row id -> the binary it needs (`buildRowGates`).
pub fn build_row_gates(statuses: &[ExternalToolStatus]) -> HashMap<String, ExternalToolStatus> {
    let mut gates = HashMap::new();
    for status in statuses {
        for row in status.doc.dependent_rows {
            gates.insert(row.to_string(), status.clone());
        }
    }
    gates
}

/// `ToolConfig`: where a managed binary comes from and what it is called.
struct ToolConfig {
    tool: &'static str,
    name: &'static str,
    repo: &'static str,
    binary_name: &'static str,
    /// Names to look for on PATH (`systemBinaryNames`, default the binary).
    system_binary_names: &'static [&'static str],
}

/// `TOOLS`.
const TOOLS: &[ToolConfig] = &[
    ToolConfig {
        tool: "fd",
        name: "fd",
        repo: "sharkdp/fd",
        binary_name: "fd",
        system_binary_names: &["fd", "fdfind"],
    },
    ToolConfig {
        tool: "rg",
        name: "ripgrep",
        repo: "BurntSushi/ripgrep",
        binary_name: "rg",
        system_binary_names: &["rg"],
    },
    ToolConfig {
        tool: "webtools",
        name: "webtools",
        repo: "kolisachint/webtools",
        binary_name: "webtools",
        system_binary_names: &["webtools"],
    },
    ToolConfig {
        tool: "voicetools",
        name: "voicetools",
        repo: "kolisachint/voicetools",
        binary_name: "voicetools",
        system_binary_names: &["voicetools"],
    },
    ToolConfig {
        tool: "embsearch",
        name: "embsearch",
        repo: "kolisachint/embeddingsearchtools",
        binary_name: "embsearch",
        system_binary_names: &["embsearch"],
    },
];

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// `commandExists`: an executable of that name on PATH. The original probes
/// by spawning `<cmd> --version`; a PATH lookup finds the same binaries
/// without running them.
fn command_exists(cmd: &str) -> bool {
    let exe = exe_name(cmd);
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(&exe).is_file()))
}

/// The per-tool override variable (`<PREFIX><TOOL>_BINARY`) that is set,
/// with its value.
fn override_path(tool: &str) -> Option<(String, String)> {
    cortexcode_code_paths::ENV_PREFIXES
        .iter()
        .find_map(|prefix| {
            let name = format!("{prefix}{}_BINARY", tool.to_uppercase());
            let value = std::env::var(&name).ok()?.trim().to_string();
            (!value.is_empty()).then_some((name, value))
        })
}

/// `getToolPath`: an already-available binary (override, managed copy, or a
/// command name on PATH), never downloading.
pub fn get_tool_path(tool: &str) -> Option<String> {
    let config = TOOLS.iter().find(|c| c.tool == tool)?;
    if let Some((_, path)) = override_path(tool) {
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }
    let local = cortexcode_code_paths::bin_dir().join(exe_name(config.binary_name));
    if local.exists() {
        return Some(local.to_string_lossy().into_owned());
    }
    config
        .system_binary_names
        .iter()
        .find(|name| command_exists(name))
        .map(|name| name.to_string())
}

/// `ManagedToolStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedToolStatus {
    pub name: String,
    pub repo: String,
    pub override_env: String,
    pub path: Option<String>,
    pub source: Option<ManagedToolSource>,
}

/// `getToolStatus`: where a binary resolves from, if anywhere.
pub fn get_tool_status(tool: &str) -> ManagedToolStatus {
    let config = TOOLS.iter().find(|c| c.tool == tool);
    let override_env = override_path(tool)
        .map(|(name, _)| name)
        .unwrap_or_else(|| format!("HOOCODE_{}_BINARY", tool.to_uppercase()));
    let mut status = ManagedToolStatus {
        name: config.map_or(tool, |c| c.name).to_string(),
        repo: config.map_or("", |c| c.repo).to_string(),
        override_env,
        path: None,
        source: None,
    };
    let (Some(config), Some(path)) = (config, get_tool_path(tool)) else {
        return status;
    };
    let managed = cortexcode_code_paths::bin_dir().join(exe_name(config.binary_name));
    status.source = Some(
        if override_path(tool).is_some_and(|(_, value)| value == path) {
            ManagedToolSource::Override
        } else if std::path::Path::new(&path) == managed {
            ManagedToolSource::Managed
        } else {
            ManagedToolSource::Path
        },
    );
    status.path = Some(path);
    status
}

/// `describeExternalTools`: every external tool resolved. Never downloads:
/// this is what the pane shows before the user opts into anything.
pub fn describe_external_tools() -> Vec<ExternalToolStatus> {
    let offline = cortexcode_code_paths::is_offline_mode();
    let android = cfg!(target_os = "android");
    EXTERNAL_TOOLS
        .iter()
        .map(|doc| {
            let status = get_tool_status(doc.tool);
            ExternalToolStatus {
                doc: doc.clone(),
                name: status.name,
                repo: status.repo,
                override_env: status.override_env,
                installed: status.path.is_some(),
                path: status.path,
                source: status.source,
                downloadable: !offline && !android && doc.acquisition != Acquisition::Manual,
            }
        })
        .collect()
}
