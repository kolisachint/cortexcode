//! `parseFrontmatter` of `harness/skills.ts` / `prompt-templates.ts`
//! (v0.5.89): an optional `---` YAML block, then the trimmed body.

use serde_json::{Map, Value};

/// Parsed frontmatter (an object; empty when absent or not a mapping) and
/// the body. A malformed YAML block is an error with the parser's message.
pub fn parse_frontmatter(content: &str) -> Result<(Map<String, Value>, String), String> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return Ok((Map::new(), normalized));
    }
    let Some(end) = normalized
        .get(3..)
        .and_then(|rest| rest.find("\n---"))
        .map(|i| i + 3)
    else {
        return Ok((Map::new(), normalized));
    };
    let yaml = normalized.get(4..end).unwrap_or("");
    let body = normalized[end + 4..].trim().to_string();
    let value: Value = serde_yaml_ng::from_str(yaml).map_err(|e| e.to_string())?;
    let frontmatter = match value {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    Ok((frontmatter, body))
}

/// An approximation of `String.prototype.localeCompare` for file names:
/// case-insensitive first, lowercase before uppercase on a tie.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| b.cmp(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splits_yaml_frontmatter_from_the_body() {
        let (fm, body) =
            parse_frontmatter("---\r\nname: x\r\nflag: true\r\n---\r\n\r\nBody\r\n").unwrap();
        assert_eq!(Value::Object(fm), json!({"name": "x", "flag": true}));
        assert_eq!(body, "Body");
        let (fm, body) = parse_frontmatter("No frontmatter\n").unwrap();
        assert!(fm.is_empty());
        assert_eq!(body, "No frontmatter\n");
        let (fm, _) = parse_frontmatter("---\n---\nBody").unwrap();
        assert!(fm.is_empty());
        assert!(parse_frontmatter("---\ndescription: [unterminated\n---\nBody").is_err());
    }
}
