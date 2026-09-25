//! System prompt construction (`core/system-prompt.ts`), with the sections it
//! appends: skills (`formatSkillsForPrompt` in `core/skills.ts`), agents
//! (`formatAgentsForPrompt` in `core/agent-registry.ts`) and the app's own docs
//! (`core/self-docs.ts`).
//!
//! The text is hoocode's with the app name swapped (`APP_NAME`): the parity
//! harness normalizes the two names, so everything else must match exactly.

use std::path::{Path, PathBuf};

/// The app name the prompt introduces itself with (hoocode's `APP_NAME`).
pub const APP_NAME: &str = "cortex";

/// Tool name of the subagent tool (`TASK_TOOL_NAME`).
pub const TASK_TOOL_NAME: &str = "Task";

/// Name of the tool that searches the app's own docs (hoocode `SearchHooCode`).
/// Kept verbatim until the self-knowledge extension is ported.
pub const SELF_SEARCH_TOOL_NAME: &str = "SearchHooCode";

/// Terse replacement for the default system prompt in light mode
/// (`LIGHT_SYSTEM_PROMPT` in `core/light.ts`). `build_system_prompt` appends the
/// date and working directory; light mode disables everything else.
pub const LIGHT_SYSTEM_PROMPT: &str = "You are a coding agent. Use the tools to read, edit, and write files and run shell commands.\nSearch with bash (rg/find/ls). Prefer edit for changes; write for new files.\nBe concise. No preamble.";

/// A project context file (`AGENTS.md`/`CLAUDE.md`) already loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    pub path: String,
    pub content: String,
}

/// The parts of a skill the prompt lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptSkill {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub allowed_tools: Vec<String>,
    pub disable_model_invocation: bool,
}

/// The parts of an agent definition the prompt lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptAgent {
    pub name: String,
    pub description: String,
    pub tools: Vec<String>,
    pub model: Option<String>,
}

/// One of the app's shipped docs (`SelfDoc`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfDoc {
    /// The filename, e.g. `skills.md`.
    pub id: String,
    /// Absolute path.
    pub path: PathBuf,
}

/// `BuildSystemPromptOptions`.
#[derive(Debug, Clone, Default)]
pub struct BuildSystemPromptOptions {
    /// Custom system prompt (replaces the default).
    pub custom_prompt: Option<String>,
    /// Tools to include. Default: read, bash, edit, write, SearchCodebase.
    pub selected_tools: Option<Vec<String>>,
    /// One-line tool snippets keyed by tool name.
    pub tool_snippets: Vec<(String, String)>,
    /// Additional guideline bullets appended to the defaults.
    pub prompt_guidelines: Vec<String>,
    /// Text to append to the system prompt.
    pub append_system_prompt: Option<String>,
    /// Working directory.
    pub cwd: String,
    pub context_files: Vec<ContextFile>,
    pub skills: Vec<PromptSkill>,
    /// Emitted only when the Task tool is active.
    pub agents: Vec<PromptAgent>,
    /// Point the model at the app's own docs. Default: true for the built-in
    /// prompt, false when `custom_prompt` replaces it.
    pub include_self_docs: Option<bool>,
    /// The docs listing (`listSelfDocs()`); computed by the caller because the
    /// install location is the caller's to know.
    pub self_docs: Vec<SelfDoc>,
    /// `YYYY-MM-DD`; defaults to today's local date.
    pub date: Option<String>,
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// `formatSkillsForPrompt`: skills the model may load, or `""`.
pub fn format_skills_for_prompt(skills: &[PromptSkill]) -> String {
    let visible: Vec<&PromptSkill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the read tool to load a skill's file when the task matches its description.".to_string(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];
    for skill in visible {
        lines.push("  <skill>".into());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        if !skill.allowed_tools.is_empty() {
            lines.push(format!(
                "    <tools>{}</tools>",
                escape_xml(&skill.allowed_tools.join(", "))
            ));
        }
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path)
        ));
        lines.push("  </skill>".into());
    }
    lines.push("</available_skills>".into());
    lines.join("\n")
}

