//! Case-for-case port of the pin's `coding-agent/test/path-utils.test.ts`.

use cortexcode_code_tool_api::{expand_path, resolve_read_path, resolve_to_cwd};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "path-utils-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn touch(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        fs::write(&p, "content").unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn expands_tilde_to_home() {
    assert!(!expand_path("~").contains('~'));
}

#[test]
fn expands_tilde_slash_path_to_home() {
    assert!(!expand_path("~/Documents/file.txt").contains("~/"));
}

#[test]
fn normalizes_unicode_spaces() {
    assert_eq!(expand_path("file\u{00A0}name.txt"), "file name.txt");
}

#[test]
fn resolves_absolute_paths_as_is() {
    assert_eq!(
        resolve_to_cwd("/absolute/path/file.txt", Path::new("/some/cwd")),
        PathBuf::from("/absolute/path/file.txt")
    );
}

#[test]
fn resolves_relative_paths_against_cwd() {
    assert_eq!(
        resolve_to_cwd("relative/file.txt", Path::new("/some/cwd")),
        PathBuf::from("/some/cwd/relative/file.txt")
    );
}

#[test]
fn resolves_existing_file_path() {
    let t = TempDir::new();
    let p = t.touch("test-file.txt");
    assert_eq!(resolve_read_path("test-file.txt", &t.0), p);
}

#[test]
fn handles_nfc_vs_nfd_filenames() {
    let t = TempDir::new();
    let nfd = "file\u{0065}\u{0301}.txt";
    let nfc = "file\u{00e9}.txt";
    assert_ne!(nfd, nfc);
    t.touch(nfd);
    let r = resolve_read_path(nfc, &t.0);
    let s = r.to_string_lossy();
    assert!(r.starts_with(&t.0));
    assert!(s.ends_with(".txt") && s.contains("/file"));
}

#[test]
fn handles_curly_vs_straight_quotes() {
    let t = TempDir::new();
    let p = t.touch("Capture d\u{2019}cran.txt");
    assert_eq!(resolve_read_path("Capture d'cran.txt", &t.0), p);
}

#[test]
fn handles_combined_nfc_and_curly_quote() {
    let t = TempDir::new();
    let p = t.touch("Capture d\u{2019}\u{00e9}cran.txt");
    assert_eq!(resolve_read_path("Capture d'\u{00e9}cran.txt", &t.0), p);
}

#[test]
fn handles_screenshot_am_pm_with_narrow_no_break_space() {
    let t = TempDir::new();
    let p = t.touch("Screenshot 2024-01-01 at 10.00.00\u{202F}AM.png");
    assert_eq!(
        resolve_read_path("Screenshot 2024-01-01 at 10.00.00 AM.png", &t.0),
        p
    );
}

#[test]
fn handles_screenshot_lowercase_am_pm() {
    let t = TempDir::new();
    let p = t.touch("Screenshot 2024-01-01 at 10.00.00\u{202F}am.png");
    assert_eq!(
        resolve_read_path("Screenshot 2024-01-01 at 10.00.00 am.png", &t.0),
        p
    );
}
