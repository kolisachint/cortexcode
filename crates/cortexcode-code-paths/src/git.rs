//! Git source parsing: port of hoocode `utils/git.ts` (`parseGitUrl`) with the
//! parts of `hosted-git-info` 9.0.3 (`fromUrl`) it relies on.
//!
//! Node's `URL` and the `url` crate implement the same WHATWG parser, so the
//! host / path / serialization behavior carries over.

use url::Url;

/// A parsed git package source (`GitSource`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    /// Clone URL, without the ref suffix.
    pub repo: String,
    /// Git host domain, e.g. `github.com`.
    pub host: String,
    /// Repository path, e.g. `user/repo`.
    pub path: String,
    /// Branch, tag or commit when one was given.
    pub git_ref: Option<String>,
    /// True when a ref was given (the package is not auto-updated).
    pub pinned: bool,
}

// ---------------------------------------------------------------------------
// hosted-git-info
// ---------------------------------------------------------------------------

/// A known git host (`hosts.js`): name, domain and accepted protocols.
struct KnownHost {
    name: &'static str,
    domain: &'static str,
    protocols: &'static [&'static str],
}

const HOSTS: &[KnownHost] = &[
    KnownHost {
        name: "github",
        domain: "github.com",
        protocols: &["git:", "http:", "git+ssh:", "git+https:", "ssh:", "https:"],
    },
    KnownHost {
        name: "bitbucket",
        domain: "bitbucket.org",
        protocols: &["git+ssh:", "git+https:", "ssh:", "https:"],
    },
    KnownHost {
        name: "gitlab",
        domain: "gitlab.com",
        protocols: &["git+ssh:", "git+https:", "ssh:", "https:"],
    },
    KnownHost {
        name: "gist",
        domain: "gist.github.com",
        protocols: &["git:", "git+ssh:", "git+https:", "ssh:", "https:"],
    },
    KnownHost {
        name: "sourcehut",
        domain: "git.sr.ht",
        protocols: &["git+ssh:", "https:"],
    },
];

/// `GitHost.#protocols`: protocol -> (default representation name, auth allowed).
fn protocol_info(protocol: &str) -> Option<(Option<&'static str>, bool)> {
    Some(match protocol {
        "git+ssh:" | "ssh:" => (Some("sshurl"), false),
        "git+https:" => (Some("https"), true),
        "git:" | "http:" | "https:" | "git+http:" => (None, true),
        _ => {
            let host = HOSTS
                .iter()
                .find(|h| protocol.strip_suffix(':') == Some(h.name))?;
            (Some(host.name), false)
        }
    })
}

/// What `hostedGitInfo.fromUrl` returns (the fields `parseGitUrl` reads, plus
/// the rest of the constructor arguments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedGitInfo {
    /// Host name: `github`, `gitlab`, `bitbucket`, `gist` or `sourcehut`.
    pub host_type: &'static str,
    pub domain: &'static str,
    /// `None` is JS `null` (a gist without a user, or a shortcut without one).
    pub user: Option<String>,
    pub auth: Option<String>,
    pub project: String,
    /// `None` is JS `null`. A missing ref after `/tree/` is the string
    /// `"undefined"`, as `decodeURIComponent(undefined)` yields in hoocode.
    pub committish: Option<String>,
    pub default_representation: String,
}

fn is_js_whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// `isGitHubShorthand` (from-url.js).
fn is_github_shorthand(arg: &str) -> bool {
    let first_hash = arg.find('#');
    let first_slash = arg.find('/');
    let second_slash = first_slash.and_then(|i| arg[i + 1..].find('/').map(|j| i + 1 + j));
    let first_colon = arg.find(':');
    let first_space = arg.find(is_js_whitespace);
    let first_at = arg.find('@');
    let only_after_hash = |pos: Option<usize>| match pos {
        None => true,
        Some(p) => first_hash.is_some_and(|h| p > h),
    };
    let has_slash = first_slash.is_some_and(|i| i > 0);
    let does_not_end_with_slash = match first_hash {
        // `arg[firstHash - 1] !== '/'` (index -1 is undefined, so true).
        Some(h) => h == 0 || arg.as_bytes()[h - 1] != b'/',
        None => !arg.ends_with('/'),
    };
    only_after_hash(first_space)
        && has_slash
        && does_not_end_with_slash
        && !arg.starts_with('.')
        && only_after_hash(first_at)
        && only_after_hash(first_colon)
        && only_after_hash(second_slash)
}

/// `lastIndexOfBefore(str, char, beforeChar)` (parse-url.js).
fn last_index_of_before(s: &str, ch: char, before: char) -> Option<usize> {
    let end = s.find(before).unwrap_or(s.len());
    s[..end].rfind(ch)
}

