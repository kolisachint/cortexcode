//! Lossless output compression: hoocode `harness/utils/output-compression.ts`
//! (v0.5.89). General whitespace compression for every output, plus
//! command-aware compression of bash output (npm installs, git diffs, test
//! runners, docker builds). Outputs under 1KB are left alone.

use std::sync::OnceLock;

use regex::Regex;

/// Below this many bytes compression costs more than it saves.
const MIN_COMPRESSION_SIZE: usize = 1024;

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

/// JS `trimEnd()` whitespace (Rust's differs in U+FEFF and U+0085).
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

fn js_trim_end(line: &str) -> &str {
    line.trim_end_matches(is_js_space)
}

/// `collapseBlankLines`: three or more newlines become two.
pub fn collapse_blank_lines(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    regex(&RE, r"\n{3,}").replace_all(text, "\n\n").into_owned()
}

/// `stripTrailingWhitespace`.
pub fn strip_trailing_whitespace(text: &str) -> String {
    text.split('\n')
        .map(js_trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `removeDuplicateLines`: drop consecutive repeats.
pub fn remove_duplicate_lines(text: &str) -> String {
    let mut result: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if result.last() != Some(&line) {
            result.push(line);
        }
    }
    result.join("\n")
}

/// `compressGeneral`: trailing whitespace stripped, at most two blank lines
/// in a row.
pub fn compress_general(text: &str) -> String {
    let mut result: Vec<&str> = Vec::new();
    let mut blank_count = 0;
    for line in text.split('\n') {
        let trimmed = js_trim_end(line);
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                result.push(trimmed);
            }
        } else {
            blank_count = 0;
            result.push(trimmed);
        }
    }
    result.join("\n")
}

/// `detectCommand`: the program name, past `VAR=value` and `sudo`.
fn detect_command(command: &str) -> String {
    static ENV: OnceLock<Regex> = OnceLock::new();
    static SUDO: OnceLock<Regex> = OnceLock::new();
    static SPACE: OnceLock<Regex> = OnceLock::new();
    let trimmed = command.trim_matches(is_js_space);
    let without_env = regex(&ENV, r"^[A-Z_]+=\S+\s+").replace(trimmed, "");
    let without_sudo = regex(&SUDO, r"^sudo\s+").replace(&without_env, "");
    let first_word = regex(&SPACE, r"\s+")
        .split(&without_sudo)
        .next()
        .unwrap_or("");
    first_word.rsplit('/').next().unwrap_or("").to_string()
}

