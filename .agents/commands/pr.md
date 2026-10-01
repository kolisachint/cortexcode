---
description: Branch, commit, push and open a labelled PR (CI releases on merge)
argument-hint: "<patch|minor|major|none> [crates] [fast] [draft] [auto] [title=\"...\"]"
---
Ship the current work as a GitHub pull request. Do every step yourself with
`git` and `gh`; only stop where this says to ask. Arguments: `$ARGUMENTS`

## Arguments

| Arg | Meaning | PR label |
|---|---|---|
| `patch` / `minor` / `major` | release level, **required** (first arg) | `rust:<level>` |
| `none` | merge without a release | (no release label) |
| `crates` | deferred: crates.io publishing is off, the label only leaves a notice | `release:crates` |
| `fast` | release skips CI gates (PR CI already ran) | `release:skip-gates` |
| `draft` | open as a draft PR | — |
| `auto` | enable auto-merge (merge commit) once checks pass | — |
| `title="..."` | PR title; otherwise derive it from the commits | — |

These labels drive `.github/workflows/merge-release.yml`. If the level is
missing or not one of the five values, stop and show the usage line.

## Steps

1. **Preflight.**
   - `gh auth status` must pass; `git remote get-url origin` must exist.
   - Base branch is the repo default: `gh repo view --json defaultBranchRef -q .defaultBranchRef.name`.
   - Stop if a merge, rebase or cherry-pick is in progress.
2. **Branch.**
   - If on the base branch, create a new branch with `git switch -c <name>`.
     Name it `<type>/<short-kebab-summary>` from the changes
     (`feat/`, `fix/`, `ci/`, `docs/`, `chore/`, `refactor/`).
   - Otherwise keep the current branch.
3. **Commit.**
   - Show `git status -s`. Stage everything with `git add -A`.
   - Refuse to commit secrets or junk: `.env*`, keys, tokens, `target/`,
     large binaries. Unstage them and tell the user.
   - If anything is staged, commit with a Conventional Commit message
     (`type(scope): summary` + short body of what and why) based on `git diff --cached`.
   - If nothing is staged and the branch has no commits ahead of base, stop:
     nothing to ship.
4. **Sync with base.**
   - `git fetch origin`. If base moved, `git rebase origin/<base>`.
   - On conflicts: `git rebase --abort`, stop, list the files.
5. **Local checks** (Rust changes only, skip for docs/CI-only diffs):
   - `cargo fmt --all -- --check` — if it fails, run `cargo fmt --all` and commit as `style: cargo fmt`.
   - `cargo clippy --workspace --all-targets -- -D warnings`.
   - If clippy fails, stop and report. Do not push red code.
6. **Push.** `git push -u origin HEAD` (use `--force-with-lease` only if step 4 rebased an already-pushed branch).
7. **Labels.** Create any that are missing (`gh label create ... --force` is idempotent):
   - `rust:patch` `rust:minor` `rust:major` — colour `0E8A16`, "Release level on merge"
   - `release:crates` — `1D76DB`, "Also publish to crates.io"
   - `release:skip-gates` — `FBCA04`, "Release skips CI gates"
8. **PR.**
   - If a PR for this branch already exists (`gh pr view --json number,url`),
     update it: replace any `rust:*` label with the new one and add the flag labels.
   - Otherwise `gh pr create --base <base> --title ... --body ...` (add `--draft` if asked),
     with `--label` for each label.
   - Body: summary bullets, the release level, and a `## Test plan` section.
9. **Auto-merge** (only with `auto`): `gh pr merge --auto --merge`.
10. **Report** in a short list:
    - branch, commit(s), PR URL
    - labels applied, and what will happen on merge
      (e.g. "merging releases v0.X.Y+1 → binaries")
    - next step: `/postmerge` after the PR is merged.

Never merge the PR yourself unless `auto` was given. Never push to the base branch directly.
