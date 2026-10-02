//! Port of hoocode `packages/coding-agent/test/agent-registry.test.ts` (v0.5.89).

use cortexcode_code_resources::{
    format_agents_for_prompt, load_agent_registry, AgentDefinition, AgentRegistry, AgentSource,
    DiagnosticType, LoadAgentRegistryOptions,
};

fn def(name: &str, description: &str, prompt: &str, source: AgentSource) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
        description: description.into(),
        tools: None,
        disallowed_tools: None,
        model: None,
        prompt: prompt.into(),
        source,
        file_path: None,
        max_turns: None,
        background: None,
        delegate: None,
        delegate_to: None,
        fork: None,
    }
}

fn tmp() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    (dir, path)
}

fn write_agent(dir: &str, name: &str, body: &str, extra: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        format!("{dir}/{name}.md"),
        format!("---\nname: {name}\ndescription: {name} agent for testing purposes.\n{extra}---\n{body}"),
    )
    .unwrap();
}

fn options(cwd: &str, agent_dir: &str, builtins: bool, claude: bool) -> LoadAgentRegistryOptions {
    LoadAgentRegistryOptions {
        cwd: cwd.into(),
        agent_dir: Some(agent_dir.into()),
        include_builtins: builtins,
        include_claude: claude,
        agent_paths: Vec::new(),
    }
}

const CONFIG: &str = cortexcode_code_paths::CONFIG_DIR_NAME;

// AgentRegistry

#[test]
fn register_get_has_list() {
    let mut reg = AgentRegistry::new();
    let d = def("custom", "x", "p", AgentSource::Project);
    reg.register(d.clone());
    assert!(reg.has("custom"));
    assert_eq!(reg.get("custom"), Some(&d));
    assert_eq!(reg.list().len(), 1);
}

#[test]
fn re_registering_the_same_name_records_a_collision_and_the_later_wins() {
    let mut reg = AgentRegistry::new();
    reg.register(def("dup", "a", "1", AgentSource::User));
    reg.register(def("dup", "b", "2", AgentSource::Project));
    assert_eq!(reg.get("dup").unwrap().prompt, "2");
    assert_eq!(reg.get("dup").unwrap().source, AgentSource::Project);
    assert!(reg
        .diagnostics()
        .iter()
        .any(|d| d.kind == DiagnosticType::Collision));
    assert_eq!(
        reg.diagnostics()[0].message,
        "agent \"dup\" from project overrides user"
    );
}

// loadAgentRegistry

#[test]
fn loads_embedded_built_in_agents_by_default() {
    let ((_a, cwd), (_b, agent_dir)) = (tmp(), tmp());
    let reg = load_agent_registry(&options(&cwd, &agent_dir, true, false));
    assert!(reg.has("explore"));
    assert!(reg.has("general-purpose"));
    assert!(reg.list().len() >= 2);
}

#[test]
fn project_agents_override_user_agents_and_built_ins() {
    let ((_a, cwd), (_b, agent_dir)) = (tmp(), tmp());
    write_agent(
        &format!("{agent_dir}/agents"),
        "explore",
        "USER explore prompt",
        "",
    );
    write_agent(
        &format!("{cwd}/{CONFIG}/agents"),
        "explore",
        "PROJECT explore prompt",
        "",
    );
    let reg = load_agent_registry(&options(&cwd, &agent_dir, true, false));
    let explore = reg.get("explore").unwrap();
    assert_eq!(explore.source, AgentSource::Project);
    assert_eq!(explore.prompt, "PROJECT explore prompt");
}

#[test]
fn imports_claude_agents_natively() {
    let ((_a, cwd), (_b, agent_dir)) = (tmp(), tmp());
    write_agent(
        &format!("{cwd}/.claude/agents"),
        "claude-helper",
        "From .claude",
        "tools: Read, Glob\n",
    );
    let reg = load_agent_registry(&options(&cwd, &agent_dir, false, true));
    let helper = reg.get("claude-helper").unwrap();
    assert_eq!(helper.source, AgentSource::ClaudeProject);
    assert_eq!(helper.tools.as_deref().unwrap(), ["read", "SearchCodebase"]);
}

#[test]
fn native_project_agents_take_precedence_over_imported_claude_project_agents() {
    let ((_a, cwd), (_b, agent_dir)) = (tmp(), tmp());
    write_agent(
        &format!("{cwd}/.claude/agents"),
        "shared",
        "CLAUDE version",
        "",
    );
    write_agent(
        &format!("{cwd}/{CONFIG}/agents"),
        "shared",
        "HOOCODE version",
        "",
    );
    let reg = load_agent_registry(&options(&cwd, &agent_dir, false, true));
    assert_eq!(reg.get("shared").unwrap().source, AgentSource::Project);
    assert_eq!(reg.get("shared").unwrap().prompt, "HOOCODE version");
}

