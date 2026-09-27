//! `core/source-info.ts`: where a resource came from.

/// `SourceScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceScope {
    User,
    Project,
    Temporary,
}

/// `SourceOrigin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOrigin {
    Package,
    TopLevel,
    ClaudeCode,
}

/// `SourceInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub path: String,
    pub source: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    pub base_dir: Option<String>,
}

impl SourceInfo {
    /// The TS `SourceInfo` object (`{path, source, scope, origin, baseDir?}`).
    pub fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("path".into(), self.path.clone().into());
        map.insert("source".into(), self.source.clone().into());
        let scope = match self.scope {
            SourceScope::User => "user",
            SourceScope::Project => "project",
            SourceScope::Temporary => "temporary",
        };
        map.insert("scope".into(), scope.into());
        let origin = match self.origin {
            SourceOrigin::Package => "package",
            SourceOrigin::TopLevel => "top-level",
            SourceOrigin::ClaudeCode => "claude-code",
        };
        map.insert("origin".into(), origin.into());
        if let Some(base_dir) = &self.base_dir {
            map.insert("baseDir".into(), base_dir.clone().into());
        }
        serde_json::Value::Object(map)
    }
}

/// `createSyntheticSourceInfo`: scope defaults to temporary, origin to top-level.
pub fn create_synthetic_source_info(
    path: &str,
    source: &str,
    scope: Option<SourceScope>,
    origin: Option<SourceOrigin>,
    base_dir: Option<&str>,
) -> SourceInfo {
    SourceInfo {
        path: path.to_string(),
        source: source.to_string(),
        scope: scope.unwrap_or(SourceScope::Temporary),
        origin: origin.unwrap_or(SourceOrigin::TopLevel),
        base_dir: base_dir.map(str::to_string),
    }
}
