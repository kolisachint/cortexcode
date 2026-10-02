# Changelog

All notable changes to CortexCode will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed: subagents stopped failing on almost every dispatch (2026-10-03)
- Subagents ran to completion in none of ten recorded dispatches. Seven fixes, detailed in
  `docs/design/subagents.md`.
- **Every subagent was killed after 5 minutes.** The per-agent timeout table was keyed on
  `edit`, `test` and `review` — agents this project does not ship — so no entry ever matched
  and every agent fell through to the 5-minute default. `code-review` died at exactly 300s
  three times holding up to 297s of finished work. Real per-agent deadlines now exist
  (`code-review`/`security-review` 15 min, `general-purpose` 20, `explore`/`plan` 10), and
  the warm worker pool's flat 3-minute limit follows the same table.
- **A subagent killed at its deadline lost everything it had finished.** `result.json` is
  written only after the run returns cleanly, so the kill discarded it. A child is now told
  its deadline up front and asked to wrap up shortly before it, so a run cut short reports
  its findings as a usable partial result instead of vanishing.
- **A subagent that died because its model was unreachable never retried on a reachable one.**
  Timed-out and stalled runs skipped the inherited-model fallback entirely, and the error
  classifier did not recognise region rejections (`requires Global regions`) or
  `finish_reason: error`. Both fixed; a cancelled run is still never retried.
- **The inherited-model fallback could silently do nothing.** When a caller passed a
  `complexity` tier, the fallback re-resolved that same tier and retried the identical model.
- **Reported token usage was wildly wrong.** The subagent budget summed each turn's whole
  context size, so a 15-turn run reported 1.6M tokens against a 35k budget. It now counts what
  the subagent generated; context size is reported separately as `peak_context`.
- A failed dispatch wrote a 256 KB `output.json` that duplicated the transcript already in
  `session.jsonl`. It now holds the outcome, the cause, and a short tail of stderr.

### Release: npm publish is now verified after it runs (2026-10-02)
- The npm publish loop moved from an inline shell loop in `binaries.yml` to
  `scripts/npm/publish_packages.py`, which publishes, then checks every package
  against the registry packument and fails the job if any is missing. A fresh
  publish is readable only after registry propagation — after v0.1.5, `darwin-x64`
  took ~11 minutes to appear — so a partial publish used to look exactly like a
  successful one. Re-runs still skip published versions, so a failed release is
  recovered by re-running `binaries.yml` for the same tag.
