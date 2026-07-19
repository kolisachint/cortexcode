//! Shared utilities for cortex AI.
//!
//! Provides JSON repair, hashing, header conversion, and context-overflow
//! detection functions ported from TypeScript `@kolisachint/hoocode-ai` →
//! `utils/`.

// ---------------------------------------------------------------------------
// hash
// ---------------------------------------------------------------------------

/// A fast, deterministic hash used to shorten long strings.
///
/// Ported from TypeScript `utils/hash.ts` → `shortHash()`.
pub fn short_hash(input: &str) -> String {
    let mut h1: u32 = 0xdeadbeef;
    let mut h2: u32 = 0x41c6ce57;

    for byte in input.bytes() {
        let ch = byte as u32;
        h1 = h1.wrapping_mul(2654435761) ^ ch;
        h2 = h2.wrapping_mul(1597334677) ^ ch;
    }

    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2246822507) ^ (h2 ^ (h2 >> 13)).wrapping_mul(3266489909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2246822507) ^ (h1 ^ (h1 >> 13)).wrapping_mul(3266489909);

    format!("{:x}{:x}", h2, h1)
}

// ---------------------------------------------------------------------------
// headers
// ---------------------------------------------------------------------------

/// Convert a `Vec<(String, String)>` of HTTP headers to a `HashMap`.
///
/// Ported from TypeScript `utils/headers.ts` → `headersToRecord()`.
pub fn headers_to_record(
    headers: &[(String, String)],
) -> std::collections::HashMap<String, String> {
    headers.iter().cloned().collect()
}

// ---------------------------------------------------------------------------
// json
// ---------------------------------------------------------------------------

use std::collections::HashSet;

/// Set of valid JSON escape characters.
fn valid_json_escapes() -> HashSet<char> {
    ['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u']
        .into_iter()
        .collect()
}

/// Check if a character is a control character (U+0000–U+001F).
fn is_control_char(c: char) -> bool {
    let code = c as u32;
    code <= 0x1F
}

/// Escape a control character as a JSON escape sequence.
fn escape_control_char(c: char) -> String {
    match c {
        '\u{0008}' => "\\b".to_string(), // backspace
        '\u{000C}' => "\\f".to_string(), // form feed
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        _ => format!("\\u{:04x}", c as u32),
    }
}

/// Repair malformed JSON by escaping raw control characters inside strings
/// and doubling backslashes before invalid escape characters.
///
/// Ported from TypeScript `utils/json-parse.ts` → `repairJson()`.
pub fn repair_json(json: &str) -> String {
    let escapes = valid_json_escapes();
    let mut repaired = String::with_capacity(json.len());
    let mut in_string = false;
    let chars: Vec<char> = json.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if !in_string {
            repaired.push(c);
            if c == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }

        if c == '"' {
            repaired.push(c);
            in_string = false;
            i += 1;
            continue;
        }

        if c == '\\' {
            if i + 1 >= chars.len() {
                repaired.push_str("\\\\");
                i += 1;
                continue;
            }

            let next = chars[i + 1];

            if next == 'u' {
                // Check if followed by 4 hex digits
                if i + 5 < chars.len() {
                    let hex: String = chars[i + 2..=i + 5].iter().collect();
                    if hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                        repaired.push('\\');
                        repaired.push('u');
                        repaired.push_str(&hex);
                        i += 6;
                        continue;
                    }
                }
            }

            if escapes.contains(&next) {
                repaired.push('\\');
                repaired.push(next);
                i += 2;
                continue;
            }

            // Invalid escape — double the backslash
            repaired.push_str("\\\\");
            i += 1;
            continue;
        }

        if is_control_char(c) {
            repaired.push_str(&escape_control_char(c));
        } else {
            repaired.push(c);
        }
        i += 1;
    }

    repaired
}

/// Parse JSON, repairing common malformations first if the initial parse fails.
///
/// Ported from TypeScript `utils/json-parse.ts` → `parseJsonWithRepair()`.
pub fn parse_json_with_repair<T: serde::de::DeserializeOwned>(
    json: &str,
) -> Result<T, serde_json::Error> {
    match serde_json::from_str(json) {
        Ok(val) => Ok(val),
        Err(e) => {
            let repaired = repair_json(json);
            if repaired != json {
                serde_json::from_str(&repaired)
            } else {
                Err(e)
            }
        }
    }
}

