//! `handleConfigCommand` (package-manager-cli.ts): `cortex config` opens the
//! resource list over everything the settings resolve to.
//!
//! Package sources (`packages`: npm / git) are not resolved yet (ledger
//! 12.2), so only settings entries and auto-discovered resources are listed.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use cortexcode_code_resources::package_resolve::{
    home_dir, resolve_local_resources, ResolveOptions,
};
use cortexcode_code_settings::SettingsManager;

/// `reportSettingsErrors`.
fn report_settings_errors(
    settings: &mut SettingsManager,
    context: &str,
    color: bool,
    err: &mut dyn Write,
) {
    for e in settings.drain_errors() {
        let line = format!(
            "Warning ({context}, {} settings): {}",
            e.scope.as_str(),
            e.error
        );
        let line = if color {
            format!("\x1b[33m{line}\x1b[39m")
        } else {
            line
        };
        let _ = writeln!(err, "{line}");
    }
}

/// Run `cortex config`. Returns the exit code (0 on close and on ctrl+c).
pub fn run_config_command(color: bool, err: &mut dyn Write) -> i32 {
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .into_owned();
    let agent_dir = cortexcode_code_paths::agent_dir()
        .to_string_lossy()
        .into_owned();
    let mut settings = SettingsManager::create(&cwd, &agent_dir);
    report_settings_errors(&mut settings, "config command", color, err);
    let resolved = resolve_local_resources(&ResolveOptions {
        cwd: &cwd,
        agent_dir: &agent_dir,
        home: home_dir(),
        global_settings: &settings.global_settings(),
        project_settings: &settings.project_settings(),
    });
    let settings = Rc::new(RefCell::new(settings));
    cortexcode_code_tui_app::session_picker::select_config(&resolved, settings, &cwd, &agent_dir);
    0
}
