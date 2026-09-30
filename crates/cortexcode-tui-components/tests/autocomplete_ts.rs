//! Case-for-case port of the pin's `test/autocomplete.test.ts`. Like the TS
//! suite, the `fd @ file suggestions` cases need `fd` on PATH and are skipped
//! (with a note on stderr) without it.

use cortexcode_tui_components::{
    AutocompleteProvider, AutocompleteSuggestions, CombinedAutocompleteProvider,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cortex-autocomplete-ts-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn setup_folder(base: &Path, dirs: &[&str], files: &[(&str, &str)]) {
    for d in dirs {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    for (path, contents) in files {
        let full = base.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, contents).unwrap();
    }
}

fn fd_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    // Debian and Ubuntu install fd as `fdfind`.
    std::env::split_paths(&path)
        .flat_map(|d| [d.join("fd"), d.join("fdfind")])
        .find(|p| p.is_file())
}

fn suggest(
    p: &CombinedAutocompleteProvider,
    line: &str,
    col: usize,
    force: bool,
) -> Option<AutocompleteSuggestions> {
    p.get_suggestions(&[line.to_string()], 0, col, force)
}

fn values(r: &Option<AutocompleteSuggestions>) -> Vec<String> {
    r.as_ref()
        .map(|r| r.items.iter().map(|i| i.value.clone()).collect())
        .unwrap_or_default()
}

// --- extractPathPrefix

#[test]
fn extracts_root_slash_from_hey_slash_when_forced() {
    let p = CombinedAutocompleteProvider::new(vec![], "/tmp", None);
    let r = suggest(&p, "hey /", 5, true).expect("suggestions for /");
    assert_eq!(r.prefix, "/");
}

#[test]
fn extracts_slash_a_when_forced() {
    let p = CombinedAutocompleteProvider::new(vec![], "/tmp", None);
    // May be None when nothing under / starts with A; the prefix is the point.
    if let Some(r) = suggest(&p, "/A", 2, true) {
        assert_eq!(r.prefix, "/A");
    }
}

#[test]
fn does_not_trigger_for_slash_commands() {
    let p = CombinedAutocompleteProvider::new(vec![], "/tmp", None);
    assert!(suggest(&p, "/model", 6, true).is_none());
}

#[test]
fn triggers_for_absolute_paths_after_slash_command_argument() {
    let p = CombinedAutocompleteProvider::new(vec![], "/tmp", None);
    let r = suggest(&p, "/command /", 10, true).expect("suggestions");
    assert_eq!(r.prefix, "/");
}

// --- fd @ file suggestions

struct FdCase {
    _root: TempDir,
    base: PathBuf,
    outside: PathBuf,
    fd: PathBuf,
}

fn fd_case() -> Option<FdCase> {
    let Some(fd) = fd_path() else {
        eprintln!("fd not installed: skipping (as the TS suite does)");
        return None;
    };
    let root = TempDir::new();
    let base = root.0.join("cwd");
    let outside = root.0.join("outside");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(&outside).unwrap();
    Some(FdCase {
        _root: root,
        base,
        outside,
        fd,
    })
}

impl FdCase {
    fn provider(&self) -> CombinedAutocompleteProvider {
        self.provider_at(&self.base)
    }
    fn provider_at(&self, base: &Path) -> CombinedAutocompleteProvider {
        CombinedAutocompleteProvider::new(vec![], base, Some(self.fd.clone()))
    }
    fn at(&self, line: &str) -> Vec<String> {
        values(&suggest(
            &self.provider(),
            line,
            line.chars().count(),
            false,
        ))
    }
}

#[test]
fn fd_returns_all_files_and_folders_for_empty_at_query() {
    let Some(c) = fd_case() else { return };
    setup_folder(&c.base, &["src"], &[("README.md", "readme")]);
    let mut v = c.at("@");
    v.sort();
    assert_eq!(v, ["@README.md", "@src/"]);
}

#[test]
fn fd_matches_file_with_extension_in_query() {
    let Some(c) = fd_case() else { return };
    setup_folder(&c.base, &[], &[("file.txt", "content")]);
    assert!(c.at("@file.txt").contains(&"@file.txt".to_string()));
}

#[test]
fn fd_filters_are_case_insensitive() {
    let Some(c) = fd_case() else { return };
    setup_folder(&c.base, &["src"], &[("README.md", "readme")]);
    assert_eq!(c.at("@re"), ["@README.md"]);
}

#[test]
fn fd_ranks_directories_before_files() {
    let Some(c) = fd_case() else { return };
    setup_folder(&c.base, &["src"], &[("src.txt", "text")]);
    let v = c.at("@src");
    assert_eq!(v.first().map(String::as_str), Some("@src/"));
    assert!(v.contains(&"@src.txt".to_string()));
}

