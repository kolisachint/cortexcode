//! `core/git-branch.ts`: locate a repo's git metadata and read its branch
//! (the footer shows it; a session records the one it started on).

use std::path::{Path, PathBuf};

/// `GitPaths`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPaths {
    pub repo_dir: PathBuf,
    pub common_git_dir: PathBuf,
    pub head_path: PathBuf,
}

/// Node's `path.resolve`: absolute, with `.` and `..` resolved lexically.
fn resolve(path: PathBuf) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in std::path::absolute(path).ok()?.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    Some(out)
}

/// `findGitPaths`: walk up from `cwd` to a `.git` directory, or a `.git`
/// file (worktree) pointing at one.
pub fn find_git_paths(cwd: &Path) -> Option<GitPaths> {
    let mut dir = cwd.to_path_buf();
    loop {
        let git_path = dir.join(".git");
        if git_path.exists() {
            let meta = std::fs::metadata(&git_path).ok()?;
            if meta.is_file() {
                let content = std::fs::read_to_string(&git_path).ok()?;
                if let Some(rest) = content.trim().strip_prefix("gitdir: ") {
                    let git_dir = resolve(dir.join(rest.trim()))?;
                    let head_path = git_dir.join("HEAD");
                    if !head_path.exists() {
                        return None;
                    }
                    let common_dir_path = git_dir.join("commondir");
                    let common_git_dir = if common_dir_path.exists() {
                        let common = std::fs::read_to_string(&common_dir_path).ok()?;
                        resolve(git_dir.join(common.trim()))?
                    } else {
                        git_dir
                    };
                    return Some(GitPaths {
                        repo_dir: dir,
                        common_git_dir,
                        head_path,
                    });
                }
            } else if meta.is_dir() {
                let head_path = git_path.join("HEAD");
                if !head_path.exists() {
                    return None;
                }
                return Some(GitPaths {
                    repo_dir: dir,
                    common_git_dir: git_path,
                    head_path,
                });
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// `resolveBranchWithGitSync`: ask git (for reftable repos).
pub fn resolve_branch_with_git(repo_dir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args([
            "--no-optional-locks",
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .current_dir(repo_dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !branch.is_empty()).then_some(branch)
}

/// `readBranchFromHead`: the branch, `"detached"`, or `None` when HEAD is unreadable.
pub fn read_branch_from_head(paths: &GitPaths) -> Option<String> {
    let content = std::fs::read_to_string(&paths.head_path).ok()?;
    match content.trim().strip_prefix("ref: refs/heads/") {
        Some(".invalid") => {
            Some(resolve_branch_with_git(&paths.repo_dir).unwrap_or_else(|| "detached".into()))
        }
        Some(branch) => Some(branch.to_string()),
        None => Some("detached".into()),
    }
}

/// `readGitBranch`: the branch `cwd` is on; `None` outside a repo or detached.
pub fn read_git_branch(cwd: &Path) -> Option<String> {
    let paths = find_git_paths(cwd)?;
    read_branch_from_head(&paths).filter(|b| b != "detached")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_branches_worktrees_and_detached_heads() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(
            read_git_branch(&repo.join("src/deep")).as_deref(),
            Some("feature/x")
        );

        std::fs::write(repo.join(".git/HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(read_git_branch(&repo), None);

        // A worktree: `.git` is a file naming the gitdir, with a commondir.
        let wt_git = repo.join(".git/worktrees/wt");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt_git.join("HEAD"), "ref: refs/heads/wt-branch\n").unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        let paths = find_git_paths(&wt).unwrap();
        assert_eq!(paths.repo_dir, wt);
        assert_eq!(paths.head_path, wt_git.join("HEAD"));
        assert_eq!(paths.common_git_dir, repo.join(".git"));
        assert_eq!(read_git_branch(&wt).as_deref(), Some("wt-branch"));

        assert_eq!(read_git_branch(&tmp.path().join("nowhere")), None);
    }
}