/// Attempt to parse potentially incomplete JSON (streaming partial output).
///
/// Falls back progressively: full JSON parse → with repair → partial-json
/// tolerant parse → empty object.
///
/// Ported from TypeScript `utils/json-parse.ts` → `parseStreamingJson()`.
pub fn parse_streaming_json<T: serde::de::DeserializeOwned + Default>(partial: Option<&str>) -> T {
    let Some(input) = partial else {
        return serde_json::from_str("{}").unwrap_or_default();
    };

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return serde_json::from_str("{}").unwrap_or_default();
    }

    // Try full parse first.
    if let Ok(val) = serde_json::from_str::<T>(trimmed) {
        return val;
    }

    // Try with repair.
    let repaired = repair_json(trimmed);
    if let Ok(val) = serde_json::from_str::<T>(&repaired) {
        return val;
    }

    // Try tolerant partial parse — find the deepest valid JSON prefix by
    // progressively trimming trailing characters.
    try_tolerant_parse::<T>(trimmed)
        .or_else(|| try_tolerant_parse::<T>(&repaired))
        .unwrap_or_else(|| serde_json::from_str("{}").unwrap_or_default())
}

/// Try to parse JSON by trimming trailing characters until valid.
fn try_tolerant_parse<T: serde::de::DeserializeOwned + Default>(input: &str) -> Option<T> {
    // For arrays, try adding closing brackets.
    if let Some(stripped) = input.strip_suffix(',') {
        if let Ok(val) = serde_json::from_str::<T>(stripped) {
            return Some(val);
        }
    }

    // Try adding closing quotes, brackets, braces progressively.
    let mut candidate = input.to_string();
    for _ in 0..10 {
        if let Ok(val) = serde_json::from_str::<T>(&candidate) {
            return Some(val);
        }
        // Add closing brace if it looks like an object
        if candidate.starts_with('{') && !candidate.ends_with('}') {
            candidate.push('}');
            continue;
        }
        // Add closing bracket if it looks like an array
        if candidate.starts_with('[') && !candidate.ends_with(']') {
            candidate.push(']');
            continue;
        }
        // Add closing quote if in string
        if candidate.ends_with('"') || candidate.matches('"').count() % 2 == 1 {
            candidate.push('"');
            continue;
        }
        break;
    }

    None
}

// ---------------------------------------------------------------------------
// overflow
// ---------------------------------------------------------------------------

use cortexcode_ai_types::{AssistantMessage, StopReason};

/// Check if an assistant message represents a context overflow error.
///
/// Ported from TypeScript `utils/overflow.ts` → `isContextOverflow()`.
///
/// Two detection modes:
/// 1. **Error-based**: Most providers return `stopReason == Error` with a
///    matching error message.
/// 2. **Silent overflow**: Some providers (z.ai, Xiaomi) accept overflow and
///    return successfully. Use `context_window` to detect via token counts.
pub fn is_context_overflow(message: &AssistantMessage, context_window: Option<u64>) -> bool {
    // Case 1: Check error message patterns.
    if message.stop_reason == Some(StopReason::Error) {
        if let Some(ref err) = message.error_message {
            if !is_non_overflow(err) && overflow_patterns().iter().any(|p| p.is_match(err)) {
                return true;
            }
        }
    }

    let Some(cw) = context_window else {
        return false;
    };

    // Case 2: Silent overflow (z.ai style) — successful but usage exceeds context.
    if message.stop_reason == Some(StopReason::EndTurn) {
        if let Some(ref usage) = message.usage {
            let input_tokens = usage.input + usage.cache_read;
            if input_tokens > cw {
                return true;
            }
        }
    }

    // Case 3: Length-stop overflow (Xiaomi MiMo style) — server truncates input
    // to fit context window, leaving no room for output.
    if message.stop_reason == Some(StopReason::StopSequence)
        || message.stop_reason == Some(StopReason::MaxTokens)
    {
        if let Some(ref usage) = message.usage {
            if usage.output == 0 {
                let input_tokens = usage.input + usage.cache_read;
                if input_tokens as f64 >= cw as f64 * 0.99 {
                    return true;
                }
            }
        }
    }

    false
}

/// Compiled regex patterns that indicate context overflow errors.
fn overflow_patterns() -> &'static [regex::Regex] {
    use std::sync::OnceLock;
    static PATTERNS: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)prompt is too long",
            r"(?i)request_too_large",
            r"(?i)input is too long for requested model",
            r"(?i)exceeds the context window",
            r"(?i)input token count.*exceeds the maximum",
            r"(?i)maximum prompt length is \d+",
            r"(?i)reduce the length of the messages",
            r"(?i)maximum context length is \d+ tokens",
            r"(?i)input \(\d+ tokens\) is longer than the model's? context length \(\d+ tokens\)",
            r"(?i)exceeds the limit of \d+",
            r"(?i)exceeds the available context size",
            r"(?i)greater than the context length",
            r"(?i)context window exceeds limit",
            r"(?i)exceeded model token limit",
            r"(?i)too large for model with \d+ maximum context length",
            r"(?i)model_context_window_exceeded",
            r"(?i)prompt too long; exceeded (?:max )?context length",
            r"(?i)context[_ ]length[_ ]exceeded",
            r"(?i)too many tokens",
            r"(?i)token limit exceeded",
            r"(?i)^4(?:00|13)\s*(?:status code)?\s*\(no body\)",
        ]
        .iter()
        .map(|p| regex::Regex::new(p).expect("valid regex"))
        .collect()
    })
}