#[test]
fn fd_returns_nested_file_paths() {
    let Some(c) = fd_case() else { return };
    setup_folder(&c.base, &[], &[("src/index.ts", "export {};\n")]);
    assert!(c.at("@index").contains(&"@src/index.ts".to_string()));
}

#[test]
fn fd_matches_deeply_nested_paths() {
    let Some(c) = fd_case() else { return };
    setup_folder(
        &c.base,
        &[],
        &[
            ("packages/tui/src/autocomplete.ts", "export {};"),
            ("packages/ai/src/autocomplete.ts", "export {};"),
        ],
    );
    let v = c.at("@tui/src/auto");
    assert!(v.contains(&"@packages/tui/src/autocomplete.ts".to_string()));
    assert!(!v.contains(&"@packages/ai/src/autocomplete.ts".to_string()));
}

#[test]
fn fd_matches_directory_in_middle_of_path() {
    let Some(c) = fd_case() else { return };
    setup_folder(
        &c.base,
        &[],
        &[
            ("src/components/Button.tsx", "export {};"),
            ("src/utils/helpers.ts", "export {};"),
        ],
    );
    let v = c.at("@components/");
    assert!(v.contains(&"@src/components/Button.tsx".to_string()));
    assert!(!v.contains(&"@src/utils/helpers.ts".to_string()));
}

#[test]
fn fd_scopes_fuzzy_search_to_relative_directories_recursively() {
    let Some(c) = fd_case() else { return };
    setup_folder(
        &c.outside,
        &[],
        &[
            ("nested/alpha.ts", "export {};"),
            ("nested/deeper/also-alpha.ts", "export {};"),
            ("nested/deeper/zzz.ts", "export {};"),
        ],
    );
    let v = c.at("@../outside/a");
    assert!(v.contains(&"@../outside/nested/alpha.ts".to_string()));
    assert!(v.contains(&"@../outside/nested/deeper/also-alpha.ts".to_string()));
    assert!(!v.contains(&"@../outside/nested/deeper/zzz.ts".to_string()));
}

#[test]
fn fd_quotes_paths_with_spaces() {
    let Some(c) = fd_case() else { return };
    setup_folder(
        &c.base,
        &["my folder"],
        &[("my folder/test.txt", "content")],
    );
    assert!(c.at("@my").contains(&"@\"my folder/\"".to_string()));
}

#[test]
fn fd_includes_hidden_paths_but_excludes_git() {
    let Some(c) = fd_case() else { return };
    // `.hoocode` in the TS test: any hidden directory will do.
    setup_folder(
        &c.base,
        &[".hoocode", ".github", ".git"],
        &[
            (".hoocode/config.json", "{}"),
            (".github/workflows/ci.yml", "name: ci"),
            (".git/config", "[core]"),
        ],
    );
    let v = c.at("@");
    assert!(v.contains(&"@.hoocode/".to_string()));
    assert!(v.contains(&"@.github/".to_string()));
    assert!(!v.iter().any(|x| x == "@.git" || x.starts_with("@.git/")));
}

#[cfg(unix)]
#[test]
fn fd_follows_symlinked_directories() {
    let Some(c) = fd_case() else { return };
    setup_folder(&c.base, &[], &[("dir/some_file.txt", "real")]);
    setup_folder(&c.outside, &[], &[("some_file.txt", "symlinked")]);
    std::os::unix::fs::symlink("../outside", c.base.join("symlinked_dir")).unwrap();
    let v = c.at("@some");
    assert!(v.contains(&"@dir/some_file.txt".to_string()));
    assert!(v.contains(&"@symlinked_dir/some_file.txt".to_string()));
}

#[cfg(unix)]
#[test]
fn fd_returns_symlinked_directories_by_name() {
    let Some(c) = fd_case() else { return };
    setup_folder(&c.outside, &[], &[("nested/file.txt", "symlinked")]);
    std::os::unix::fs::symlink("../outside", c.base.join("symlinked_dir")).unwrap();
    assert!(c.at("@symlinked").contains(&"@symlinked_dir/".to_string()));
}

#[cfg(unix)]
#[test]
fn fd_returns_symlinked_files() {
    let Some(c) = fd_case() else { return };
    setup_folder(&c.base, &[], &[("original.txt", "content")]);
    std::os::unix::fs::symlink("original.txt", c.base.join("link.txt")).unwrap();
    assert!(c.at("@link").contains(&"@link.txt".to_string()));
}

