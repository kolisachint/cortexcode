# Pending CI changes (apply manually)

The session's GitHub token can't modify `.github/workflows/`, so these changes are
staged here. To apply them:

```bash
git apply migration/ci/ci.yml.patch                     # ledger + dependency-firewall checks in CI
cp migration/ci/tui-parity.yml .github/workflows/       # manual/nightly Level-2 parity job
```

Tracked by ledger tasks 7.1 (CI gates) and 13.3 (parity workflow).