/// Non-overflow patterns that should be excluded even if they also match an
/// OVERFLOW_PATTERN (e.g. AWS Bedrock throttling).
fn is_non_overflow(msg: &str) -> bool {
    let patterns = [
        r"^(?i)(Throttling error|Service unavailable):",
        r"(?i)rate limit",
        r"(?i)too many requests",
    ];
    patterns.iter().any(|p| {
        regex::Regex::new(p)
            .map(|re| re.is_match(msg))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- hash ---

    #[test]
    fn test_short_hash_consistent() {
        let h1 = short_hash("hello world");
        let h2 = short_hash("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_short_hash_different() {
        let h1 = short_hash("hello");
        let h2 = short_hash("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_short_hash_empty() {
        let h = short_hash("");
        assert!(!h.is_empty());
    }

    // --- headers ---

    #[test]
    fn test_headers_to_record() {
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Authorization".to_string(), "Bearer token".to_string()),
        ];
        let map = headers_to_record(&headers);
        assert_eq!(map.get("Content-Type").unwrap(), "application/json");
        assert_eq!(map.get("Authorization").unwrap(), "Bearer token");
        assert_eq!(map.len(), 2);
    }

    // --- json repair ---

    #[test]
    fn test_repair_json_valid() {
        let input = r#"{"a": 1, "b": "hello"}"#;
        assert_eq!(repair_json(input), input);
    }

    #[test]
    fn test_repair_json_control_chars() {
        // Raw newline inside string
        let input = "{\"a\": \"hello\nworld\"}";
        let repaired = repair_json(input);
        assert_eq!(repaired, "{\"a\": \"hello\\nworld\"}");
    }

    #[test]
    fn test_repair_json_invalid_escape() {
        // \x is not a valid JSON escape
        let input = r#"{"a": "hello\xworld"}"#;
        let repaired = repair_json(input);
        assert_eq!(repaired, r#"{"a": "hello\\xworld"}"#);
    }

    #[test]
    fn test_repair_json_trailing_backslash() {
        let input = r#"{"a": "hello\"#;
        let repaired = repair_json(input);
        assert_eq!(repaired, r#"{"a": "hello\\"#);
    }

    #[test]
    fn test_parse_json_with_repair_valid() {
        let input = r#"{"a": 1, "b": "hello"}"#;
        let val: serde_json::Value = parse_json_with_repair(input).unwrap();
        assert_eq!(val["a"], 1);
    }

    #[test]
    fn test_parse_streaming_json_empty() {
        let val: serde_json::Value = parse_streaming_json(None);
        assert!(val.as_object().is_some_and(|o| o.is_empty()));
    }

    #[test]
    fn test_parse_streaming_json_valid() {
        let input = r#"{"a": 1, "b": "hello"}"#;
        let val: serde_json::Value = parse_streaming_json(Some(input));
        assert_eq!(val["a"], 1);
        assert_eq!(val["b"], "hello");
    }

    // --- overflow ---

    fn make_error_msg(msg: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            stop_reason: Some(StopReason::Error),
            stop_sequence: None,
            usage: None,
            timestamp: None,
            error_message: Some(msg.to_string()),
        }
    }

    #[test]
    fn test_overflow_anthropic() {
        let msg = make_error_msg("prompt is too long: 213462 tokens > 200000 maximum");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_overflow_openai() {
        let msg = make_error_msg("Your input exceeds the context window of this model");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_not_overflow_rate_limit() {
        let msg = make_error_msg("Rate limit exceeded");
        assert!(!is_context_overflow(&msg, None));
    }

    #[test]
    fn test_not_overflow_throttling() {
        let msg = make_error_msg("Throttling error: Too many tokens, please wait");
        assert!(!is_context_overflow(&msg, None));
    }

    #[test]
    fn test_not_overflow_no_error() {
        let msg = AssistantMessage {
            content: vec![],
            stop_reason: Some(StopReason::EndTurn),
            stop_sequence: None,
            usage: None,
            timestamp: None,
            error_message: None,
        };
        assert!(!is_context_overflow(&msg, Some(100_000)));
    }

    #[test]
    fn test_silent_overflow_z_ai() {
        let msg = AssistantMessage {
            content: vec![],
            stop_reason: Some(StopReason::EndTurn),
            stop_sequence: None,
            usage: Some(cortexcode_ai_types::Usage {
                input: 150_000,
                output: 100,
                cache_read: 0,
                cache_write: 0,
                total_tokens: 150_100,
                cost: cortexcode_ai_types::Cost::default(),
            }),
            timestamp: None,
            error_message: None,
        };
        assert!(is_context_overflow(&msg, Some(100_000)));
        assert!(!is_context_overflow(&msg, Some(200_000)));
    }
}
