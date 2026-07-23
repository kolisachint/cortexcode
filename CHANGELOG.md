# Changelog

All notable changes to CortexCode will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