/// Truncate to `max` UTF-16 code units, ending in `…` (`slice(0, max - 1).trimEnd() + "…"`).
fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    if units.len() <= max {
        return s.to_string();
    }
    let head = String::from_utf16_lossy(&units[..max - 1]);
    format!("{}…", head.trim_end())
}

/// `summarizeAgentDescription`: one positive "when to use" line.
pub fn summarize_agent_description(description: &str) -> String {
    let lines: Vec<&str> = description
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let is_stop = |line: &str| {
        let lower = line.to_lowercase();
        let starts_word = |prefix: &str| {
            lower.strip_prefix(prefix).is_some_and(|rest| {
                !rest
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        };
        // /^(do\s*not|don'?t|avoid)\b/i
        let do_not = lower.strip_prefix("do").is_some_and(|rest| {
            let rest = rest.trim_start();
            rest.strip_prefix("not").is_some_and(|after| {
                !after
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        });
        do_not || starts_word("don't") || starts_word("dont") || starts_word("avoid")
    };
    let region: &[&str] = match lines.iter().position(|l| is_stop(l)) {
        Some(stop) => &lines[..stop],
        None => &lines,
    };
    let body: &[&str] = if region.len() > 1 && region[0].ends_with(':') {
        &region[1..]
    } else {
        region
    };
    let is_bullet = |line: &str| {
        let mut chars = line.chars();
        matches!(chars.next(), Some('-' | '*' | '•'))
            && chars.next().is_some_and(char::is_whitespace)
    };
    let strip_bullet = |line: &str| {
        let mut chars = line.chars();
        chars.next();
        chars.as_str().trim().to_string()
    };
    let bullets: Vec<String> = body
        .iter()
        .filter(|l| is_bullet(l))
        .map(|l| strip_bullet(l))
        .filter(|l| !l.is_empty())
        .collect();
    let summary = if !bullets.is_empty() {
        bullets.into_iter().take(3).collect::<Vec<_>>().join("; ")
    } else {
        let first = body.first().or(lines.first()).copied().unwrap_or("");
        first.strip_suffix(':').unwrap_or(first).to_string()
    };
    truncate_with_ellipsis(&summary, 200)
}

/// `formatAgentsForPrompt`: agents available through the Task tool, or `""`.
pub fn format_agents_for_prompt(agents: &[PromptAgent]) -> String {
    if agents.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "\n\nThe following specialized agents are available for delegation via the Task tool."
            .to_string(),
        "Choose the agent whose description best matches the task and pass it as `subagent_type`."
            .to_string(),
        String::new(),
        "<available_agents>".to_string(),
    ];
    for agent in agents {
        lines.push("  <agent>".into());
        lines.push(format!("    <name>{}</name>", escape_xml(&agent.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&summarize_agent_description(&agent.description))
        ));
        if !agent.tools.is_empty() {
            lines.push(format!(
                "    <tools>{}</tools>",
                escape_xml(&agent.tools.join(", "))
            ));
        }
        if let Some(model) = &agent.model {
            lines.push(format!("    <model>{}</model>", escape_xml(model)));
        }
        lines.push("  </agent>".into());
    }
    lines.push("</available_agents>".into());
    lines.join("\n")
}

/// `listSelfDocs`: the `.md` files in `docs_root` (`index.md` first, then
/// alphabetical), then the README and CHANGELOG when they exist. Empty when
/// the docs directory is absent.
pub fn list_self_docs(docs_root: &Path, readme: &Path, changelog: &Path) -> Vec<SelfDoc> {
    let mut docs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(docs_root) {
        let mut files: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|f| f.ends_with(".md"))
            .collect();
        files.sort_by(|a, b| match (a == "index.md", b == "index.md") {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => locale_compare(a, b),
        });
        for file in files {
            let path = docs_root.join(&file);
            if path.is_file() {
                docs.push(SelfDoc { id: file, path });
            }
        }
    }
    for extra in [readme, changelog] {
        if extra.exists() {
            docs.push(SelfDoc {
                id: extra
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: extra.to_path_buf(),
            });
        }
    }
    docs
}

/// An approximation of `String.prototype.localeCompare` for doc filenames:
/// case-insensitive first, lowercase before uppercase on a tie.
fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| b.cmp(a))
}