fn keep_lines(output: &str, keep: impl Fn(&str) -> bool) -> String {
    output
        .split('\n')
        .filter(|line| keep(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `compressNpmInstall`: drop progress and fetch/cache chatter.
fn compress_npm_install(output: &str) -> String {
    static PROGRESS: OnceLock<Regex> = OnceLock::new();
    static FETCH: OnceLock<Regex> = OnceLock::new();
    let progress = regex(
        &PROGRESS,
        r"^(fetchMetadata|reify|audit|idealTree|sill|warn)(?-u:\b)",
    );
    let fetch = regex(
        &FETCH,
        r"^\s*(http|https|fetch|cache|tarball|extract)(?-u:\b)",
    );
    keep_lines(output, |line| {
        !progress.is_match(line) && !fetch.is_match(line)
    })
}

/// `compressGitDiff`: drop file headers, keep two context lines per run.
fn compress_git_diff(output: &str) -> String {
    static META: OnceLock<Regex> = OnceLock::new();
    let meta = regex(&META, r"^(diff --git|index [0-9a-f]+|--- a/|\+\+\+ b/)");
    let mut result: Vec<&str> = Vec::new();
    let mut context_count = 0;
    for line in output.split('\n') {
        if meta.is_match(line) {
            continue;
        }
        if line.starts_with(' ') && !line.starts_with("  ") {
            context_count += 1;
            if context_count > 2 {
                continue;
            }
        } else {
            context_count = 0;
        }
        result.push(line);
    }
    result.join("\n")
}

fn omitted(count: usize) -> String {
    format!("  ({count} passing tests omitted)")
}

/// `compressCargoTest`: passing tests become a count.
fn compress_cargo_test(output: &str) -> String {
    static OK: OnceLock<Regex> = OnceLock::new();
    static IGNORED: OnceLock<Regex> = OnceLock::new();
    let ok = regex(&OK, r"^test\s+.+\s+\.\.\.\s+ok\s*$");
    let ignored = regex(&IGNORED, r"^test\s+.+\s+\.\.\.\s+ignored\s*$");
    let mut result: Vec<String> = Vec::new();
    let mut passing = 0;
    for line in output.split('\n') {
        if ok.is_match(line) {
            passing += 1;
            continue;
        }
        if ignored.is_match(line) {
            continue;
        }
        if line.starts_with("test result:") {
            result.push(line.to_string());
            continue;
        }
        if passing > 0 && line.is_empty() {
            result.push(omitted(passing));
            passing = 0;
        }
        result.push(line.to_string());
    }
    if passing > 0 {
        result.push(omitted(passing));
    }
    result.join("\n")
}

/// `compressDockerBuild`: drop layer progress and progress bars.
fn compress_docker_build(output: &str) -> String {
    static PROGRESS: OnceLock<Regex> = OnceLock::new();
    static BAR: OnceLock<Regex> = OnceLock::new();
    let progress = regex(
        &PROGRESS,
        r"^(Downloading|Pulling|Extracting|Waiting|Verifying)",
    );
    let bar = regex(&BAR, r"[\u{2588}\u{2591}\u{2592}]{10,}");
    keep_lines(output, |line| {
        !progress.is_match(line) && !bar.is_match(line)
    })
}

/// `compressJsTest`: jest/mocha/vitest/pytest passes become a count.
fn compress_js_test(output: &str) -> String {
    static MARK: OnceLock<Regex> = OnceLock::new();
    static PASS: OnceLock<Regex> = OnceLock::new();
    static SUMMARY: OnceLock<Regex> = OnceLock::new();
    let mark = regex(&MARK, r"^\s*[✓✔○●]\s+");
    let pass = regex(&PASS, r"^\s*PASS\s+");
    let summary = regex(&SUMMARY, r"^(Tests|Test Suites):");
    let mut result: Vec<String> = Vec::new();
    let mut passing = 0;
    for line in output.split('\n') {
        if mark.is_match(line) || pass.is_match(line) {
            passing += 1;
            continue;
        }
        if summary.is_match(line) {
            result.push(line.to_string());
            continue;
        }
        if passing > 0 && (line.contains("FAIL") || line.contains('×') || line.contains('✗')) {
            result.push(omitted(passing));
            passing = 0;
        }
        result.push(line.to_string());
    }
    if passing > 0 {
        result.push(omitted(passing));
    }
    result.join("\n")
}

/// `compressGoTest`: `--- PASS` lines become a count.
fn compress_go_test(output: &str) -> String {
    static PASS: OnceLock<Regex> = OnceLock::new();
    static SUMMARY: OnceLock<Regex> = OnceLock::new();
    let pass = regex(&PASS, r"^---\s+PASS:");
    let summary = regex(&SUMMARY, r"^(ok|FAIL)\s+");
    let mut result: Vec<String> = Vec::new();
    let mut passing = 0;
    for line in output.split('\n') {
        if pass.is_match(line) {
            passing += 1;
            continue;
        }
        if line == "PASS" {
            continue;
        }
        if summary.is_match(line) {
            result.push(line.to_string());
            continue;
        }
        if passing > 0 && line.contains("FAIL") {
            result.push(omitted(passing));
            passing = 0;
        }
        result.push(line.to_string());
    }
    if passing > 0 {
        result.push(omitted(passing));
    }
    result.join("\n")
}

/// Whether `command` contains `word` as a whole ASCII word (`/\bword\b/`).
fn has_word(command: &str, words: &str) -> bool {
    Regex::new(&format!(r"(?-u:\b)({words})(?-u:\b)"))
        .expect("valid regex")
        .is_match(command)
}

/// `compressCommandSpecific`.
fn compress_command_specific(command: &str, output: &str) -> String {
    match detect_command(command).as_str() {
        "npm" | "yarn" | "pnpm" if has_word(command, "install|add|i") => {
            compress_npm_install(output)
        }
        "git" if has_word(command, "diff") => compress_git_diff(output),
        "cargo" if has_word(command, "test") => compress_cargo_test(output),
        "docker" if has_word(command, "build") => compress_docker_build(output),
        "jest" | "mocha" | "vitest" | "pytest" => compress_js_test(output),
        "go" if has_word(command, "test") => compress_go_test(output),
        _ => output.to_string(),
    }
}

/// `stripNoisePatterns`: warning lines of npm, yarn, pnpm and bash are
/// emptied (the lines stay, as blank lines).
fn strip_noise_patterns(text: &str) -> String {
    static NOISE: OnceLock<Regex> = OnceLock::new();
    let noise = regex(
        &NOISE,
        r"(?m)^(?:npm (?:warn|notice) .+|warning .+|pnpm (?:warn|notice) .+|bash: .+ warning: .+|The command exited with exit code .+)$",
    );
    noise.replace_all(text, "").into_owned()
}

/// `compressBashOutput`: noise, then command-specific, then general
/// compression.
pub fn compress_bash_output(command: &str, output: &str) -> String {
    if output.len() < MIN_COMPRESSION_SIZE {
        return output.to_string();
    }
    let result = strip_noise_patterns(output);
    let result = compress_command_specific(command, &result);
    compress_general(&result)
}

/// `compressGrepOutput`.
pub fn compress_grep_output(output: &str) -> String {
    if output.len() < MIN_COMPRESSION_SIZE {
        return output.to_string();
    }
    compress_general(output)
}

/// `compressReadOutput`: trailing whitespace only (no line collapsing).
pub fn compress_read_output(output: &str) -> String {
    if output.len() < MIN_COMPRESSION_SIZE {
        return output.to_string();
    }
    strip_trailing_whitespace(output)
}

/// `compressFindOutput`.
pub fn compress_find_output(output: &str) -> String {
    compress_grep_output(output)
}

/// `compressLsOutput`.
pub fn compress_ls_output(output: &str) -> String {
    compress_grep_output(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big(text: &str) -> String {
        // Past the 1KB threshold without changing what compression sees.
        format!("{text}\n{}", "x".repeat(MIN_COMPRESSION_SIZE))
    }

    #[test]
    fn general_compression_strips_trailing_space_and_caps_blank_runs() {
        assert_eq!(compress_general("a  \n\n\n\n\nb\t"), "a\n\n\nb");
        assert_eq!(collapse_blank_lines("a\n\n\n\nb"), "a\n\nb");
        assert_eq!(remove_duplicate_lines("a\na\nb\na"), "a\nb\na");
        assert_eq!(strip_trailing_whitespace("a \u{FEFF}\nb"), "a\nb");
    }

    #[test]
    fn small_outputs_are_left_alone() {
        assert_eq!(compress_bash_output("npm i", "warn x  \n"), "warn x  \n");
        assert_eq!(compress_read_output("a  "), "a  ");
    }

    #[test]
    fn detects_the_program_past_env_and_sudo() {
        assert_eq!(detect_command("  FOO=1 sudo /usr/bin/cargo test"), "cargo");
        assert_eq!(detect_command("git diff"), "git");
    }

    #[test]
    fn cargo_test_passes_become_a_count() {
        let out = compress_bash_output(
            "cargo test -p x",
            &big("test a ... ok\ntest b ... ok\ntest c ... ignored\ntest d ... FAILED\n\ntest result: FAILED"),
        );
        assert!(out
            .starts_with("test d ... FAILED\n  (2 passing tests omitted)\n\ntest result: FAILED"));
    }

    #[test]
    fn npm_noise_and_install_progress_are_dropped() {
        let out = compress_bash_output(
            "npm install",
            &big("npm warn deprecated x\nreify: thing\nadded 3 packages"),
        );
        // The warning line is emptied, the progress line dropped.
        assert!(out.starts_with("\nadded 3 packages\n"), "{out:?}");
    }

    #[test]
    fn git_diff_keeps_two_context_lines() {
        let out = compress_bash_output(
            "git diff",
            &big("diff --git a/f b/f\nindex 1a2b\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n c1\n c2\n c3\n-old\n+new"),
        );
        assert!(
            out.starts_with("@@ -1 +1 @@\n c1\n c2\n-old\n+new\n"),
            "{out:?}"
        );
    }

    #[test]
    fn js_and_go_test_passes_become_counts() {
        let out = compress_bash_output(
            "vitest run",
            &big(" ✓ a\n ✓ b\n × c failed\nTests: 1 failed"),
        );
        assert!(out.starts_with("  (2 passing tests omitted)\n × c failed\nTests: 1 failed"));
        let out = compress_bash_output(
            "go test ./...",
            &big("--- PASS: TestA (0.00s)\nPASS\nok  pkg 0.1s"),
        );
        assert!(out.starts_with("ok  pkg 0.1s\n"), "{out:?}");
        assert!(out.ends_with("  (1 passing tests omitted)"), "{out:?}");
    }
}
