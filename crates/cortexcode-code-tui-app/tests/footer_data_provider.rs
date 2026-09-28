//! Port of the pin's `test/footer-data-provider.test.ts`. hoocode mocks
//! `child_process` to fake git's answer for reftable repos; these tests use
//! real files instead (a fake reftable repo that git cannot resolve is the
//! detached case), and exercise the watcher through HEAD rewrites. The
//! fs.watch error-retry case has no counterpart: the watcher polls.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cortexcode_code_tui_app::footer_data::FooterDataProvider;

fn plain_repo(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    repo
}

fn wait_for(cond: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

/// HEAD rewritten so the change is visible even on coarse mtimes.
fn write_head(repo: &Path, content: &str) {
    std::fs::write(repo.join(".git/HEAD"), content).unwrap();
}

#[test]
fn uses_head_directly_in_a_regular_repo_from_a_nested_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let nested = plain_repo(tmp.path()).join("src/nested");
    std::fs::create_dir_all(&nested).unwrap();
    let provider = FooterDataProvider::new(&nested);
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));
    provider.dispose();
}

#[test]
fn treats_an_unresolved_invalid_reftable_head_as_detached() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git/reftable")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/.invalid\n").unwrap();
    let provider = FooterDataProvider::new(&repo);
    assert_eq!(provider.get_git_branch().as_deref(), Some("detached"));
    provider.dispose();
}

#[test]
fn is_none_outside_a_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = FooterDataProvider::new(tmp.path());
    assert_eq!(provider.get_git_branch(), None);
    provider.dispose();
}

#[test]
fn does_not_notify_listeners_when_an_update_keeps_the_same_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = plain_repo(tmp.path());
    let provider = FooterDataProvider::new(&repo);
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = calls.clone();
    provider.on_branch_change(move || {
        sink.fetch_add(1, Ordering::SeqCst);
    });
    write_head(&repo, "ref: refs/heads/main\n\n");
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));
    provider.dispose();
}

#[test]
fn debounces_rapid_updates_into_a_single_refresh_and_updates_the_cached_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = plain_repo(tmp.path());
    let provider = FooterDataProvider::new(&repo);
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = calls.clone();
    provider.on_branch_change(move || {
        sink.fetch_add(1, Ordering::SeqCst);
    });
    write_head(&repo, "ref: refs/heads/a\n");
    write_head(&repo, "ref: refs/heads/bb\n");
    write_head(&repo, "ref: refs/heads/foo\n");
    assert!(wait_for(
        || provider.get_git_branch().as_deref() == Some("foo")
    ));
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    provider.dispose();
}

#[test]
fn set_cwd_moves_to_another_repo_and_notifies() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = plain_repo(tmp.path());
    let other = tmp.path().join("other");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    std::fs::write(other.join(".git/HEAD"), "ref: refs/heads/dev\n").unwrap();
    let provider = FooterDataProvider::new(&repo);
    assert_eq!(provider.get_git_branch().as_deref(), Some("main"));
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = calls.clone();
    provider.on_branch_change(move || {
        sink.fetch_add(1, Ordering::SeqCst);
    });
    provider.set_cwd(&other);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.get_git_branch().as_deref(), Some("dev"));
    provider.dispose();
}
