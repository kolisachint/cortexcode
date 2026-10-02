//! Port of hoocode `test/git-ssh-url.test.ts`, plus a golden corpus produced
//! by running the pinned hoocode `parseGitUrl` (fixtures/git-url-gold.json).

use cortexcode_code_paths::git::{parse_git_url, GitSource};

fn check(input: &str, host: &str, path: &str, repo: &str, git_ref: Option<&str>) {
    let r = parse_git_url(input).unwrap_or_else(|| panic!("{input} should parse"));
    assert_eq!(r.host, host, "{input}");
    assert_eq!(r.path, path, "{input}");
    assert_eq!(r.repo, repo, "{input}");
    if let Some(git_ref) = git_ref {
        assert_eq!(r.git_ref.as_deref(), Some(git_ref), "{input}");
    }
}

#[test]
fn protocol_urls_are_accepted_without_git_prefix() {
    check(
        "https://github.com/user/repo",
        "github.com",
        "user/repo",
        "https://github.com/user/repo",
        None,
    );
    check(
        "ssh://git@github.com/user/repo",
        "github.com",
        "user/repo",
        "ssh://git@github.com/user/repo",
        None,
    );
    check(
        "https://github.com/user/repo@v1.0.0",
        "github.com",
        "user/repo",
        "https://github.com/user/repo",
        Some("v1.0.0"),
    );
}

#[test]
fn shorthand_urls_need_the_git_prefix() {
    check(
        "git:git@github.com:user/repo",
        "github.com",
        "user/repo",
        "git@github.com:user/repo",
        None,
    );
    check(
        "git:github.com/user/repo",
        "github.com",
        "user/repo",
        "https://github.com/user/repo",
        None,
    );
    check(
        "git:git@github.com:user/repo@v1.0.0",
        "github.com",
        "user/repo",
        "git@github.com:user/repo",
        Some("v1.0.0"),
    );
    assert_eq!(parse_git_url("git@github.com:user/repo"), None);
    assert_eq!(parse_git_url("github.com/user/repo"), None);
    assert_eq!(parse_git_url("user/repo"), None);
}

#[test]
fn golden_corpus_matches_hoocode() {
    let text = include_str!("../fixtures/git-url-gold.json");
    let cases = fixture::parse(text);
    assert!(cases.len() > 60);
    let mut failures = Vec::new();
    for (input, expected) in cases {
        let got = parse_git_url(&input);
        if got != expected {
            failures.push(format!("{input:?}\n  want {expected:?}\n  got  {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

mod fixture {
    use super::GitSource;

    pub fn parse(text: &str) -> Vec<(String, Option<GitSource>)> {
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        v.as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                let src = (!v.is_null()).then(|| GitSource {
                    repo: v["repo"].as_str().unwrap().into(),
                    host: v["host"].as_str().unwrap().into(),
                    path: v["path"].as_str().unwrap().into(),
                    git_ref: v["ref"].as_str().map(String::from),
                    pinned: v["pinned"].as_bool().unwrap(),
                });
                (k.clone(), src)
            })
            .collect()
    }
}
