//! The `config` subcommand's resource list (`config-selector.ts`, which has
//! no TS tests of its own; the L2 scenario `config-selector` covers the
//! screen). These hold the grouping and what a toggle writes.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_resources::package_resolve::{PathMetadata, ResolvedPaths, ResolvedResource};
use cortexcode_code_resources::{SourceOrigin, SourceScope};
use cortexcode_code_settings::SettingsManager;
use cortexcode_code_tui_selectors::config_selector::{build_groups, ConfigSelectorComponent};
use cortexcode_tui_render::Component;
use serde_json::json;

use crate::support::{lock, strip};

fn resource(path: &str, enabled: bool, metadata: &PathMetadata) -> ResolvedResource {
    ResolvedResource {
        path: path.into(),
        enabled,
        metadata: metadata.clone(),
    }
}

fn project_auto(base: &str) -> PathMetadata {
    PathMetadata::new(
        "auto",
        SourceScope::Project,
        SourceOrigin::TopLevel,
        Some(base),
    )
}

#[test]
fn groups_by_origin_scope_and_source_packages_first_then_user() {
    let project = project_auto("/w/.cortexcode");
    let user = PathMetadata::new("local", SourceScope::User, SourceOrigin::TopLevel, None);
    let package = PathMetadata::new(
        "npm:tools",
        SourceScope::User,
        SourceOrigin::Package,
        Some("/pkg"),
    );
    let resolved = ResolvedPaths {
        prompts: vec![
            resource("/w/.cortexcode/prompts/b.md", true, &project),
            resource("/w/.cortexcode/prompts/A.md", true, &project),
        ],
        skills: vec![resource("/pkg/skills/lint/SKILL.md", true, &package)],
        extensions: vec![
            resource("/home/u/ext/tool/index.ts", false, &user),
            resource("/home/u/extensions/solo.ts", true, &user),
        ],
        ..ResolvedPaths::default()
    };
    let groups = build_groups(&resolved);
    let labels: Vec<&str> = groups.iter().map(|g| g.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "npm:tools (user)",
            "User settings",
            "Project (/w/.cortexcode/)"
        ]
    );
    // A skill shows its folder; an extension outside `extensions/` its parent.
    assert_eq!(groups[0].subgroups[0].items[0].display_name, "lint");
    let names: Vec<&str> = groups[1].subgroups[0]
        .items
        .iter()
        .map(|i| i.display_name.as_str())
        .collect();
    assert_eq!(names, ["solo.ts", "tool/index.ts"]);
    // Items by name, case-insensitively.
    let names: Vec<&str> = groups[2].subgroups[0]
        .items
        .iter()
        .map(|i| i.display_name.as_str())
        .collect();
    assert_eq!(names, ["A.md", "b.md"]);
}

#[test]
fn space_writes_a_pattern_to_the_scope_the_resource_came_from() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let agent = dir.path().join("agent").to_string_lossy().into_owned();
    let base = format!("{cwd}/{}", cortexcode_code_paths::CONFIG_DIR_NAME);
    let resolved = ResolvedPaths {
        prompts: vec![resource(
            &format!("{base}/prompts/review.md"),
            true,
            &project_auto(&base),
        )],
        ..ResolvedPaths::default()
    };
    let settings = Rc::new(RefCell::new(SettingsManager::create(&cwd, &agent)));
    let mut selector =
        ConfigSelectorComponent::new(&resolved, settings.clone(), &cwd, &agent, || {}, || {});

    selector.handle_input(" ");
    assert_eq!(
        settings.borrow().project_settings()["prompts"],
        json!(["-prompts/review.md"])
    );
    assert!(strip(&selector.render(80)).contains("[ ] review.md"));

    // Toggling back replaces the entry rather than adding a second one.
    selector.handle_input(" ");
    assert_eq!(
        settings.borrow().project_settings()["prompts"],
        json!(["+prompts/review.md"])
    );
}

#[test]
fn a_package_filter_is_added_and_dropped_again_when_empty() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let agent = dir.path().join("agent").to_string_lossy().into_owned();
    let mut manager = SettingsManager::create(&cwd, &agent);
    manager.set_packages(&[cortexcode_code_settings::PackageSource::Source(
        "npm:tools".into(),
    )]);
    let settings = Rc::new(RefCell::new(manager));
    let package = PathMetadata::new(
        "npm:tools",
        SourceScope::User,
        SourceOrigin::Package,
        Some("/pkg"),
    );
    let resolved = ResolvedPaths {
        skills: vec![resource("/pkg/skills/lint/SKILL.md", true, &package)],
        ..ResolvedPaths::default()
    };
    let mut selector =
        ConfigSelectorComponent::new(&resolved, settings.clone(), &cwd, &agent, || {}, || {});

    selector.handle_input(" ");
    assert_eq!(
        settings.borrow().global_settings()["packages"],
        json!([{"source": "npm:tools", "skills": ["-skills/lint/SKILL.md"]}])
    );
}

#[test]
fn escape_closes_and_typing_filters() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let base = format!("{cwd}/.cortexcode");
    let meta = project_auto(&base);
    let resolved = ResolvedPaths {
        prompts: vec![
            resource(&format!("{base}/prompts/review.md"), true, &meta),
            resource(&format!("{base}/prompts/deploy.md"), true, &meta),
        ],
        ..ResolvedPaths::default()
    };
    let closed = Rc::new(RefCell::new(false));
    let on_close = closed.clone();
    let settings = Rc::new(RefCell::new(SettingsManager::create(&cwd, &cwd)));
    let mut selector = ConfigSelectorComponent::new(
        &resolved,
        settings,
        &cwd,
        &cwd,
        move || *on_close.borrow_mut() = true,
        || {},
    );
    for ch in ["r", "e", "v"] {
        selector.handle_input(ch);
    }
    let screen = strip(&selector.render(80));
    assert!(screen.contains("review.md"));
    assert!(!screen.contains("deploy.md"));

    selector.handle_input("\x1b");
    assert!(*closed.borrow());
}
