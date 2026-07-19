#!/usr/bin/env bash
# Generate placeholder crates for the cortexcode workspace.
# This is a one-off scaffolding script used during the initial migration.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CRATES_DIR="$ROOT/crates"

mkdir -p "$CRATES_DIR"

# name | description | namespace | kind
# kind: leaf | umbrella | root
declare -a CRATES=(
    "cortexcode|Umbrella crate for the cortexcode Rust SDK|root|root"

    "cortexcode-ai|Umbrella crate for the cortex AI namespace|ai|umbrella"
    "cortexcode-ai-env|Environment and API key handling for cortex AI|ai|leaf"
    "cortexcode-ai-images|Image generation and model registry for cortex AI|ai|leaf"
    "cortexcode-ai-models|LLM model registry and discovery for cortex AI|ai|leaf"
    "cortexcode-ai-oauth|OAuth flows for cortex AI providers|ai|leaf"
    "cortexcode-ai-provider-anthropic|Anthropic provider for cortex AI|ai|leaf"
    "cortexcode-ai-provider-azure|Azure OpenAI provider for cortex AI|ai|leaf"
    "cortexcode-ai-provider-faux|Faux / test provider for cortex AI|ai|leaf"
    "cortexcode-ai-provider-google|Google Gemini provider for cortex AI|ai|leaf"
    "cortexcode-ai-provider-openai|OpenAI provider for cortex AI|ai|leaf"
    "cortexcode-ai-stream|Streaming response utilities for cortex AI|ai|leaf"
    "cortexcode-ai-types|Shared types for cortex AI|ai|leaf"
    "cortexcode-ai-util|Shared utilities for cortex AI|ai|leaf"

    "cortexcode-agent|Umbrella crate for the cortex agent namespace|agent|umbrella"
    "cortexcode-agent-core|Core agent runtime for cortex agents|agent|leaf"
    "cortexcode-agent-compaction|Session compaction for cortex agents|agent|leaf"
    "cortexcode-agent-harness|Agent harness for cortex agents|agent|leaf"
    "cortexcode-agent-loop|Agent loop for cortex agents|agent|leaf"
    "cortexcode-agent-mcp|MCP tool integration for cortex agents|agent|leaf"
    "cortexcode-agent-session|Session management for cortex agents|agent|leaf"
    "cortexcode-agent-tools|Built-in tools for cortex agents|agent|leaf"
    "cortexcode-agent-types|Shared types for cortex agents|agent|leaf"

    "cortexcode-code|Umbrella crate for the cortex code namespace|code|umbrella"
    "cortexcode-code-config|Configuration for the cortex coding agent|code|leaf"
    "cortexcode-code-extensions|Extension system for the cortex coding agent|code|leaf"
    "cortexcode-code-main|Main entry point for the cortex coding agent|code|leaf"
    "cortexcode-code-print|Output formatting for the cortex coding agent|code|leaf"
    "cortexcode-code-prompts|Prompt templates for the cortex coding agent|code|leaf"
    "cortexcode-code-resources|Resource management for the cortex coding agent|code|leaf"
    "cortexcode-code-rpc|RPC mode for the cortex coding agent|code|leaf"
    "cortexcode-code-session|Session handling for the cortex coding agent|code|leaf"
    "cortexcode-code-subagents|Subagent orchestration for the cortex coding agent|code|leaf"
    "cortexcode-code-tools|Coding tools for the cortex coding agent|code|leaf"

    "cortexcode-tui|Umbrella crate for the cortex TUI namespace|tui|umbrella"
    "cortexcode-tui-components|UI components for the cortex TUI|tui|leaf"
    "cortexcode-tui-editing|Text editing primitives for the cortex TUI|tui|leaf"
    "cortexcode-tui-fuzzy|Fuzzy matching for the cortex TUI|tui|leaf"
    "cortexcode-tui-images|Terminal image rendering for the cortex TUI|tui|leaf"
    "cortexcode-tui-keys|Keyboard handling for the cortex TUI|tui|leaf"
    "cortexcode-tui-render|Differential rendering for the cortex TUI|tui|leaf"
    "cortexcode-tui-terminal|Terminal abstraction for the cortex TUI|tui|leaf"
    "cortexcode-tui-util|Shared utilities for the cortex TUI|tui|leaf"
)

# Collect leaves per namespace into simple indexed arrays.
AI_LEAVES=()
AGENT_LEAVES=()
CODE_LEAVES=()
TUI_LEAVES=()

for entry in "${CRATES[@]}"; do
    IFS='|' read -r name desc namespace kind <<< "$entry"
    if [ "$kind" = "leaf" ]; then
        case "$namespace" in
            ai) AI_LEAVES+=("$name") ;;
            agent) AGENT_LEAVES+=("$name") ;;
            code) CODE_LEAVES+=("$name") ;;
            tui) TUI_LEAVES+=("$name") ;;
        esac
    fi
done

for entry in "${CRATES[@]}"; do
    IFS='|' read -r name desc namespace kind <<< "$entry"
    dir="$CRATES_DIR/$name"
    mkdir -p "$dir/src"

    # Cargo.toml
    cat > "$dir/Cargo.toml" <<EOF
[package]
name = "$name"
version.workspace = true
authors.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true
description = "$desc"
readme = "README.md"

[package.metadata.cortex]
publish = true

[dependencies]
EOF

    # Add dependencies for umbrella crates.
    if [ "$kind" = "umbrella" ]; then
        case "$namespace" in
            ai)
                for leaf in "${AI_LEAVES[@]}"; do
                    echo "$leaf = { workspace = true }" >> "$dir/Cargo.toml"
                done
                ;;
            agent)
                for leaf in "${AGENT_LEAVES[@]}"; do
                    echo "$leaf = { workspace = true }" >> "$dir/Cargo.toml"
                done
                ;;
            code)
                for leaf in "${CODE_LEAVES[@]}"; do
                    echo "$leaf = { workspace = true }" >> "$dir/Cargo.toml"
                done
                ;;
            tui)
                for leaf in "${TUI_LEAVES[@]}"; do
                    echo "$leaf = { workspace = true }" >> "$dir/Cargo.toml"
                done
                ;;
        esac
    elif [ "$kind" = "root" ]; then
        echo "cortexcode-ai = { workspace = true }" >> "$dir/Cargo.toml"
        echo "cortexcode-agent = { workspace = true }" >> "$dir/Cargo.toml"
        echo "cortexcode-code = { workspace = true }" >> "$dir/Cargo.toml"
        echo "cortexcode-tui = { workspace = true }" >> "$dir/Cargo.toml"
    fi

    # src/lib.rs
    cat > "$dir/src/lib.rs" <<EOF
//! $desc
//!
//! This crate is currently a placeholder reserved for the cortexcode Rust migration.
//! Functionality will be ported from the TypeScript HooCode project incrementally.
EOF

    # README.md
    cat > "$dir/README.md" <<EOF
# $name

$desc

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

This crate is currently a placeholder reserved for the Rust migration from HooCode.
EOF

done

echo "Generated ${#CRATES[@]} crates in $CRATES_DIR"
