//! The mode system as the interactive mode drives it (ledger 10.5e): the
//! `session_start` re-resolve a reload runs, `/mode`'s argument completions
//! (alt+a steps through them), the footer badge (`ctx.ui.setMode`) and the
//! command actions handed to the UI.

use cortexcode_code_agent_session::{
    ExtensionHooks, ExtensionUiRequest, NotifyLevel, SessionEvent, SessionStartEvent,
    SessionStartReason,
};
use cortexcode_code_modes::config::HooConfig;
use cortexcode_code_modes::{ModeAction, ModeSession, ModesExtension};

fn session(cwd: &std::path::Path, light: bool) -> ModeSession {
    ModeSession {
        cwd: cwd.to_path_buf(),
        session_id: "s1".into(),
        light,
        mode_search_paths: vec![],
    }
}

fn write_project_config(cwd: &std::path::Path, json: &str) {
    let dir = cwd.join(cortexcode_code_paths::CONFIG_DIR_NAME);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hoo-config.json"), json).unwrap();
}

fn reload_event() -> SessionEvent {
    SessionEvent::Start(SessionStartEvent {
        reason: SessionStartReason::Reload,
        previous_session_file: None,
    })
}

#[test]
fn session_start_re_resolves_the_mode_and_returns_its_tool_filter() {
    let dir = tempfile::tempdir().unwrap();
    let ext = ModesExtension::with_config(session(dir.path(), false), &HooConfig::new());
    assert_eq!(ext.active().mode, "build");
    assert!(ext.has_handlers("session_start"));

    write_project_config(
        dir.path(),
        r#"{"active_mode":"plan","modes":{"plan":{"enabled_tools":["read","grep"]}}}"#,
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let result = rt.block_on(ext.emit_session_event(reload_event()));
    assert_eq!(
        result.active_tools,
        Some(vec!["read".to_string(), "grep".to_string()])
    );
    assert_eq!(ext.active().mode, "plan");
    assert_eq!(ext.active_mode().as_deref(), Some("plan"));
    let prompt = ext.before_agent_start("hi", "BASE").unwrap();
    assert!(prompt.contains("<!-- hoo-core: mode=plan -->"));
}

#[test]
fn mode_completions_are_the_known_modes_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let ext = ModesExtension::with_config(session(dir.path(), false), &HooConfig::new());
    assert_eq!(
        ext.argument_completions("mode", "").unwrap(),
        ["ask", "plan", "build", "debug"]
    );
    assert_eq!(ext.argument_completions("mode", "d").unwrap(), ["debug"]);
    assert_eq!(
        ext.argument_completions("grill", "").unwrap(),
        ["me", "plan"]
    );
    assert_eq!(
        ext.argument_completions("plan", "").unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(ext.argument_completions("unknown", ""), None);
}

#[test]
fn commands_are_listed_as_a_temporary_extension() {
    let dir = tempfile::tempdir().unwrap();
    let ext = ModesExtension::with_config(session(dir.path(), false), &HooConfig::new());
    let commands = ext.commands();
    let names: Vec<_> = commands.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["mode", "plan", "grill", "goal", "approve"]);
    assert_eq!(
        commands[0].description.as_deref(),
        Some("Switch active mode. Usage: /mode <ask|plan|build|debug>")
    );
    assert_eq!(commands[0].source_info["scope"], "temporary");
}

#[test]
fn light_mode_sets_no_badge() {
    let dir = tempfile::tempdir().unwrap();
    let ext = ModesExtension::with_config(session(dir.path(), true), &HooConfig::new());
    assert_eq!(ext.active_mode(), None);
}

#[test]
fn command_actions_become_ui_requests_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let ext = ModesExtension::with_config(session(dir.path(), false), &HooConfig::new());
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(ext.run_command("mode", "")).unwrap();
    assert_eq!(
        ext.take_ui_requests(),
        [ExtensionUiRequest::Notify(
            "Active mode: build".into(),
            NotifyLevel::Info
        )]
    );
    assert!(ext.take_ui_requests().is_empty());
    // LOOP_AUTO_START has no listener until the loop extension (12.5).
    assert!(matches!(
        ext.command("goal", "ship it")[0],
        ModeAction::StartAutoLoop { .. }
    ));
    rt.block_on(ext.run_command("goal", "ship it")).unwrap();
    assert!(ext.take_ui_requests().is_empty());
}