/// `correctProtocol` (parse-url.js): `git:github.com:user/repo` -> `git://...`.
fn correct_protocol(arg: &str) -> String {
    let first_colon = arg.find(':');
    let proto = first_colon.map_or("", |c| &arg[..=c]);
    if protocol_info(proto).is_some() {
        return arg.to_string();
    }
    if first_colon.is_some_and(|c| arg[c..].starts_with("://")) {
        return arg.to_string();
    }
    if let Some(first_at) = arg.find('@') {
        return if first_colon.is_none_or(|c| first_at > c) {
            format!("git+ssh://{arg}")
        } else {
            arg.to_string()
        };
    }
    match first_colon {
        Some(c) => format!("{}//{}", &arg[..=c], &arg[c + 1..]),
        None => format!("//{arg}"),
    }
}

/// `correctUrl` (parse-url.js): make an scp-style URL parse with `new URL()`.
fn correct_url(giturl: &str) -> String {
    let mut giturl = giturl.to_string();
    let first_at = last_index_of_before(&giturl, '@', '#');
    let last_colon = last_index_of_before(&giturl, ':', '#');
    if let Some(colon) = last_colon {
        if first_at.is_none_or(|at| colon > at) {
            giturl = format!("{}/{}", &giturl[..colon], &giturl[colon + 1..]);
        }
    }
    if last_index_of_before(&giturl, ':', '#').is_none() && !giturl.contains("//") {
        giturl = format!("git+ssh://{giturl}");
    }
    giturl
}

fn parse_url(giturl: &str) -> Option<Url> {
    let with_protocol = correct_protocol(giturl);
    Url::parse(&with_protocol)
        .ok()
        .or_else(|| Url::parse(&correct_url(&with_protocol)).ok())
}

/// JS `decodeURIComponent`; `None` where it throws `URIError`.
fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `url.hash`: `#` plus the fragment, or empty when the fragment is empty.
fn url_hash(url: &Url) -> String {
    match url.fragment() {
        Some(f) if !f.is_empty() => format!("#{f}"),
        _ => String::new(),
    }
}

/// `url.protocol`, with its trailing colon.
fn url_protocol(url: &Url) -> String {
    format!("{}:", url.scheme())
}

/// JS `str.split('/', limit)`, indexed like an array (`None` = `undefined`).
fn split_limit(s: &str, limit: usize) -> Vec<&str> {
    s.split('/').take(limit).collect()
}

struct Segments {
    user: Option<String>,
    project: String,
    committish: Option<String>,
}

fn strip_git(project: &str) -> &str {
    project.strip_suffix(".git").unwrap_or(project)
}

/// The per-host `extract(url)` functions (hosts.js).
fn extract(host: &str, url: &Url) -> Option<Segments> {
    let pathname = url.path();
    let hash = url_hash(url);
    let hash_ref = || Some(hash.get(1..).unwrap_or("").to_string());
    match host {
        "github" => {
            let parts = split_limit(pathname, 5);
            let user = parts.get(1).copied().unwrap_or("");
            let project = parts.get(2).copied().unwrap_or("");
            let kind = parts.get(3).copied().unwrap_or("");
            if !kind.is_empty() && kind != "tree" {
                return None;
            }
            let committish = if kind.is_empty() {
                hash_ref()
            } else {
                parts.get(4).map(|s| s.to_string())
            };
            let project = strip_git(project);
            if user.is_empty() || project.is_empty() {
                return None;
            }
            Some(Segments {
                user: Some(user.into()),
                project: project.into(),
                committish,
            })
        }
        "bitbucket" | "sourcehut" => {
            let parts = split_limit(pathname, 4);
            let user = parts.get(1).copied().unwrap_or("");
            let project = strip_git(parts.get(2).copied().unwrap_or(""));
            let blocked = if host == "bitbucket" {
                "get"
            } else {
                "archive"
            };
            if parts.get(3) == Some(&blocked) {
                return None;
            }
            if user.is_empty() || project.is_empty() {
                return None;
            }
            Some(Segments {
                user: Some(user.into()),
                project: project.into(),
                committish: hash_ref(),
            })
        }
        "gitlab" => {
            let path = pathname.get(1..).unwrap_or("");
            if path.contains("/-/") || path.contains("/archive.tar.gz") {
                return None;
            }
            let (user, project) = match path.rfind('/') {
                Some(i) => (&path[..i], &path[i + 1..]),
                None => ("", path),
            };
            let project = strip_git(project);
            if user.is_empty() || project.is_empty() {
                return None;
            }
            Some(Segments {
                user: Some(user.into()),
                project: project.into(),
                committish: hash_ref(),
            })
        }
        "gist" => {
            let parts = split_limit(pathname, 4);
            if parts.get(3) == Some(&"raw") {
                return None;
            }
            let mut user = parts.get(1).map(|s| s.to_string());
            let project = match parts.get(2).filter(|p| !p.is_empty()) {
                Some(p) => p.to_string(),
                None => user.take().filter(|u| !u.is_empty())?,
            };
            Some(Segments {
                user,
                project: strip_git(&project).into(),
                committish: hash_ref(),
            })
        }
        _ => None,
    }
}

