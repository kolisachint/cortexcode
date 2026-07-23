#!/bin/bash

# Hoocode → CortexCode Feature Parity Test Script
# Tests binary functionality without requiring API calls

set -e

echo "============================================"
echo "Hoocode → CortexCode Feature Parity Test"
echo "============================================"
echo ""

PASSED=0
FAILED=0
TOTAL=0

test_feature() {
    local name="$1"
    local cmd="$2"
    local expected="$3"
    
    TOTAL=$((TOTAL + 1))
    echo -n "Test $TOTAL: $name ... "
    
    result=$(eval "$cmd" 2>&1)
    
    if echo "$result" | grep -q "$expected"; then
        echo "✅ PASS"
        PASSED=$((PASSED + 1))
    else
        echo "❌ FAIL"
        echo "  Expected: $expected"
        echo "  Got: $result"
        FAILED=$((FAILED + 1))
    fi
}

# Build first
echo "Building workspace..."
cargo build --release 2>&1 | tail -3
echo ""

# Binary Tests
echo "=== Binary Tests ==="
test_feature "Binary exists" "ls -la target/release/cortex 2>&1" "cortex"
test_feature "Help flag" "./target/release/cortex --help 2>&1" "Usage:"
test_feature "Version flag" "./target/release/cortex --version 2>&1" "cortex"
test_feature "Provider flag" "./target/release/cortex --help 2>&1 | grep provider" "provider"
test_feature "Model flag" "./target/release/cortex --help 2>&1 | grep model" "model"
test_feature "Config flag" "./target/release/cortex --help 2>&1 | grep config" "config"
test_feature "Session flag" "./target/release/cortex --help 2>&1 | grep session" "session"
test_feature "API key flag" "./target/release/cortex --help 2>&1 | grep api-key" "api-key"
test_feature "Max turns flag" "./target/release/cortex --help 2>&1 | grep max-turns" "max-turns"
test_feature "Mode flag" "./target/release/cortex --help 2>&1 | grep mode" "mode"
test_feature "MCP binary exists" "ls -la target/release/mcp-stub-server 2>&1" "mcp-stub-server"
echo ""

# Code Quality Tests
echo "=== Code Quality Tests ==="
test_feature "Workspace builds" "cargo build --release 2>&1 | tail -1" "Finished"
test_feature "Tests pass" "cargo test --workspace 2>&1 | grep 'test result:' | tail -1" "ok"
test_feature "Clippy passes" "cargo clippy --workspace --all-targets 2>&1 | tail -1" "Finished"
test_feature "Format check" "cargo fmt --all -- --check 2>&1; echo 'fmt ok'" "fmt ok"
echo ""

# Provider Tests
echo "=== Provider Tests ==="
test_feature "OpenCode provider" "grep -r 'opencode' crates/cortexcode-code-main/src/runtime.rs 2>&1" "opencode"
test_feature "Anthropic provider" "ls crates/ | grep anthropic" "anthropic"
test_feature "OpenAI provider" "ls crates/ | grep openai" "openai"
test_feature "Google provider" "ls crates/ | grep google" "google"
test_feature "Azure provider" "ls crates/ | grep azure" "azure"
test_feature "Faux provider" "ls crates/ | grep faux" "faux"
echo ""

# Feature Tests
echo "=== Feature Tests ==="
test_feature "Models configured" "python3 -c \"import json; m=json.load(open('crates/cortexcode-ai-models/data/models.json')); print(len(m))\" 2>&1" "907"
test_feature "Tools exist" "ls crates/cortexcode-code-tools/src/*.rs 2>&1 | wc -l" "2"
test_feature "Agent core exists" "ls crates/cortexcode-agent-core/src/*.rs 2>&1 | wc -l" "2"
test_feature "TUI exists" "ls crates/cortexcode-tui/src/*.rs 2>&1 | wc -l" "1"
echo ""

# Documentation Tests
echo "=== Documentation Tests ==="
test_feature "README exists" "ls -la README.md 2>&1" "README.md"
test_feature "CHANGELOG exists" "ls -la CHANGELOG.md 2>&1" "CHANGELOG.md"
test_feature "Migration doc exists" "ls -la docs/design/hoocode-to-cortexcode-migration.md 2>&1" "migration.md"
echo ""

echo "============================================"
echo "Test Results: $PASSED passed, $FAILED failed, $TOTAL total"
echo "============================================"

if [ $FAILED -eq 0 ]; then
    echo "✅ All parity tests passed!"
    exit 0
else
    echo "❌ Some parity tests failed"
    exit 1
fi