#[test]
fn fd_suggestions_do_not_depend_on_the_query_appearing_in_cwd() {
    let Some(c) = fd_case() else { return };
    let root = c.base.parent().unwrap();
    let normal = root.join("cwd-normal");
    let in_path = root.join("cwd-plan-repro");
    for base in [&normal, &in_path] {
        setup_folder(
            base,
            &["packages/coding-agent/examples/extensions/plan-mode"],
            &[
                (
                    "packages/coding-agent/examples/extensions/plan-mode/README.md",
                    "readme",
                ),
                ("packages/tui/docs/plan.md", "plan"),
            ],
        );
    }
    let norm = |base: &Path| {
        let r = suggest(&c.provider_at(base), "@plan", 5, false);
        let mut v: Vec<String> = r
            .map(|r| {
                r.items
                    .iter()
                    .map(|i| {
                        format!(
                            "{} :: {}",
                            i.label,
                            i.description.clone().unwrap_or_default()
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    };
    let a = norm(&normal);
    assert_eq!(norm(&in_path), a);
    assert!(a.contains(
        &"plan-mode/ :: packages/coding-agent/examples/extensions/plan-mode".to_string()
    ));
    assert!(a.contains(&"plan.md :: packages/tui/docs/plan.md".to_string()));
}

#[test]
fn fd_continues_inside_quoted_at_paths() {
    let Some(c) = fd_case() else { return };
    setup_folder(
        &c.base,
        &[],
        &[
            ("my folder/test.txt", "content"),
            ("my folder/other.txt", "content"),
        ],
    );
    let line = "@\"my folder/\"";
    let r = suggest(&c.provider(), line, line.chars().count() - 1, false);
    assert!(r.is_some());
    let v = values(&r);
    assert!(v.contains(&"@\"my folder/test.txt\"".to_string()));
    assert!(v.contains(&"@\"my folder/other.txt\"".to_string()));
}

#[test]
fn fd_applies_quoted_at_completion_without_duplicating_closing_quote() {
    let Some(c) = fd_case() else { return };
    setup_folder(&c.base, &[], &[("my folder/test.txt", "content")]);
    let p = c.provider();
    let line = "@\"my folder/te\"";
    let col = line.chars().count() - 1;
    let r = suggest(&p, line, col, false).expect("suggestions");
    let item = r
        .items
        .iter()
        .find(|i| i.value == "@\"my folder/test.txt\"")
        .expect("test.txt");
    let applied = p.apply_completion(&[line.to_string()], 0, col, item, &r.prefix);
    assert_eq!(applied.lines[0], "@\"my folder/test.txt\" ");
}

// --- dot-slash path completion

#[test]
fn preserves_dot_slash_prefix_when_completing_paths() {
    let t = TempDir::new();
    setup_folder(
        &t.0,
        &[],
        &[("update.sh", "#!/bin/bash"), ("utils.ts", "export {};")],
    );
    let p = CombinedAutocompleteProvider::new(vec![], &t.0, None);
    let r = suggest(&p, "./up", 4, true);
    assert!(r.is_some());
    assert!(
        values(&r).contains(&"./update.sh".to_string()),
        "{:?}",
        values(&r)
    );
}

#[test]
fn preserves_dot_slash_prefix_for_directory_completions() {
    let t = TempDir::new();
    setup_folder(&t.0, &["src"], &[("src/index.ts", "export {};")]);
    let p = CombinedAutocompleteProvider::new(vec![], &t.0, None);
    let r = suggest(&p, "./sr", 4, true);
    assert!(r.is_some());
    assert!(
        values(&r).contains(&"./src/".to_string()),
        "{:?}",
        values(&r)
    );
}

// --- quoted path completion

#[test]
fn quotes_paths_with_spaces_for_direct_completion() {
    let t = TempDir::new();
    setup_folder(&t.0, &["my folder"], &[("my folder/test.txt", "content")]);
    let p = CombinedAutocompleteProvider::new(vec![], &t.0, None);
    let r = suggest(&p, "my", 2, true);
    assert!(values(&r).contains(&"\"my folder/\"".to_string()));
}

#[test]
fn continues_completion_inside_quoted_paths() {
    let t = TempDir::new();
    setup_folder(
        &t.0,
        &[],
        &[
            ("my folder/test.txt", "content"),
            ("my folder/other.txt", "content"),
        ],
    );
    let p = CombinedAutocompleteProvider::new(vec![], &t.0, None);
    let line = "\"my folder/\"";
    let r = suggest(&p, line, line.chars().count() - 1, true);
    let v = values(&r);
    assert!(v.contains(&"\"my folder/test.txt\"".to_string()));
    assert!(v.contains(&"\"my folder/other.txt\"".to_string()));
}

#[test]
fn applies_quoted_completion_without_duplicating_closing_quote() {
    let t = TempDir::new();
    setup_folder(&t.0, &[], &[("my folder/test.txt", "content")]);
    let p = CombinedAutocompleteProvider::new(vec![], &t.0, None);
    let line = "\"my folder/te\"";
    let col = line.chars().count() - 1;
    let r = suggest(&p, line, col, true).expect("suggestions");
    let item = r
        .items
        .iter()
        .find(|i| i.value == "\"my folder/test.txt\"")
        .expect("test.txt");
    let applied = p.apply_completion(&[line.to_string()], 0, col, item, &r.prefix);
    assert_eq!(applied.lines[0], "\"my folder/test.txt\"");
}