- `--verify-only` checks a published release without publishing, and reads the
  packument with cache busting (npm's CDN otherwise serves stale versions).

### Fixed: compaction and branch summaries on OpenCode Go (2026-10-02)
- Compaction (`/compact`, automatic context compaction, branch summaries) builds its own
  summarization request, which did not carry the session id, so OpenCode Go rejected it with
  `400 Request is missing x-opencode-session and cannot be routed efficiently`. Normal turns
  were unaffected — they already sent the header. Summarization now passes the session id
  through on every request (chat completions, responses, Anthropic Messages), and it is still
  omitted when there is no session. Two tests cover both directions.

### Pin bumped to hoocode v0.6.0 (2026-10-01)
- Model catalog regenerated from hoocode v0.6.0 (1228 models). It adds
  `anthropic/claude-sonnet-5-5`, `opencode-go/longcat-2.5-preview-free`, `opencode/gpt-6.1-sol`
  and others, and drops models upstream removed (e.g. `opencode-go/kimi-k2.6`).
- OpenCode Go: every request sends `user-agent: hoocode` and, when a session exists,
  `x-opencode-session` (Chat Completions, Responses and Anthropic Messages), even with prompt
  caching off. Go rejected requests without it: `400 Request is missing x-opencode-session`.
- Default models for `opencode-go`, `fireworks` and `together` are now Kimi K3. A test now
  fails when any provider default drops out of the catalog.
- Bundled theme JSONs match the pin (a test checks them against the pinned checkout).
- `migration/pin_drift.py` reports when upstream has released past the pin and prints the
  port checklist for a bump. `setup_hoocode.sh` installs with the pin's declared bun version.

### Migration paused, `cortex` ready for use (2026-10-01)
- Every remaining ledger task is deferred by user decision: MCP (9.1, 10.11),
  webfetch/websearch (10.2e), the TS test ledger close-out (13.4) and phase 12. See the
  README's "Migration status" and §0.3 of the migration plan for what works and what doesn't.
- Interactive agent modes: `/mode`, `/plan`, `/grill`, `/approve` now act in the TUI. They
  notify, reload with the new mode's prompt and tool filter, and update the footer badge. alt+a
  cycles ask → plan → build → debug, and mode commands appear in autocomplete with
  argument completions (10.5e).
- Earlier in this cycle: phase 11 (the interactive TUI) is complete. That covers the chat
  view, selectors, input features, task panel, key bindings, thinking toggle and the
  external editor.

### Migration re-baseline (2026-09-24)
- Pinned the port to hoocode v0.5.89 (`a6cd96e7`). An audit found the earlier
  "100% complete" status overstated, and progress is now tracked in
  `migration/ledger.json` (see `docs/design/hoocode-to-cortexcode-migration.md` §0).
- Added two-level done gates: Level 1 = cargo checks, Level 2 = rendered-TUI parity
  against hoocode via `migration/tui-parity/`.
- Removed the stale status reports (`MIGRATION_*.md`, `TEST_RECORD.md`,
  `E2E_TESTING_SUMMARY.md`, `OPENCODE_E2E_TEST_REPORT.md`, `QUICKSTART.md`).
- Live OpenCode checks moved to `scripts/live/` and
  `crates/cortexcode-ai-provider-openai/tests/opencode_live.rs` (`#[ignore]`d). The API
  key now comes only from `OPENCODE_API_KEY`. A key previously committed in these
  files must be rotated.

### Added
- OpenCode API provider support (opencode, opencode-go)
- mimo-v2.5-free as default model for OpenCode provider
- Comprehensive E2E testing framework
- Vertex AI Application Default Credentials (ADC) support
- Service account JWT authentication for Google Cloud
- GCE/GKE metadata server authentication
- Windows virtual terminal input tweaks
- Paste-marker compression in editor
- Vim-style character jump (f/F) in editor
- Interactive OAuth login (Anthropic, GitHub Copilot)
- Permission gates for tool call approval
- Auto-migration from legacy ~/.hoocode settings
- Explicit --config CLI override
- Reasoning-item ID pairing in Azure provider

### Changed
- Improved provider configuration and model selection
- Enhanced error handling and user feedback
- Optimized streaming performance
- Updated documentation and test coverage

### Fixed
- Cargo fmt formatting issues
- Clippy warnings resolved
- Windows console input handling
- Context preservation in multi-turn conversations

## [0.1.0] - 2026-07-23

### Added
- Initial release of CortexCode
- Rust migration from HooCode TypeScript framework
- 43 crates in workspace structure
- 640 tests passing
- Support for 6 LLM providers:
  - Anthropic (Claude)
  - OpenAI (GPT)
  - OpenCode (MiMo, DeepSeek, etc.)
  - Google (Gemini)
  - Azure OpenAI
  - Faux (testing)
- CLI with print and interactive modes
- JSON-RPC server mode
- Subagent pool and Task tool
- Core tools (read, write, edit, bash, grep, find, ls)
- MCP (Model Context Protocol) support
- WASM plugin extensions
- Session persistence and management
- Context compaction
- Cross-platform binary builds (Linux, macOS, Windows)
- GitHub Actions CI/CD pipeline
- crates.io publishing workflow

### Technical Details
- **Build System:** Cargo workspace with lockstep versioning
- **Minimum Rust Version:** 1.78
- **Test Coverage:** 640 tests across 43 crates
- **Clippy Warnings:** 0
- **CI Status:** All checks passing

### Supported Platforms
- Linux (x86_64)
- macOS (Intel, Apple Silicon)
- Windows (x86_64)

### Providers
| Provider | Models | Status |
|----------|--------|--------|
| Anthropic | Claude 3.5, Claude 4 | ✅ |
| OpenAI | GPT-4, GPT-4o | ✅ |
| OpenCode | MiMo, DeepSeek, Claude | ✅ |
| Google | Gemini 2.5 | ✅ |
| Azure | GPT-4, GPT-4o | ✅ |
| Faux | Test models | ✅ |

---

## Release Notes

### v0.1.0 - Initial Release

CortexCode is a Rust migration of the HooCode TypeScript coding-agent framework. This release includes:

**Core Features:**
- Full LLM runtime with streaming support
- Tool execution and permission gates
- Session management and persistence
- MCP (Model Context Protocol) support
- Interactive TUI with vim-style editing

**Providers:**
- Anthropic (Claude 3.5, Claude 4)
- OpenAI (GPT-4, GPT-4o)
- OpenCode (MiMo, DeepSeek, Claude, Gemini)
- Google (Gemini 2.5)
- Azure OpenAI
- Faux (testing)

**CLI Modes:**
- Print mode (text/JSON output)
- Interactive TUI mode
- JSON-RPC server mode
- Subagent mode

**Development:**
- 43 crates in workspace structure
- 640 tests passing
- Zero clippy warnings
- CI/CD with GitHub Actions
- Cross-platform binary builds

**Installation:**
```bash
# From source
cargo install --path crates/cortexcode-code-main --bin cortex

# Pre-built binaries
# Download from GitHub Releases
```

**Usage:**
```bash
# Single-shot mode
cortex -p "Explain this codebase"

# Interactive mode
cortex

# With specific provider
cortex --provider opencode --model mimo-v2.5-free -p "Hello"
```

---

## Migration Status

The migration from HooCode TypeScript to CortexCode Rust is **98% complete**.

**Completed:**
- ✅ TUI Namespace (120 tests)
- ✅ AI Namespace (155 tests)
- ✅ Agent Namespace (57 tests)
- ✅ Code Namespace (88 tests)
- ✅ OpenCode API Provider (22+ tests)
- ✅ All 6 Providers Working
- ✅ 640 Tests Passing
- ✅ Zero Clippy Warnings
- ✅ CI/CD Pipeline Working

**Remaining (Deferred):**
- ⏳ Full parity testing with scripted hoocode scenarios
- ⏳ Release notes and changelog (this document)

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on how to contribute to CortexCode.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