/// `formatSelfDocsForPrompt`: the docs section, or `""` without docs.
pub fn format_self_docs_for_prompt(docs: &[SelfDoc]) -> String {
    if docs.is_empty() {
        return String::new();
    }
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for doc in docs {
        let root = doc
            .path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        match groups.iter_mut().find(|(r, _)| *r == root) {
            Some((_, files)) => files.push(doc.id.clone()),
            None => groups.push((root, vec![doc.id.clone()])),
        }
    }
    let sections: Vec<String> = groups
        .iter()
        .map(|(root, files)| format!("{root}/: {}", files.join(", ")))
        .collect();
    format!(
        "\n\n# About {APP_NAME} itself\n\nYou are running inside {APP_NAME}. Its own docs ship with the install, listed below; {APP_NAME} is actively developed, so answer questions about it from these files rather than from memory. They sit outside the working directory, so searching the project will not find them. Use {SELF_SEARCH_TOOL_NAME} to locate a specific heading, or read a file directly.\n\n{}",
        sections.join("\n")
    )
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn context_files_section(context_files: &[ContextFile]) -> String {
    if context_files.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n# Project Context\n\n");
    out.push_str("Project-specific instructions and guidelines:\n\n");
    for file in context_files {
        out.push_str(&format!("## {}\n\n{}\n\n", file.path, file.content));
    }
    out
}

/// `buildSystemPrompt`: the system prompt with tools, guidelines, and context.
pub fn build_system_prompt(options: &BuildSystemPromptOptions) -> String {
    let prompt_cwd = options.cwd.replace('\\', "/");
    let date = options.date.clone().unwrap_or_else(today);
    let append_section = options
        .append_system_prompt
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| format!("\n\n{s}"))
        .unwrap_or_default();
    let custom_prompt = options.custom_prompt.as_deref().filter(|s| !s.is_empty());
    let want_self_docs = options.include_self_docs.unwrap_or(custom_prompt.is_none());

    if let Some(custom) = custom_prompt {
        let selected = options.selected_tools.as_ref();
        let has = |name: &str| selected.is_none_or(|tools| tools.iter().any(|t| t == name));
        let mut prompt = custom.to_string();
        prompt.push_str(&append_section);
        prompt.push_str(&context_files_section(&options.context_files));
        let has_read = has("read");
        if has_read {
            prompt.push_str(&format_skills_for_prompt(&options.skills));
        }
        if has(TASK_TOOL_NAME) {
            prompt.push_str(&format_agents_for_prompt(&options.agents));
        }
        if want_self_docs && has_read {
            prompt.push_str(&format_self_docs_for_prompt(&options.self_docs));
        }
        prompt.push_str(&format!("\n\nCurrent date: {date}"));
        prompt.push_str(&format!("\nCurrent working directory: {prompt_cwd}"));
        return prompt;
    }

    let default_tools: Vec<String> = ["read", "bash", "edit", "write", "SearchCodebase"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let tools = options.selected_tools.as_ref().unwrap_or(&default_tools);
    let snippet = |name: &str| {
        options
            .tool_snippets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s.as_str())
            .filter(|s| !s.is_empty())
    };
    let visible: Vec<String> = tools
        .iter()
        .filter_map(|name| snippet(name).map(|s| format!("- {name}: {s}")))
        .collect();
    let tools_list = if visible.is_empty() {
        "(none)".to_string()
    } else {
        visible.join("\n")
    };

    let mut guidelines: Vec<String> = Vec::new();
    let mut add = |g: &str| {
        if !guidelines.iter().any(|x| x == g) {
            guidelines.push(g.to_string());
        }
    };
    let has = |name: &str| tools.iter().any(|t| t == name);
    let has_bash = has("bash");
    let has_search = has("SearchCodebase");
    let has_read = has("read");

    if has_search {
        add(if has_bash {
            "SearchCodebase finds where code lives by concept, behavior, or half-known name (ranked, respects .gitignore); shell out to rg/find/ls only for exact matching lines, counts, or a raw listing"
        } else {
            "For code discovery use SearchCodebase — it finds where code lives by concept, behavior, or half-known name, and respects .gitignore"
        });
    } else if has_bash {
        add("Use bash for file exploration (ls, rg/grep, find)");
    }
    for guideline in &options.prompt_guidelines {
        let normalized = guideline.trim();
        if !normalized.is_empty() {
            add(normalized);
        }
    }
    add("Put independent tool calls in one message — they execute in parallel; only split them across turns when a call needs an earlier call's result");
    add("Be concise: no preamble or postamble, no restating the task or summarizing what you just did, no closers like \"Let me know\"");
    add("Do not narrate routine tool calls or results — the permission gate already shows them; speak when you have the answer or need a decision");
    add("Match the surrounding code's conventions for comments, docstrings, and types — do not add or strip them by default");
    add("Cite path:line when referring to code");
    add(
        "If a command failed, a test is still red, or you did not verify something, say so plainly",
    );

    let guidelines = guidelines
        .iter()
        .map(|g| format!("- {g}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut prompt = format!(
        "You are an expert coding assistant operating inside {APP_NAME}, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.\n\nAvailable tools:\n{tools_list}\n\nGuidelines:\n{guidelines}"
    );
    prompt.push_str(&append_section);
    prompt.push_str(&context_files_section(&options.context_files));
    if has_read {
        prompt.push_str(&format_skills_for_prompt(&options.skills));
    }
    if has(TASK_TOOL_NAME) {
        prompt.push_str(&format_agents_for_prompt(&options.agents));
    }
    if want_self_docs && has_read {
        prompt.push_str(&format_self_docs_for_prompt(&options.self_docs));
    }
    prompt.push_str(&format!("\n\nCurrent date: {date}"));
    prompt.push_str(&format!("\nCurrent working directory: {prompt_cwd}"));
    prompt
}

#[cfg(test)]
mod tests {
    //! Port of `test/system-prompt.test.ts`, plus the appended sections.
    use super::*;

    fn opts(selected: Option<&[&str]>) -> BuildSystemPromptOptions {
        BuildSystemPromptOptions {
            selected_tools: selected.map(|s| s.iter().map(|t| t.to_string()).collect()),
            cwd: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn shows_none_for_empty_tools_list() {
        assert!(build_system_prompt(&opts(Some(&[]))).contains("Available tools:\n(none)"));
    }

    #[test]
    fn shows_the_code_citation_guideline_even_with_no_tools() {
        assert!(
            build_system_prompt(&opts(Some(&[]))).contains("Cite path:line when referring to code")
        );
    }

    #[test]
    fn separates_the_date_cwd_block_from_the_guidelines_list() {
        let prompt = build_system_prompt(&opts(Some(&[])));
        assert!(prompt.contains("\n\nCurrent date: "));
        let i = prompt.find("Current date: ").unwrap();
        assert_eq!(&prompt[i - 2..i], "\n\n");
    }

    #[test]
    fn includes_the_default_output_constraint_guidelines() {
        let prompt = build_system_prompt(&opts(Some(&[])));
        assert!(prompt.contains("no restating the task or summarizing what you just did"));
        assert!(prompt.contains("\"Let me know\""));
        assert!(!prompt.contains("Be concise in your responses"));
        assert!(prompt.contains("Do not narrate routine tool calls or results"));
        assert!(prompt.contains("Match the surrounding code's conventions"));
        assert!(prompt.contains("say so plainly"));
    }

    #[test]
    fn includes_all_default_tools_when_snippets_are_provided() {
        let mut o = opts(None);
        o.tool_snippets = [
            ("read", "Read file contents"),
            ("bash", "Execute bash commands"),
            ("edit", "Make surgical edits"),
            ("write", "Create or overwrite files"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let prompt = build_system_prompt(&o);
        for t in ["- read:", "- bash:", "- edit:", "- write:"] {
            assert!(prompt.contains(t), "{t}");
        }
    }

    #[test]
    fn includes_custom_tools_in_available_tools_when_prompt_snippet_is_provided() {
        let mut o = opts(Some(&["read", "dynamic_tool"]));
        o.tool_snippets = vec![("dynamic_tool".into(), "Run dynamic test behavior".into())];
        assert!(build_system_prompt(&o).contains("- dynamic_tool: Run dynamic test behavior"));
    }

    #[test]
    fn omits_custom_tools_from_available_tools_without_prompt_snippet() {
        assert!(
            !build_system_prompt(&opts(Some(&["read", "dynamic_tool"]))).contains("dynamic_tool")
        );
    }

    #[test]
    fn appends_prompt_guidelines_to_default_guidelines() {
        let mut o = opts(Some(&["read", "dynamic_tool"]));
        o.prompt_guidelines = vec!["Use dynamic_tool for project summaries.".into()];
        assert!(build_system_prompt(&o).contains("- Use dynamic_tool for project summaries."));
    }

    #[test]
    fn deduplicates_and_trims_prompt_guidelines() {
        let mut o = opts(Some(&["read", "dynamic_tool"]));
        o.prompt_guidelines = vec![
            "Use dynamic_tool for summaries.".into(),
            "  Use dynamic_tool for summaries.  ".into(),
            "   ".into(),
        ];
        assert_eq!(
            build_system_prompt(&o)
                .matches("- Use dynamic_tool for summaries.")
                .count(),
            1
        );
    }

    #[test]
    fn emits_the_routing_guideline_only_when_both_search_and_bash_are_active() {
        let both = build_system_prompt(&opts(Some(&["SearchCodebase", "bash"])));
        assert!(both.contains("shell out to rg/find/ls only for"));
        let search_only = build_system_prompt(&opts(Some(&["SearchCodebase"])));
        assert!(search_only.contains("For code discovery use SearchCodebase"));
        assert!(!search_only.contains("shell out to rg/find/ls only for"));
    }

    #[test]
    fn falls_back_to_the_shell_guideline_when_search_is_absent() {
        let bash_only = build_system_prompt(&opts(Some(&["bash"])));
        assert!(bash_only.contains("Use bash for file exploration"));
        assert!(!bash_only.contains("SearchCodebase"));
    }

    #[test]
    fn exact_layout_for_read_only() {
        let mut o = opts(Some(&["read"]));
        o.tool_snippets = vec![("read".into(), "Read file contents".into())];
        o.cwd = "C:\\w\\p".into();
        o.date = Some("2026-01-02".into());
        assert_eq!(
            build_system_prompt(&o),
            "You are an expert coding assistant operating inside cortex, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.\n\n\
Available tools:\n- read: Read file contents\n\n\
Guidelines:\n\
- Put independent tool calls in one message — they execute in parallel; only split them across turns when a call needs an earlier call's result\n\
- Be concise: no preamble or postamble, no restating the task or summarizing what you just did, no closers like \"Let me know\"\n\
- Do not narrate routine tool calls or results — the permission gate already shows them; speak when you have the answer or need a decision\n\
- Match the surrounding code's conventions for comments, docstrings, and types — do not add or strip them by default\n\
- Cite path:line when referring to code\n\
- If a command failed, a test is still red, or you did not verify something, say so plainly\n\n\
Current date: 2026-01-02\nCurrent working directory: C:/w/p"
        );
    }

    #[test]
    fn custom_prompt_replaces_the_default_and_skips_self_docs() {
        let mut o = opts(Some(&["read"]));
        o.custom_prompt = Some("Be terse.".into());
        o.append_system_prompt = Some("Extra.".into());
        o.date = Some("2026-01-02".into());
        o.cwd = "/w".into();
        o.self_docs = vec![SelfDoc {
            id: "index.md".into(),
            path: "/d/docs/index.md".into(),
        }];
        o.context_files = vec![ContextFile {
            path: "/w/AGENTS.md".into(),
            content: "rules".into(),
        }];
        assert_eq!(
            build_system_prompt(&o),
            "Be terse.\n\nExtra.\n\n# Project Context\n\nProject-specific instructions and guidelines:\n\n## /w/AGENTS.md\n\nrules\n\n\n\nCurrent date: 2026-01-02\nCurrent working directory: /w"
        );
    }

    #[test]
    fn skills_agents_and_self_docs_sections() {
        let mut o = opts(Some(&["read", "Task"]));
        o.date = Some("2026-01-02".into());
        o.skills = vec![
            PromptSkill {
                name: "a&b".into(),
                description: "Use <this>".into(),
                file_path: "/s/SKILL.md".into(),
                allowed_tools: vec!["read".into(), "write".into()],
                disable_model_invocation: false,
            },
            PromptSkill {
                name: "hidden".into(),
                disable_model_invocation: true,
                ..Default::default()
            },
        ];
        o.agents = vec![PromptAgent {
            name: "explore".into(),
            description:
                "Use this subagent ONLY when:\n- Reading code\n- Scouting\nDO NOT use when editing"
                    .into(),
            tools: vec!["read".into()],
            model: Some("fast".into()),
        }];
        o.self_docs = vec![
            SelfDoc {
                id: "index.md".into(),
                path: "/d/docs/index.md".into(),
            },
            SelfDoc {
                id: "skills.md".into(),
                path: "/d/docs/skills.md".into(),
            },
            SelfDoc {
                id: "README.md".into(),
                path: "/d/README.md".into(),
            },
        ];
        let prompt = build_system_prompt(&o);
        assert!(prompt.contains(
            "\n\nThe following skills provide specialized instructions for specific tasks.\nUse the read tool to load a skill's file when the task matches its description.\n"
        ));
        assert!(prompt.contains(
            "  <skill>\n    <name>a&amp;b</name>\n    <description>Use &lt;this&gt;</description>\n    <tools>read, write</tools>\n    <location>/s/SKILL.md</location>\n  </skill>\n</available_skills>"
        ));
        assert!(!prompt.contains("hidden"));
        assert!(prompt.contains(
            "  <agent>\n    <name>explore</name>\n    <description>Reading code; Scouting</description>\n    <tools>read</tools>\n    <model>fast</model>\n  </agent>\n</available_agents>"
        ));
        assert!(prompt.contains("\n\n# About cortex itself\n\nYou are running inside cortex."));
        assert!(prompt.contains(
            "\n\n/d/docs/: index.md, skills.md\n/d/: README.md\n\nCurrent date: 2026-01-02"
        ));
    }

    #[test]
    fn agents_need_the_task_tool() {
        let mut o = opts(Some(&["read"]));
        o.agents = vec![PromptAgent {
            name: "explore".into(),
            description: "Scout".into(),
            ..Default::default()
        }];
        assert!(!build_system_prompt(&o).contains("available_agents"));
    }

    #[test]
    fn summarize_agent_description_cases() {
        assert_eq!(summarize_agent_description(""), "");
        assert_eq!(
            summarize_agent_description("Just one line:"),
            "Just one line"
        );
        assert_eq!(
            summarize_agent_description("Header:\nFirst body line\nDon't use for X"),
            "First body line"
        );
        assert_eq!(summarize_agent_description("- a\n- b\n- c\n- d"), "a; b; c");
        let long = format!("- {}", "x".repeat(300));
        let s = summarize_agent_description(&long);
        assert_eq!(s.encode_utf16().count(), 200);
        assert!(s.ends_with('…'));
        // "Avoidance" is not the word "avoid".
        assert_eq!(
            summarize_agent_description("- a\nAvoidance tips\n- b"),
            "a; b"
        );
    }

    #[test]
    fn list_self_docs_orders_index_first_then_extras() {
        let dir = std::env::temp_dir().join(format!("cortex-self-docs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let docs = dir.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        for f in ["zeta.md", "Alpha.md", "index.md", "beta.md", "notes.txt"] {
            std::fs::write(docs.join(f), "# x").unwrap();
        }
        std::fs::write(dir.join("README.md"), "r").unwrap();
        let listed = list_self_docs(&docs, &dir.join("README.md"), &dir.join("CHANGELOG.md"));
        let ids: Vec<&str> = listed.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(
            ids,
            ["index.md", "Alpha.md", "beta.md", "zeta.md", "README.md"]
        );
        assert!(
            list_self_docs(&dir.join("missing"), &dir.join("nope"), &dir.join("nope")).is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
