//! Resource types of `harness/types.ts` (hoocode v0.5.89). The
//! `ExecutionEnv` interface of the same file is ported with its tokio
//! implementation (ledger 9.4a).

use serde::{Deserialize, Serialize};

/// `Skill`: instructions the model can load on demand.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    /// Stable skill name used for lookup and model-visible listings.
    pub name: String,
    /// Short model-visible description of when to use the skill.
    pub description: String,
    /// Full skill instructions.
    pub content: String,
    /// Absolute path to the skill file.
    pub file_path: String,
    /// Keep the skill out of model-visible listings (explicit invocation
    /// still works).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_model_invocation: Option<bool>,
}

/// `PromptTemplate`: a prompt formatted for explicit invocation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplate {
    /// Stable template name used for lookup or command routing.
    pub name: String,
    /// Optional description for command lists or autocomplete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Template content; see [`crate::format_prompt_template_invocation`].
    pub content: String,
}
