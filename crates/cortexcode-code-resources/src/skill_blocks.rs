//! `core/agent-session-skills.ts`: the `<skill …>` envelope in user messages
//! and `/skill:name args` expansion.

use crate::skills::Skill;

/// `ParsedSkillBlock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSkillBlock {
    pub name: String,
    pub location: String,
    pub content: String,
    pub user_message: Option<String>,
}

/// `parseSkillBlock`: the whole text must be
/// `<skill name="N" location="L">\n…\n</skill>` optionally followed by
/// `\n\n` and a user message.
pub fn parse_skill_block(text: &str) -> Option<ParsedSkillBlock> {
    let rest = text.strip_prefix("<skill name=\"")?;
    let name_end = rest.find('"')?;
    let name = &rest[..name_end];
    let rest = rest[name_end..].strip_prefix("\" location=\"")?;
    let loc_end = rest.find('"')?;
    let location = &rest[..loc_end];
    let body = rest[loc_end..].strip_prefix("\">\n")?;
    if name.is_empty() || location.is_empty() {
        return None;
    }
    // Lazy `([\s\S]*?)\n<\/skill>` anchored by `(?:\n\n([\s\S]+))?$`.
    let mut search = 0;
    while let Some(i) = body[search..].find("\n</skill>") {
        let at = search + i;
        let after = &body[at + "\n</skill>".len()..];
        let user_message = if after.is_empty() {
            Some(None)
        } else {
            after
                .strip_prefix("\n\n")
                .filter(|m| !m.is_empty())
                .map(|m| Some(m.trim().to_string()).filter(|m| !m.is_empty()))
        };
        if let Some(user_message) = user_message {
            return Some(ParsedSkillBlock {
                name: name.to_string(),
                location: location.to_string(),
                content: body[..at].to_string(),
                user_message,
            });
        }
        search = at + 1;
    }
    None
}

/// `expandSkillCommand`: `/skill:name args` becomes the skill block (plus the
/// args); unknown skills and non-commands pass through. A read failure is
/// returned as `Err((file_path, message))` alongside the unchanged text.
pub fn expand_skill_command(text: &str, skills: &[Skill]) -> (String, Option<(String, String)>) {
    let Some(rest) = text.strip_prefix("/skill:") else {
        return (text.to_string(), None);
    };
    let (skill_name, args) = match rest.find(' ') {
        Some(i) => (&rest[..i], rest[i + 1..].trim()),
        None => (rest, ""),
    };
    let Some(skill) = skills.iter().find(|s| s.name == skill_name) else {
        return (text.to_string(), None);
    };
    let body = std::fs::read_to_string(&skill.file_path)
        .map_err(|e| e.to_string())
        .and_then(|content| crate::frontmatter::strip_frontmatter(&content));
    match body {
        Ok(body) => {
            let block = format!(
                "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
                skill.name,
                skill.file_path,
                skill.base_dir,
                body.trim()
            );
            let expanded = if args.is_empty() {
                block
            } else {
                format!("{block}\n\n{args}")
            };
            (expanded, None)
        }
        Err(error) => (text.to_string(), Some((skill.file_path.clone(), error))),
    }
}