/// `hostedGitInfo.fromUrl(giturl)`.
pub fn hosted_git_info_from_url(giturl: &str) -> Option<HostedGitInfo> {
    if giturl.is_empty() {
        return None;
    }
    let corrected = if is_github_shorthand(giturl) {
        format!("github:{giturl}")
    } else {
        giturl.to_string()
    };
    let parsed = parse_url(&corrected)?;
    let protocol = url_protocol(&parsed);
    let hostname = parsed.host_str().unwrap_or("");
    let hostname = hostname.strip_prefix("www.").unwrap_or(hostname);

    let by_shortcut = HOSTS
        .iter()
        .find(|h| protocol.strip_suffix(':') == Some(h.name));
    let host = by_shortcut.or_else(|| HOSTS.iter().find(|h| h.domain == hostname))?;

    let auth_allowed = protocol_info(&protocol).is_some_and(|(_, auth)| auth);
    let username = parsed.username();
    let password = parsed.password().unwrap_or("");
    let auth = (auth_allowed && (!username.is_empty() || !password.is_empty())).then(|| {
        if password.is_empty() {
            username.to_string()
        } else {
            format!("{username}:{password}")
        }
    });

    let (user, project, committish, default_representation);
    if by_shortcut.is_some() {
        let pathname = parsed.path();
        let mut pathname = pathname.strip_prefix('/').unwrap_or(pathname);
        // Auth is ignored for shortcuts, so trim it out.
        if let Some(at) = pathname.find('@') {
            pathname = &pathname[at + 1..];
        }
        let (u, p) = match pathname.rfind('/') {
            Some(i) => {
                let u = decode_uri_component(&pathname[..i])?;
                (
                    Some(u).filter(|u| !u.is_empty()),
                    decode_uri_component(&pathname[i + 1..])?,
                )
            }
            None => (None, decode_uri_component(pathname)?),
        };
        user = u;
        project = strip_git(&p).to_string();
        let hash = url_hash(&parsed);
        committish = if hash.is_empty() {
            None
        } else {
            Some(decode_uri_component(&hash[1..])?)
        };
        default_representation = "shortcut".to_string();
    } else {
        if !host.protocols.contains(&protocol.as_str()) {
            return None;
        }
        let segments = extract(host.name, &parsed)?;
        user = match segments.user {
            Some(u) if !u.is_empty() => Some(decode_uri_component(&u)?),
            other => other,
        };
        project = decode_uri_component(&segments.project)?;
        committish = Some(match segments.committish {
            Some(c) => decode_uri_component(&c)?,
            None => "undefined".to_string(),
        });
        default_representation = protocol_info(&protocol)
            .and_then(|(name, _)| name)
            .map(str::to_string)
            .unwrap_or_else(|| protocol[..protocol.len() - 1].to_string());
    }

    Some(HostedGitInfo {
        host_type: host.name,
        domain: host.domain,
        user,
        auth,
        project,
        committish,
        default_representation,
    })
}

// ---------------------------------------------------------------------------
// utils/git.ts
// ---------------------------------------------------------------------------

struct SplitRef {
    repo: String,
    git_ref: Option<String>,
}

/// `^git@([^:]+):(.+)$`.
fn scp_like(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("git@")?;
    let colon = rest.find(':')?;
    let (host, path) = (&rest[..colon], &rest[colon + 1..]);
    if host.is_empty() || path.is_empty() || path.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    Some((host, path))
}

fn split_at_ref(path_with_ref: &str) -> Option<(&str, &str)> {
    let at = path_with_ref.find('@')?;
    let (repo_path, git_ref) = (&path_with_ref[..at], &path_with_ref[at + 1..]);
    (!repo_path.is_empty() && !git_ref.is_empty()).then_some((repo_path, git_ref))
}

