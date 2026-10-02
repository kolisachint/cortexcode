//! Port of the platform-targets part of `platform.test.ts` (token
//! normalization, plugin and workspace resolution). The CLI parsing, plugin
//! authoring and scaffold parts go with the tasks that own those modules.

use std::sync::{Mutex, MutexGuard};

use cortexcode_code_settings::platform_targets::{
    get_workspace_platforms, normalize_platform_token, parse_platforms, resolve_plugin_platforms,
    set_platforms, DEFAULT_PLUGIN_PLATFORMS,
};
use cortexcode_code_settings::MarketplacePlatform::{self, Agents, Claude, Github};

/// The targets are process-wide: one test at a time, each starting unset.
fn fresh() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_platforms(None);
    guard
}

fn resolve(explicit: Option<&[MarketplacePlatform]>) -> Vec<MarketplacePlatform> {
    resolve_plugin_platforms(explicit).unwrap()
}

#[test]
fn folds_every_documented_alias() {
    assert_eq!(normalize_platform_token("copilot"), Some(Github));
    assert_eq!(normalize_platform_token("GH"), Some(Github));
    assert_eq!(normalize_platform_token("github"), Some(Github));
    assert_eq!(normalize_platform_token("Claude"), Some(Claude));
    assert_eq!(normalize_platform_token("native"), Some(Agents));
    assert_eq!(normalize_platform_token("agents"), Some(Agents));
    assert_eq!(normalize_platform_token("vscode"), None);
}

#[test]
fn parse_platforms_dedupes_and_reports_invalid_tokens() {
    let parsed = parse_platforms(&["copilot", "github", "bogus", "claude"]);
    assert_eq!(parsed.platforms, [Github, Claude]);
    assert_eq!(parsed.invalid, ["bogus"]);
}

#[test]
fn defaults_to_claude_when_nothing_is_configured() {
    let _g = fresh();
    assert_eq!(resolve(None), DEFAULT_PLUGIN_PLATFORMS);
    assert_eq!(resolve(None), [Claude]);
}

#[test]
fn session_targets_replace_the_default_and_an_explicit_set_still_wins() {
    let _g = fresh();
    set_platforms(Some(&[Github]));
    assert_eq!(resolve(None), [Github]);
    assert_eq!(resolve(Some(&[Claude])), [Claude]);
}

#[test]
fn errors_on_an_explicit_agents_rather_than_silently_dropping_it() {
    let _g = fresh();
    let err = resolve_plugin_platforms(Some(&[Agents])).unwrap_err();
    assert!(err.contains("belongs to no marketplace"), "{err}");
    let err = resolve_plugin_platforms(Some(&[Claude, Agents])).unwrap_err();
    assert!(err.contains("agents"), "{err}");
}

#[test]
fn filters_a_session_level_agents_instead_of_erroring() {
    let _g = fresh();
    set_platforms(Some(&[Agents]));
    assert_eq!(resolve(None), [Claude]);

    set_platforms(Some(&[Agents, Github]));
    assert_eq!(resolve(None), [Github]);
}

#[test]
fn clears_back_to_the_default() {
    let _g = fresh();
    set_platforms(Some(&[Github]));
    set_platforms(None);
    assert_eq!(resolve(None), DEFAULT_PLUGIN_PLATFORMS);
}

#[test]
fn workspace_targets_are_unset_until_configured() {
    let _g = fresh();
    assert_eq!(get_workspace_platforms(), None);
}

#[test]
fn workspace_targets_keep_agents() {
    // A scaffolded artifact is consumed in place, not distributed.
    let _g = fresh();
    set_platforms(Some(&[Agents]));
    assert_eq!(get_workspace_platforms(), Some(vec![Agents]));
}
