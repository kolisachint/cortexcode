---
description: After a PR merges - verify CI and release, switch to main, delete the branch
argument-hint: "[PR-number] [nowait] [keep]"
---
Finish up after a pull request is merged. Do every step yourself with `git`
and `gh`. Arguments: `$ARGUMENTS`

## Arguments

| Arg | Meaning |
|---|---|
| `<number>` | PR to check; otherwise the PR for the current branch |
| `nowait` | report run status once, don't wait for runs to finish |
| `keep` | don't delete the branch |

## Steps

1. **Find the PR.**
   - `gh pr view <number or current branch> --json number,title,state,mergedAt,mergeCommit,headRefName,baseRefName,labels,url`.
   - If not `MERGED`, stop. Show its state and the failing checks (`gh pr checks`).
   - Note the release level from the `rust:*` label (or "no release").
2. **CI on the base branch.**
   - Find runs for the merge commit: `gh run list --commit <mergeCommit sha> --json databaseId,workflowName,status,conclusion,url`.
   - Wait for each with `gh run watch <id> --exit-status` (skip waiting with `nowait`).
   - On failure: show the failed job and the last ~40 log lines
     (`gh run view <id> --log-failed | tail -40`). Keep going to collect the full picture,
     but do not delete anything in step 5.
3. **Release** (only if a `rust:*` label was set).
   - The "Merge Release" run (triggered by `pull_request: closed`) calls `release.yml`:
     gates → bump + tag + GitHub release → binaries for 4 targets (+ crates if `release:crates`).
   - Wait for it like step 2.
   - Then verify:
     - newest tag: `git fetch --tags origin`, `gh release list --limit 1`
     - release assets: `gh release view <tag> --json assets -q '.assets[].name'` —
       expect 4 archives (linux x86_64, macOS x86_64, macOS aarch64, windows zip).
     - version bumped: `Release <tag>` commit is on `origin/<base>`.
     - crates (only with `release:crates`): the publish job succeeded.
4. **Move to base.**
   - If the working tree is dirty, stop and ask; never stash or discard silently.
   - `git switch <base>` then `git pull --ff-only origin <base>` (picks up the release commit).
5. **Delete the merged branch** (skip with `keep`, or if anything failed above).
   - Never delete `main` / the base branch.
   - Local: `git branch -d <head>`. A merge commit makes `-d` work; if git refuses
     (squash merge), use `-D` only because the PR is confirmed `MERGED`.
   - Remote: `git push origin --delete <head>` if it still exists.
   - `git fetch --prune origin`.
6. **Report**, short lines, a word not just a symbol for each status:
   - PR: number, title, merged at
   - CI: PASS / FAIL per workflow, with link on failure
   - Release: tag, assets found (n/4), crates PASS / FAIL / skipped
   - Git: now on `<base>` at `<short sha>`, branch deleted (local, remote) or kept
   - Anything that needs the user, with the exact command to fix it.
