//! `core/search/mode.ts`: availability-first mode resolution.

use std::sync::LazyLock;

use crate::types::{ResolvedSearchMode, SearchMode};

/// `ModeResolution`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeResolution {
    pub mode: ResolvedSearchMode,
    /// Set when the resolved mode is a forced degradation of the request.
    pub degraded_reason: Option<String>,
}

static QUOTED: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"["'`][^"'`]*["'`]"#).expect("quoted pattern"));
static METACHARS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[\\^$|()\[\]{}*+?]").expect("metachar pattern"));

/// `hasStrongLexicalSignals`: regex metacharacters outside quoted segments.
pub fn has_strong_lexical_signals(query: &str) -> bool {
    let unquoted = QUOTED.replace_all(query, " ");
    METACHARS.is_match(&unquoted)
}

/// `resolveSearchMode`.
pub fn resolve_search_mode(
    query: &str,
    requested: SearchMode,
    embed_available: bool,
    embed_unavailable_reason: Option<&str>,
) -> ModeResolution {
    let resolved = |mode, degraded_reason| ModeResolution {
        mode,
        degraded_reason,
    };
    if requested == SearchMode::Lexical {
        return resolved(ResolvedSearchMode::Lexical, None);
    }
    if !embed_available {
        let reason = embed_unavailable_reason.unwrap_or("semantic index unavailable");
        return if requested == SearchMode::Auto {
            resolved(ResolvedSearchMode::Lexical, None)
        } else {
            resolved(
                ResolvedSearchMode::Lexical,
                Some(format!("{} requested but {reason}", requested.as_str())),
            )
        };
    }
    match requested {
        SearchMode::Semantic => resolved(ResolvedSearchMode::Semantic, None),
        SearchMode::Hybrid => resolved(ResolvedSearchMode::Hybrid, None),
        _ if has_strong_lexical_signals(query) => resolved(ResolvedSearchMode::Lexical, None),
        _ => resolved(ResolvedSearchMode::Hybrid, None),
    }
}
