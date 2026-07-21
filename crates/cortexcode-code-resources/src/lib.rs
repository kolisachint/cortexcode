//! Resource loading and skill management for the cortex coding agent.
//!
//! Mirrors `core/{skills,resource-loader}` from the TypeScript
//! `packages/coding-agent` package. Skills and context files are loaded from
//! the filesystem, parsed as JSON, markdown, or plain text, and assembled into
//! the context used by the agent prompt builder.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Resource representation
// ---------------------------------------------------------------------------

/// A loaded resource, such as a skill prompt, a context file, or an instruction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resource {
    /// Logical name of the resource.
    pub name: String,
    /// Resource kind: `skill`, `context`, `instruction`, or `raw`.
    pub kind: ResourceKind,
    /// File path the resource was loaded from, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Raw text content.
    pub content: String,
    /// Optional metadata parsed from the resource.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Kinds of resources the loader understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// A skill definition with a prompt and optional file globs.
    Skill,
    /// A context file that should be included in the conversation context.
    Context,
    /// A free-form instruction block.
    Instruction,
    /// A raw resource loaded without interpretation.
    Raw,
}

impl Resource {
    /// Create a new resource in memory.
    pub fn new(name: impl Into<String>, kind: ResourceKind, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind,
            path: None,
            content: content.into(),
            metadata: HashMap::new(),
        }
    }

    /// Return the resource as a formatted block suitable for injection into a
    /// system prompt.
    pub fn format_block(&self) -> String {
        match self.kind {
            ResourceKind::Skill => format!("## Skill: {}\n\n{}\n", self.name, self.content.trim()),
            ResourceKind::Context => format!(
                "## Context: {}\n\n```\n{}\n```\n",
                self.name,
                self.content.trim()
            ),
            ResourceKind::Instruction => {
                format!("## Instruction: {}\n\n{}\n", self.name, self.content.trim())
            }
            ResourceKind::Raw => self.content.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Skill definition
// ---------------------------------------------------------------------------

/// A skill definition loaded from a JSON or markdown file.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Skill {
    /// Skill name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Markdown instructions for the skill.
    pub instructions: String,
    /// File globs that the skill applies to.
    pub globs: Vec<String>,
    /// Named resources attached to the skill.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Extra metadata.
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Skill {
    /// Convert this skill into a `Resource` with kind `Skill`.
    pub fn to_resource(&self) -> Resource {
        let mut content = self.description.clone();
        if !self.instructions.is_empty() {
            content.push_str("\n\n");
            content.push_str(&self.instructions);
        }
        Resource {
            name: self.name.clone(),
            kind: ResourceKind::Skill,
            path: None,
            content,
            metadata: self.extra.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

/// Errors that can occur while loading resources.
#[derive(Debug)]
pub enum ResourceError {
    Io(std::io::Error),
    Json(serde_json::Error),
    NotFound(PathBuf),
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceError::Io(e) => write!(f, "io error: {}", e),
            ResourceError::Json(e) => write!(f, "json error: {}", e),
            ResourceError::NotFound(p) => write!(f, "resource not found: {}", p.display()),
        }
    }
}

impl std::error::Error for ResourceError {}

impl From<std::io::Error> for ResourceError {
    fn from(e: std::io::Error) -> Self {
        ResourceError::Io(e)
    }
}

impl From<serde_json::Error> for ResourceError {
    fn from(e: serde_json::Error) -> Self {
        ResourceError::Json(e)
    }
}

/// Load a single resource from a path.
///
/// Markdown files (`.md`) are loaded as `Instruction` resources. JSON files are
/// parsed as `Skill` definitions. Any other file is loaded as `Raw`.
pub fn load_resource(path: impl AsRef<Path>) -> Result<Resource, ResourceError> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(ResourceError::NotFound(path.to_path_buf()));
    }
    let content = std::fs::read_to_string(path)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("resource")
        .to_string();

    if ext == "json" {
        let skill: Skill = serde_json::from_str(&content)?;
        let mut resource = skill.to_resource();
        resource.path = Some(path.to_path_buf());
        Ok(resource)
    } else {
        let kind = if ext == "md" {
            ResourceKind::Instruction
        } else {
            ResourceKind::Raw
        };
        Ok(Resource {
            name,
            kind,
            path: Some(path.to_path_buf()),
            content,
            metadata: HashMap::new(),
        })
    }
}

/// Load a directory of resources.
///
/// Each immediate file becomes a `Resource`. Subdirectories become groups with
/// a resource name derived from the directory name.
pub fn load_resources_dir(path: impl AsRef<Path>) -> Result<Vec<Resource>, ResourceError> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut resources = Vec::new();
    for entry in walkdir::WalkDir::new(path)
        .max_depth(2)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            resources.push(load_resource(entry.path())?);
        }
    }
    Ok(resources)
}

