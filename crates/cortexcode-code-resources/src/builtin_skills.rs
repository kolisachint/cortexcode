//! `core/builtin-skills.ts`: skills the app ships, materialized into a
//! content-addressed cache so a skill's `<location>` is a readable file.
//!
//! The embedded files are hoocode's `templates/skills/**` verbatim, so the cache
//! directory name (the content hash) is the same as hoocode's.

use crate::node_path;
use sha2::{Digest, Sha256};

/// `EMBEDDED_SKILLS`: relative path -> content, in path order.
pub const EMBEDDED_SKILLS: &[(&str, &str)] = &[
    (
        "artifact-design/SKILL.md",
        include_str!("../templates/skills/artifact-design/SKILL.md"),
    ),
    (
        "canvas-design/SKILL.md",
        include_str!("../templates/skills/canvas-design/SKILL.md"),
    ),
    (
        "plugin-authoring/SKILL.md",
        include_str!("../templates/skills/plugin-authoring/SKILL.md"),
    ),
];

/// `BuiltinSkillGate`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuiltinSkillGate {
    /// The `enablePluginTools` setting.
    pub enable_plugin_tools: bool,
}

/// `BuiltinSkill`.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinSkill {
    pub name: &'static str,
    pub summary: &'static str,
    /// Registered only when this returns true; `None` = always.
    pub gate: Option<fn(BuiltinSkillGate) -> bool>,
}

/// `BUILTIN_SKILLS`.
pub const BUILTIN_SKILLS: &[BuiltinSkill] = &[
    BuiltinSkill {
        name: "artifact-design",
        summary: "Craft for a self-contained HTML visual: read the treatment the request calls for, write the color/type/layout plan before the markup, and avoid the looks generated design keeps landing on.",
        gate: None,
    },
    BuiltinSkill {
        name: "canvas-design",
        summary: "The canvas half of design craft: template-string markup, no dependencies and no build, owning the theme outright, rendering state two operators both mutate, and what actions cost while an instance is open.",
        gate: None,
    },
    BuiltinSkill {
        name: "plugin-authoring",
        summary: "The craft half of ProposePlugin/UpdatePlugin: when a capability is worth extracting, naming it so it triggers again, portability, and the hook trap.",
        gate: Some(|g| g.enable_plugin_tools),
    },
];

/// `contentHash`: sha256 over `path \0 content \0` of every embedded file (in
/// path order), first 12 hex digits.
fn content_hash() -> String {
    let mut entries: Vec<&(&str, &str)> = EMBEDDED_SKILLS.iter().collect();
    entries.sort_by_key(|(path, _)| *path);
    let mut hash = Sha256::new();
    for (path, content) in entries {
        hash.update(path.as_bytes());
        hash.update([0u8]);
        hash.update(content.as_bytes());
        hash.update([0u8]);
    }
    let digest = hash.finalize();
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..12]
        .to_string()
}

/// `builtinSkillsCacheDir`.
pub fn builtin_skills_cache_dir(agent_dir: &str) -> String {
    node_path::join(&[agent_dir, "cache", "builtin-skills", &content_hash()])
}

fn materialize(root: &str) -> std::io::Result<()> {
    for (relative, content) in EMBEDDED_SKILLS {
        let target = node_path::join(&[root, relative]);
        if std::fs::read_to_string(&target).is_ok_and(|existing| existing == *content) {
            continue;
        }
        std::fs::create_dir_all(node_path::dirname(&target))?;
        // Write-then-rename: a killed process never leaves a half-written skill.
        let temp = format!("{target}.{}.tmp", std::process::id());
        std::fs::write(&temp, content)?;
        std::fs::rename(&temp, &target)?;
    }
    Ok(())
}

/// `materializeBuiltinSkills`: the cache root, or `None` when it cannot be
/// written (the session then simply has no built-in skills).
pub fn materialize_builtin_skills(agent_dir: &str) -> Option<String> {
    let root = builtin_skills_cache_dir(agent_dir);
    match materialize(&root) {
        Ok(()) => Some(root),
        Err(_) => {
            let _ = std::fs::remove_dir_all(&root);
            None
        }
    }
}

/// `canvasDesignGuidePath`.
pub fn canvas_design_guide_path(agent_dir: &str) -> Option<String> {
    materialize_builtin_skills(agent_dir)?;
    let guide = node_path::join(&[
        &builtin_skills_cache_dir(agent_dir),
        "canvas-design",
        "SKILL.md",
    ]);
    std::path::Path::new(&guide).exists().then_some(guide)
}

/// `builtinSkillPaths`: the enabled skills' directories (after gating).
pub fn builtin_skill_paths(gate: BuiltinSkillGate, agent_dir: &str) -> Vec<String> {
    let enabled: Vec<&BuiltinSkill> = BUILTIN_SKILLS
        .iter()
        .filter(|s| s.gate.is_none_or(|g| g(gate)))
        .collect();
    if enabled.is_empty() {
        return Vec::new();
    }
    let Some(root) = materialize_builtin_skills(agent_dir) else {
        return Vec::new();
    };
    enabled
        .iter()
        .map(|s| node_path::join(&[&root, s.name]))
        .filter(|d| std::path::Path::new(d).exists())
        .collect()
}
