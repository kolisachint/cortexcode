//! Session-wide artifact platform targeting (`--platform`), hoocode
//! `core/extensions/plugins/formats/platform-targets.ts`.
//!
//! Two resolvers because the two kinds of write have different rules: a
//! plugin is a distribution unit and can never target `agents`
//! ([`resolve_plugin_platforms`]); a workspace scaffold is consumed in place,
//! where `.agents/` is a real convention ([`get_workspace_platforms`]).

use std::sync::Mutex;

use crate::MarketplacePlatform;

/// The platforms a plugin can be produced for (`PluginPlatform`): never
/// `agents`.
pub fn is_plugin_platform(platform: MarketplacePlatform) -> bool {
    platform != MarketplacePlatform::Agents
}

/// `DEFAULT_PLUGIN_PLATFORMS`: Claude, the only platform whose local loop
/// closes (an authored plugin is live on the next session).
pub const DEFAULT_PLUGIN_PLATFORMS: &[MarketplacePlatform] = &[MarketplacePlatform::Claude];

/// Canonicalize one platform token, folding the user-facing aliases.
pub fn normalize_platform_token(token: &str) -> Option<MarketplacePlatform> {
    match token.trim().to_lowercase().as_str() {
        "agents" | "native" => Some(MarketplacePlatform::Agents),
        "claude" => Some(MarketplacePlatform::Claude),
        "github" | "copilot" | "gh" => Some(MarketplacePlatform::Github),
        _ => None,
    }
}

/// `PlatformParse`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlatformParse {
    /// Canonical platforms, deduped, in the order first mentioned.
    pub platforms: Vec<MarketplacePlatform>,
    /// Tokens that matched no known platform (diagnostics, never fatal).
    pub invalid: Vec<String>,
}

/// Parse raw `--platform` / `platform` tokens into canonical platforms.
pub fn parse_platforms<S: AsRef<str>>(tokens: &[S]) -> PlatformParse {
    let mut parse = PlatformParse::default();
    for token in tokens {
        let trimmed = token.as_ref().trim();
        if trimmed.is_empty() {
            continue;
        }
        match normalize_platform_token(trimmed) {
            None => parse.invalid.push(trimmed.to_string()),
            Some(p) if !parse.platforms.contains(&p) => parse.platforms.push(p),
            Some(_) => {}
        }
    }
    parse
}

/// Session-wide targets. `None` = not configured.
static SESSION_PLATFORMS: Mutex<Option<Vec<MarketplacePlatform>>> = Mutex::new(None);

fn session() -> std::sync::MutexGuard<'static, Option<Vec<MarketplacePlatform>>> {
    SESSION_PLATFORMS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Set (or clear, with `None`/empty) the session's artifact platform targets.
pub fn set_platforms(platforms: Option<&[MarketplacePlatform]>) {
    *session() = platforms.filter(|p| !p.is_empty()).map(<[_]>::to_vec);
}

/// The session's targets for workspace scaffolds, or `None` when unset
/// (callers then use the native location rather than a default).
pub fn get_workspace_platforms() -> Option<Vec<MarketplacePlatform>> {
    session().clone()
}

/// Resolve the platforms a plugin should be produced for: an explicit
/// selection wins, then the session targets (with `agents` filtered out),
/// then [`DEFAULT_PLUGIN_PLATFORMS`]. An explicit `agents` is an error.
pub fn resolve_plugin_platforms(
    explicit: Option<&[MarketplacePlatform]>,
) -> Result<Vec<MarketplacePlatform>, String> {
    if let Some(explicit) = explicit.filter(|e| !e.is_empty()) {
        let rejected: Vec<&str> = explicit
            .iter()
            .filter(|p| !is_plugin_platform(**p))
            .map(|p| p.as_str())
            .collect();
        if !rejected.is_empty() {
            return Err(format!(
                "Cannot author a plugin for platform \"{}\": plugins are distribution units and \
                 the native `agents` layout belongs to no marketplace, so it can never be published or installed. \
                 Target `claude` or `github`, or author a workspace skill instead.",
                rejected.join(", ")
            ));
        }
        return Ok(explicit.to_vec());
    }
    let from_session: Vec<MarketplacePlatform> = session()
        .as_deref()
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|p| is_plugin_platform(*p))
        .collect();
    Ok(if from_session.is_empty() {
        DEFAULT_PLUGIN_PLATFORMS.to_vec()
    } else {
        from_session
    })
}