/// Load all skill definitions from a directory.
pub fn load_skills_dir(path: impl AsRef<Path>) -> Result<Vec<Skill>, ResourceError> {
    let path = path.as_ref();
    let mut skills = Vec::new();
    if !path.exists() {
        return Ok(skills);
    }
    for entry in walkdir::WalkDir::new(path)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if ext == "json" {
                let content = std::fs::read_to_string(entry.path())?;
                let mut skill: Skill = serde_json::from_str(&content)?;
                if skill.name.is_empty() {
                    skill.name = entry
                        .path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("skill")
                        .to_string();
                }
                skills.push(skill);
            }
        }
    }
    Ok(skills)
}

/// Load context files referenced by path globs.
pub fn load_context_files(
    paths: impl IntoIterator<Item = impl AsRef<Path>>,
) -> Result<Vec<Resource>, ResourceError> {
    let mut resources = Vec::new();
    for path in paths {
        let path = path.as_ref();
        if path.is_file() {
            let mut resource = load_resource(path)?;
            resource.kind = ResourceKind::Context;
            resources.push(resource);
        } else if path.is_dir() {
            for mut resource in load_resources_dir(path)? {
                resource.kind = ResourceKind::Context;
                resources.push(resource);
            }
        }
    }
    Ok(resources)
}

/// Assemble resources into a single context string.
pub fn assemble_context(resources: &[Resource]) -> String {
    resources
        .iter()
        .map(|r| r.format_block())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Default directory where skills are stored relative to the current working
/// directory.
pub fn default_skills_dir() -> PathBuf {
    PathBuf::from(".cortexcode/skills")
}

/// Default directory where context files are stored relative to the current
/// working directory.
pub fn default_context_dir() -> PathBuf {
    PathBuf::from(".cortexcode/context")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_format_block() {
        let resource = Resource::new("test", ResourceKind::Skill, "do this");
        let block = resource.format_block();
        assert!(block.contains("Skill: test"));
        assert!(block.contains("do this"));
    }

    #[test]
    fn test_load_resource_json_skill() {
        let dir =
            std::env::temp_dir().join(format!("cortex-resources-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.json");
        std::fs::write(
            &path,
            r#"{"description":"desc","instructions":"inst","globs":["*.rs"]}"#,
        )
        .unwrap();
        let resource = load_resource(&path).unwrap();
        assert_eq!(resource.kind, ResourceKind::Skill);
        assert!(resource.content.contains("desc"));
        assert!(resource.content.contains("inst"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_resource_not_found() {
        let path = PathBuf::from("/this/should/not/exist");
        assert!(matches!(
            load_resource(&path),
            Err(ResourceError::NotFound(_))
        ));
    }

    #[test]
    fn test_load_skills_dir() {
        let dir = std::env::temp_dir().join(format!("cortex-skills-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rust.json"),
            r#"{"description":"Rust skill","instructions":"Use idiomatic Rust."}"#,
        )
        .unwrap();
        let skills = load_skills_dir(&dir).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "rust");
        assert!(skills[0].instructions.contains("Rust"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_assemble_context() {
        let resources = vec![
            Resource::new("a", ResourceKind::Skill, "skill-a"),
            Resource::new("b", ResourceKind::Context, "context-b"),
        ];
        let context = assemble_context(&resources);
        assert!(context.contains("Skill: a"));
        assert!(context.contains("Context: b"));
    }
}
