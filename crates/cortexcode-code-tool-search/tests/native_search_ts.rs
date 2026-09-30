//! Port of the pin's `coding-agent/test/native-search.test.ts`.
//!
//! hoocode's `nativeGrep` is the pure-JS stand-in for `rg` when the binary is
//! missing; cortex has no `rg` to miss, and its only content search is the
//! in-process `run_lexical_retriever`. The cases port onto it where it has
//! the knob: hidden files, line hits, glob, limit, `.gitignore`, no match,
//! binary files. It has no `ignoreCase`/`literal` flags or single-file root
//! (the query plan is always case-insensitive and escaped, and the root is
//! the cwd), so those cases, and the invalid-regex tag they lead to, are n.a.

use std::path::{Path, PathBuf};

use cortexcode_code_tool_search::{run_lexical_retriever, GrepLineHit, RunLexicalOptions};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("hoo-native-search-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        std::fs::write(self.0.join(rel), content).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn grep(cwd: &Path, query: &str, glob: Option<&str>, limit: usize) -> Vec<GrepLineHit> {
    run_lexical_retriever(RunLexicalOptions {
        cwd,
        query,
        limit,
        glob,
        signal: None,
        paths: None,
    })
    .unwrap()
}

fn text(hits: &[GrepLineHit]) -> String {
    hits.iter()
        .map(|h| format!("{}:{}", h.rel, h.line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `nativeGrep` fixture.
fn fixture() -> TempDir {
    let d = TempDir::new();
    d.write("one.ts", "const needle = 1;\nconst other = 2;\n");
    d.write("two.js", "// NEEDLE here\nfoo();\n");
    d.write("three.txt", "no match here\n");
    d
}

#[test]
fn includes_hidden_files_that_are_not_gitignored() {
    let d = TempDir::new();
    d.write(".hidden.ts", "needle");
    assert!(text(&grep(&d.0, "needle", None, 100)).contains(".hidden.ts"));
}

#[test]
fn finds_matches_with_file_paths_and_line_numbers() {
    let d = fixture();
    let t = text(&grep(&d.0, "needle", None, 100));
    assert!(t.contains("one.ts:1"), "{t}");
    assert!(!t.contains("three.txt"));
}

#[test]
fn filters_files_by_glob() {
    let d = fixture();
    let t = text(&grep(&d.0, "needle", Some("*.ts"), 100));
    assert!(t.contains("one.ts"));
    assert!(!t.contains("two.js"));
}

#[test]
fn respects_the_match_limit() {
    let d = TempDir::new();
    d.write("many.txt", "hit\nhit\nhit\nhit\n");
    assert_eq!(grep(&d.0, "hit", None, 2).len(), 2);
}

#[test]
fn respects_gitignore() {
    let d = fixture();
    d.write(".gitignore", "ignored.ts\n");
    d.write("ignored.ts", "const needle = 9;\n");
    assert!(!text(&grep(&d.0, "needle", None, 100)).contains("ignored.ts"));
}

#[test]
fn returns_no_matches_when_nothing_matches() {
    let d = fixture();
    assert!(grep(&d.0, "zzz-nonexistent-zzz", None, 100).is_empty());
}

#[test]
fn skips_binary_files() {
    let d = fixture();
    d.write("bin.dat", [0x6e, 0x00, 0x65, 0x65, 0x64, 0x6c, 0x65]);
    assert!(!text(&grep(&d.0, "eedle", None, 100)).contains("bin.dat"));
}

// --- suite/regressions/3303-find-nested-gitignore.test.ts: each .gitignore is
// scoped to its own subtree (pinned on `collectEntries`, the walk behind the
// lexical retriever; here the retriever's own walk).

fn walk(d: &TempDir) -> Vec<String> {
    let mut rels: Vec<String> = grep(&d.0, "needle", None, 100)
        .into_iter()
        .map(|h| h.rel)
        .filter(|r| r.ends_with(".txt"))
        .collect();
    rels.sort();
    rels
}

fn files(d: &TempDir, rels: &[&str]) {
    for rel in rels {
        std::fs::create_dir_all(d.0.join(rel).parent().unwrap()).unwrap();
        d.write(rel, "needle\n");
    }
}

#[test]
fn issue_3303_nested_gitignore_applies_only_inside_its_directory() {
    let d = TempDir::new();
    files(
        &d,
        &[
            "a/ignored.txt",
            "a/kept.txt",
            "b/ignored.txt",
            "b/kept.txt",
            "root.txt",
        ],
    );
    d.write("a/.gitignore", "ignored.txt\n");
    assert_eq!(
        walk(&d),
        ["a/kept.txt", "b/ignored.txt", "b/kept.txt", "root.txt"]
    );
}

#[test]
fn issue_3303_scopes_each_gitignore_to_its_own_subtree() {
    let d = TempDir::new();
    files(
        &d,
        &[
            "a/ignored.txt",
            "a/kept.txt",
            "a/deep/ignored.txt",
            "a/deep/secret.txt",
            "a/deep/kept.txt",
            "b/ignored.txt",
            "b/kept.txt",
            "root.txt",
        ],
    );
    d.write("a/.gitignore", "ignored.txt\n");
    d.write("a/deep/.gitignore", "secret.txt\n");
    assert_eq!(
        walk(&d),
        [
            "a/deep/kept.txt",
            "a/kept.txt",
            "b/ignored.txt",
            "b/kept.txt",
            "root.txt"
        ]
    );
}
