# OpenCode API E2E Testing - Quick Start Guide

## Overview

This guide shows you how to run end-to-end tests for the OpenCode API with the mimo-v2.5-free model.

## Prerequisites

- macOS/Linux environment
- cURL installed
- Python 3 installed (for response parsing)
- OpenCode API key

## Quick Start

### 1. Set Your API Key

```bash
export OPENCODE_API_KEY="sk-YbD8JYT7pQuD1gEd8Gx5f4qR6itxCdqpjlnuLy0nqiU9FUMLQKTJumJmU3ouq62Q"
```

### 2. Run Basic Tests

```bash
chmod +x run_e2e_tests.sh
./run_e2e_tests.sh
```

### 3. Run Advanced Tests

```bash
chmod +x run_advanced_tests.sh
./run_advanced_tests.sh
```

## Test Suites

### Basic E2E Tests (`run_e2e_tests.sh`)

Tests core functionality:
- Provider configuration
- Model availability
- API connectivity
- Text completion
- Streaming
- Tool usage
- Performance

**Duration**: ~30 seconds
**Tests**: 10

### Advanced Tests (`run_advanced_tests.sh`)

Tests complex scenarios:
- Multi-turn conversations
- Complex tool chains
- Code generation
- Mathematical reasoning
- Creative writing
- Instruction following
- Error handling
- Token tracking
- High token limits
- Concurrent requests
- Context windows

**Duration**: ~60 seconds
**Tests**: 12

## Manual Testing

### Simple Completion

```bash
curl -X POST "https://opencode.ai/zen/v1/chat/completions" \
  -H "Authorization: Bearer $OPENCODE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "mimo-v2.5-free",
    "messages": [
      {"role": "user", "content": "What is 2+2?"}
    ],
    "max_tokens": 100,
    "stream": false
  }'
```

### Streaming

```bash
curl -X POST "https://opencode.ai/zen/v1/chat/completions" \
  -H "Authorization: Bearer $OPENCODE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "mimo-v2.5-free",
    "messages": [
      {"role": "user", "content": "Count to 10"}
    ],
    "max_tokens": 100,
    "stream": true
  }'
```

### Tool Usage

```bash
curl -X POST "https://opencode.ai/zen/v1/chat/completions" \
  -H "Authorization: Bearer $OPENCODE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "mimo-v2.5-free",
    "messages": [
      {"role": "user", "content": "Read the file at /tmp/test.txt"}
    ],
    "tools": [
      {
        "type": "function",
        "function": {
          "name": "read_file",
          "description": "Read a file",
          "parameters": {
            "type": "object",
            "properties": {
              "path": {"type": "string"}
            },
            "required": ["path"]
          }
        }
      }
    ],
    "max_tokens": 100,
    "stream": false
  }'
```

## Expected Results

All tests should pass with:

- ✅ API connectivity: HTTP 200
- ✅ Text completion: Valid JSON response with choices
- ✅ Streaming: SSE chunks received
- ✅ Tool usage: Tool calls in response
- ✅ Performance: Response time < 60 seconds

## Troubleshooting

### API Key Issues

```bash
# Verify API key is set
echo $OPENCODE_API_KEY

# Test authentication
curl -H "Authorization: Bearer $OPENCODE_API_KEY" \
  "https://opencode.ai/zen/v1/models"
```

### Connection Issues

```bash
# Test basic connectivity
curl -I "https://opencode.ai/zen/v1"

# Check DNS resolution
nslookup opencode.ai
```

### Response Parsing Issues

```bash
# Install Python dependencies
pip install python3

# Test manual parsing
curl -s -X POST "https://opencode.ai/zen/v1/chat/completions" \
  -H "Authorization: Bearer $OPENCODE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model": "mimo-v2.5-free", "messages": [{"role": "user", "content": "Hi"}], "max_tokens": 10}' \
  | python3 -m json.tool
```

## Model Information

- **Model ID**: mimo-v2.5-free
- **Provider**: OpenCode
- **API**: OpenAI-compatible
- **Context Window**: 262,144 tokens
- **Max Output**: 262,144 tokens
- **Reasoning**: Enabled
- **Input Types**: text, image

## Support

For issues or questions:
1. Check the full test report: `OPENCODE_E2E_TEST_REPORT.md`
2. Review the test scripts for implementation details
3. Check OpenCode API documentation: https://opencode.ai

## License

This testing suite is part of the CortexCode project.
