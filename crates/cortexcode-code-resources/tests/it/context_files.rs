//! Port of hoocode `packages/coding-agent/test/context-files-user-scope.test.ts` (v0.5.89).

use cortexcode_code_resources::{
    load_project_context_files, ContextFile, ContextFileSize, LoadProjectContextFilesOptions,
};

struct Fixture {
    _dir: tempfile::TempDir,
    cwd: String,
    agent_dir: String,
    user_agents_dir: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_string_lossy().into_owned();
    let f = Fixture {
        cwd: format!("{base}/repo"),
        agent_dir: format!("{base}/.hoocode"),
        user_agents_dir: format!("{base}/.agents"),
        _dir: dir,
    };
    for d in [&f.cwd, &f.agent_dir, &f.user_agents_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    f
}

fn write(dir: &str, content: &str) {
    std::fs::write(format!("{dir}/AGENTS.md"), content).unwrap();
}

fn load(f: &Fixture) -> (Vec<ContextFile>, Vec<String>) {
    load_project_context_files(&LoadProjectContextFilesOptions {
        cwd: f.cwd.clone(),
        agent_dir: f.agent_dir.clone(),
        user_agents_dir: Some(f.user_agents_dir.clone()),
    })
}

fn contents(files: &[ContextFile]) -> Vec<&str> {
    files.iter().map(|f| f.content.as_str()).collect()
}

// user-scope context files

#[test]
fn reads_agents_scope_agents_md_as_instructions() {
    let f = fixture();
    write(&f.user_agents_dir, "prefer functional typescript");
    let (files, _) = load(&f);
    assert_eq!(
        files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
        [format!("{}/AGENTS.md", f.user_agents_dir)]
    );
    assert_eq!(files[0].content, "prefer functional typescript");
}

#[test]
fn loads_both_user_scopes_additively_cross_vendor_first() {
    let f = fixture();
    write(&f.user_agents_dir, "cross-vendor rule");
    write(&f.agent_dir, "native rule");
    assert_eq!(contents(&load(&f).0), ["cross-vendor rule", "native rule"]);
}

#[test]
fn orders_user_scopes_before_the_repo_least_specific_first() {
    let f = fixture();
    write(&f.user_agents_dir, "cross-vendor rule");
    write(&f.agent_dir, "native rule");
    write(&f.cwd, "repo rule");
    assert_eq!(
        contents(&load(&f).0),
        ["cross-vendor rule", "native rule", "repo rule"]
    );
}

#[test]
fn does_not_double_count_when_both_user_scopes_point_at_one_directory() {
    let f = fixture();
    write(&f.agent_dir, "only once");
    let (files, _) = load_project_context_files(&LoadProjectContextFilesOptions {
        cwd: f.cwd.clone(),
        agent_dir: f.agent_dir.clone(),
        user_agents_dir: Some(f.agent_dir.clone()),
    });
    assert_eq!(files.len(), 1);
}

#[test]
fn reads_nothing_when_the_cross_vendor_scope_is_absent() {
    let f = fixture();
    std::fs::remove_dir_all(&f.user_agents_dir).unwrap();
    write(&f.cwd, "repo rule");
    assert_eq!(contents(&load(&f).0), ["repo rule"]);
}

// aggregate context-file budget

#[test]
fn stays_silent_under_the_soft_cap() {
    let f = fixture();
    write(&f.cwd, &"x".repeat(4 * 1024));
    let (files, warnings) = load(&f);
    assert!(warnings.is_empty());
    assert_eq!(files[0].size, None);
}

#[test]
fn warns_without_trimming_between_the_soft_and_hard_caps() {
    let f = fixture();
    write(&f.user_agents_dir, &"u".repeat(16 * 1024));
    write(&f.cwd, &"r".repeat(16 * 1024));
    let (files, warnings) = load(&f);
    assert!(warnings
        .iter()
        .any(|w| w.contains("re-sent on every request")));
    assert!(warnings[0].starts_with("Context files total ~8.2k tokens across 2 file(s)"));
    assert!(files
        .iter()
        .all(|f| f.size != Some(ContextFileSize::Truncated)));
    assert_eq!(
        files.iter().map(|f| f.content.len()).collect::<Vec<_>>(),
        [16 * 1024, 16 * 1024]
    );
}

fn three_large(f: &Fixture) {
    write(&f.user_agents_dir, &"u".repeat(39 * 1024));
    write(&f.agent_dir, &"n".repeat(39 * 1024));
    write(&f.cwd, &"r".repeat(39 * 1024));
}

#[test]
fn trims_the_least_specific_scope_first_past_the_hard_cap() {
    let f = fixture();
    three_large(&f);
    let (files, warnings) = load(&f);
    let (cross_vendor, native, repo) = (&files[0], &files[1], &files[2]);
    assert_eq!(repo.size, Some(ContextFileSize::Large));
    assert_eq!(repo.content, "r".repeat(39 * 1024));
    assert_eq!(native.size, Some(ContextFileSize::Truncated));
    assert_eq!(cross_vendor.size, Some(ContextFileSize::Truncated));
    assert!(warnings.iter().any(|w| w.contains("total budget")));
}

#[test]
fn keeps_a_trimmed_file_visible_instead_of_dropping_it() {
    let f = fixture();
    three_large(&f);
    let (files, _) = load(&f);
    assert_eq!(files.len(), 3);
    for file in &files[..2] {
        assert!(file.content.contains("[trimmed:"));
        assert_eq!(
            file.tokens,
            Some((file.content.len() as f64 / 4.0).round() as u64)
        );
    }
}