fn split_ref(url: &str) -> SplitRef {
    let unsplit = || SplitRef {
        repo: url.to_string(),
        git_ref: None,
    };
    if let Some((host, path)) = scp_like(url) {
        return match split_at_ref(path) {
            Some((repo_path, git_ref)) => SplitRef {
                repo: format!("git@{host}:{repo_path}"),
                git_ref: Some(git_ref.to_string()),
            },
            None => unsplit(),
        };
    }

    if url.contains("://") {
        let Ok(mut parsed) = Url::parse(url) else {
            return unsplit();
        };
        let path = parsed.path().trim_start_matches('/').to_string();
        let Some((repo_path, git_ref)) = split_at_ref(&path) else {
            return unsplit();
        };
        let git_ref = git_ref.to_string();
        parsed.set_path(&format!("/{repo_path}"));
        let serialized = parsed.to_string();
        return SplitRef {
            repo: serialized
                .strip_suffix('/')
                .unwrap_or(&serialized)
                .to_string(),
            git_ref: Some(git_ref),
        };
    }

    let Some(slash) = url.find('/') else {
        return unsplit();
    };
    let (host, path) = (&url[..slash], &url[slash + 1..]);
    match split_at_ref(path) {
        Some((repo_path, git_ref)) => SplitRef {
            repo: format!("{host}/{repo_path}"),
            git_ref: Some(git_ref.to_string()),
        },
        None => unsplit(),
    }
}

fn parse_generic_git_url(url: &str) -> Option<GitSource> {
    let SplitRef {
        repo: repo_without_ref,
        git_ref,
    } = split_ref(url);
    let mut repo = repo_without_ref.clone();
    let host;
    let path;
    if let Some((h, p)) = scp_like(&repo_without_ref) {
        host = h.to_string();
        path = p.to_string();
    } else if ["https://", "http://", "ssh://", "git://"]
        .iter()
        .any(|p| repo_without_ref.starts_with(p))
    {
        let parsed = Url::parse(&repo_without_ref).ok()?;
        host = parsed.host_str().unwrap_or("").to_string();
        path = parsed.path().trim_start_matches('/').to_string();
    } else {
        let slash = repo_without_ref.find('/')?;
        host = repo_without_ref[..slash].to_string();
        path = repo_without_ref[slash + 1..].to_string();
        if !host.contains('.') && host != "localhost" {
            return None;
        }
        repo = format!("https://{repo_without_ref}");
    }

    let normalized = strip_git(&path).trim_start_matches('/').to_string();
    if host.is_empty() || normalized.is_empty() || normalized.split('/').count() < 2 {
        return None;
    }
    Some(GitSource {
        repo,
        host,
        path: normalized,
        pinned: git_ref.is_some(),
        git_ref,
    })
}

/// `^(https?|ssh|git):\/\/` (case-insensitive).
fn has_protocol_prefix(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    ["http://", "https://", "ssh://", "git://"]
        .iter()
        .any(|p| lower.starts_with(p))
}

fn hosted_source(info: &HostedGitInfo, split: &SplitRef, repo: String) -> GitSource {
    let user = info.user.as_deref().unwrap_or("null");
    let path = format!("{user}/{}", info.project);
    let git_ref = info
        .committish
        .clone()
        .filter(|c| !c.is_empty())
        .or_else(|| split.git_ref.clone());
    GitSource {
        repo,
        host: info.domain.to_string(),
        path: strip_git(&path).to_string(),
        pinned: git_ref.is_some(),
        git_ref,
    }
}

/// `parseGitUrl`: with a `git:` prefix every historical shorthand is
/// accepted; without it only explicit protocol URLs are.
pub fn parse_git_url(source: &str) -> Option<GitSource> {
    let trimmed = source.trim_matches(is_js_whitespace);
    let has_git_prefix = trimmed.starts_with("git:");
    let url = if has_git_prefix {
        trimmed[4..].trim_matches(is_js_whitespace)
    } else {
        trimmed
    };
    if !has_git_prefix && !has_protocol_prefix(url) {
        return None;
    }

    let split = split_ref(url);

    let mut hosted_candidates = Vec::new();
    if let Some(r) = &split.git_ref {
        hosted_candidates.push(format!("{}#{r}", split.repo));
    }
    hosted_candidates.push(url.to_string());
    for candidate in hosted_candidates.iter().filter(|c| !c.is_empty()) {
        if let Some(info) = hosted_git_info_from_url(candidate) {
            if split.git_ref.is_some() && info.project.contains('@') {
                continue;
            }
            let use_https_prefix = !["http://", "https://", "ssh://", "git://", "git@"]
                .iter()
                .any(|p| split.repo.starts_with(p));
            let repo = if use_https_prefix {
                format!("https://{}", split.repo)
            } else {
                split.repo.clone()
            };
            return Some(hosted_source(&info, &split, repo));
        }
    }

    let mut https_candidates = Vec::new();
    if let Some(r) = &split.git_ref {
        https_candidates.push(format!("https://{}#{r}", split.repo));
    }
    https_candidates.push(format!("https://{url}"));
    for candidate in &https_candidates {
        if let Some(info) = hosted_git_info_from_url(candidate) {
            if split.git_ref.is_some() && info.project.contains('@') {
                continue;
            }
            return Some(hosted_source(
                &info,
                &split,
                format!("https://{}", split.repo),
            ));
        }
    }

    parse_generic_git_url(url)
}
