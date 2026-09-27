//! `utils/frontmatter.ts`: an optional leading `---` YAML block and the
//! trimmed body. Same algorithm as the agent-core harness, so it is shared.

pub use cortexcode_agent_harness::frontmatter::parse_frontmatter;

/// `stripFrontmatter`: the body only. A malformed YAML block throws in hoocode;
/// here it is an `Err`.
pub fn strip_frontmatter(content: &str) -> Result<String, String> {
    parse_frontmatter(content).map(|(_, body)| body)
}