#[test]
fn explicit_agent_paths_load_files_and_directories() {
    let ((_a, cwd), (_b, agent_dir), (_c, extra), (_d, file_dir)) = (tmp(), tmp(), tmp(), tmp());
    write_agent(&extra, "from-dir", "dir agent", "");
    write_agent(&file_dir, "from-file", "file agent", "");
    let mut opts = options(&cwd, &agent_dir, false, false);
    opts.agent_paths = vec![extra.clone(), format!("{file_dir}/from-file.md")];
    let reg = load_agent_registry(&opts);
    assert_eq!(reg.get("from-dir").unwrap().prompt, "dir agent");
    assert_eq!(reg.get("from-file").unwrap().prompt, "file agent");
}

#[test]
fn explicit_agent_paths_override_discovered_project_agents_by_name() {
    let ((_a, cwd), (_b, agent_dir), (_c, override_dir)) = (tmp(), tmp(), tmp());
    write_agent(
        &format!("{cwd}/{CONFIG}/agents"),
        "shared",
        "PROJECT version",
        "",
    );
    write_agent(&override_dir, "shared", "OVERRIDE version", "");
    let mut opts = options(&cwd, &agent_dir, false, false);
    opts.agent_paths = vec![override_dir.clone()];
    let reg = load_agent_registry(&opts);
    assert_eq!(reg.get("shared").unwrap().prompt, "OVERRIDE version");
}

#[test]
fn missing_agent_paths_surface_a_diagnostic_instead_of_throwing() {
    let ((_a, cwd), (_b, agent_dir), (_c, base)) = (tmp(), tmp(), tmp());
    let missing = format!("{base}/does-not-exist");
    let mut opts = options(&cwd, &agent_dir, false, false);
    opts.agent_paths = vec![missing.clone()];
    let reg = load_agent_registry(&opts);
    assert!(reg.list().is_empty());
    assert!(reg
        .diagnostics()
        .iter()
        .any(|d| d.kind == DiagnosticType::Warning && d.path.as_deref() == Some(missing.as_str())));
}

#[test]
fn ignores_subdirectories_and_non_md_files() {
    let ((_a, cwd), (_b, agent_dir)) = (tmp(), tmp());
    let project = format!("{cwd}/{CONFIG}/agents");
    write_agent(&project, "real", "real agent", "");
    std::fs::create_dir_all(format!("{project}/dispatch-123")).unwrap();
    std::fs::write(format!("{project}/notes.txt"), "ignore me").unwrap();
    let reg = load_agent_registry(&options(&cwd, &agent_dir, false, false));
    assert_eq!(reg.list().len(), 1);
    assert!(reg.has("real"));
}

// Beyond the TS file: the built-in roster renders exactly as hoocode's system
// prompt shows it (recorded from hoocode's model request in the print-tool-bash
// scenario).

#[test]
fn builtin_agents_render_the_hoocode_available_agents_block() {
    let ((_a, cwd), (_b, agent_dir)) = (tmp(), tmp());
    let reg = load_agent_registry(&options(&cwd, &agent_dir, true, false));
    assert!(reg.diagnostics().is_empty(), "{:?}", reg.diagnostics());
    let expected = "\n\nThe following specialized agents are available for delegation via the Task tool.\nChoose the agent whose description best matches the task and pass it as `subagent_type`.\n\n<available_agents>\n  <agent>\n    <name>code-review</name>\n    <description>Reviewing a diff, branch, or files for defects before merge; The user asks for a review, a second pass, or what they missed; Findings are wanted without spending parent context on every changed file</description>\n    <tools>read, bash, SearchCodebase</tools>\n    <model>capable</model>\n  </agent>\n  <agent>\n    <name>explore</name>\n    <description>Reading or understanding code without changes; Scouting a codebase for plans or maps; Analyzing dependencies, imports, project structure</description>\n    <tools>read, SearchCodebase</tools>\n    <model>fast</model>\n  </agent>\n  <agent>\n    <name>general-purpose</name>\n    <description>A task is multi-step or open-ended and needs both investigation and action; No other agent clearly fits; You are searching for code or a pattern and are unsure where it lives</description>\n    <tools>read, bash, edit, write, SearchCodebase</tools>\n    <model>standard</model>\n  </agent>\n  <agent>\n    <name>plan</name>\n    <description>You need to research a codebase before proposing an implementation plan; A task requires understanding scope, affected files, and approach without; You are in plan mode and want focused investigation…</description>\n    <tools>read, SearchCodebase</tools>\n    <model>standard</model>\n  </agent>\n  <agent>\n    <name>security-review</name>\n    <description>Reviewing changes for vulnerabilities before merge or release; The change touches auth, input parsing, paths, subprocesses, or secrets; The user asks for a security review or audit of pending work</description>\n    <tools>read, bash, SearchCodebase</tools>\n    <model>capable</model>\n  </agent>\n</available_agents>";
    assert_eq!(format_agents_for_prompt(reg.list()), expected);
}
