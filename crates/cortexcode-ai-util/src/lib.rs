//! Shared utilities for cortex AI.
//!
//! Provides JSON repair, hashing, header conversion, and context-overflow
//! detection functions ported from TypeScript `@kolisachint/hoocode-ai` →
//! `utils/`.

mod copilot_headers;
mod param_fallback;
mod partial_json;
mod retry_delay;
mod tool_constraints;
mod transform_messages;

pub use copilot_headers::{
    build_copilot_dynamic_headers, has_copilot_vision_input, infer_copilot_initiator,
};
pub use param_fallback::{
    droppable_params_named_by, note_rejected_params, rejected_params_for, reset_rejected_params,
    DROPPABLE_PARAMS,
};
pub use partial_json::{parse_partial_json, PartialJsonError};
pub use retry_delay::{
    describe_provider_error, exceeds_retry_delay_cap, format_delay, header_lookup,
    is_long_retry_delay_error, parse_retry_after_ms, post_json_with_sdk_retries, response_headers,
    sdk_retry_timeout_ms, sdk_should_retry, send_with_sdk_retries, SendFailure,
    DEFAULT_MAX_RETRY_DELAY_MS, DEFAULT_SDK_MAX_RETRIES, MAX_TIMER_DELAY_MS,
};
pub use tool_constraints::to_strict_json_schema;
pub use transform_messages::{transform_messages, NormalizeToolCallId};

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
    let empty = || serde_json::from_str("{}").unwrap_or_default();
    let Some(input) = partial.filter(|p| !p.trim().is_empty()) else {
        return empty();
    };
    if let Ok(val) = parse_json_with_repair::<T>(input) {
        return val;
    }
    // `partial-json`, on the raw text and then on the repaired text; a null
    // result counts as nothing (`result ?? {}`).
    let partial = |text: &str| {
        partial_json::parse_partial_json(text)
            .ok()
            .map(|v| {
                if v.is_null() {
                    serde_json::json!({})
                } else {
                    v
                }
            })
            .and_then(|v| serde_json::from_value::<T>(v).ok())
    };
    partial(input)
        .or_else(|| partial(&repair_json(input)))
        .unwrap_or_else(empty)
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
    if message.stop_reason == StopReason::Error {
        if let Some(ref err) = message.error_message {
            if !is_non_overflow(err) && overflow_patterns().iter().any(|p| p.is_match(err)) {
                return true;
            }
        }
    }

    // `contextWindow &&` in TS: absent or 0 disables the usage-based checks.
    let Some(cw) = context_window.filter(|cw| *cw > 0) else {
        return false;
    };
    let input_tokens = message.usage.input + message.usage.cache_read;

    // Case 2: Silent overflow (z.ai style) — successful but usage exceeds context.
    if message.stop_reason == StopReason::Stop && input_tokens > cw {
        return true;
    }

    // Case 3: Length-stop overflow (Xiaomi MiMo style) — server truncates oversized
    // input to fit the context window, leaving no room for output.
    if message.stop_reason == StopReason::Length
        && message.usage.output == 0
        && input_tokens as f64 >= cw as f64 * 0.99
    {
        return true;
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
            r"(?i)input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)",
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

// ---------------------------------------------------------------------------
// Cache retention (providers/cache-retention.ts)
// ---------------------------------------------------------------------------

/// `resolveCacheRetention`: the explicit preference, else the
/// `CORTEXCODE_CACHE_RETENTION` / `HOOCODE_CACHE_RETENTION` environment
/// variable (`short`, `long` or `none`), else `long`.
pub fn resolve_cache_retention(
    cache_retention: Option<cortexcode_ai_types::CacheRetention>,
) -> cortexcode_ai_types::CacheRetention {
    resolve_cache_retention_with(cache_retention, |var| std::env::var(var).ok())
}

fn resolve_cache_retention_with(
    cache_retention: Option<cortexcode_ai_types::CacheRetention>,
    env: impl Fn(&str) -> Option<String>,
) -> cortexcode_ai_types::CacheRetention {
    use cortexcode_ai_types::CacheRetention;
    if let Some(retention) = cache_retention {
        return retention;
    }
    for var in ["CORTEXCODE_CACHE_RETENTION", "HOOCODE_CACHE_RETENTION"] {
        match env(var).as_deref() {
            Some("short") => return CacheRetention::Short,
            Some("long") => return CacheRetention::Long,
            Some("none") => return CacheRetention::None,
            _ => {}
        }
    }
    CacheRetention::Long
}

// ---------------------------------------------------------------------------
// OpenAI SDK errors
// ---------------------------------------------------------------------------

/// The message of the `openai` SDK's `APIError` for a non-2xx response
/// (`APIError.makeMessage` with the body parsed by `safeJSON`), which hoocode
/// reports as the assistant `errorMessage`. The retry-after suffix of
/// `describeProviderError` arrives with the retry utilities (8.5).
pub fn openai_api_error_message(status: u16, body: &str) -> String {
    fn truthy(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Null => false,
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
            serde_json::Value::String(s) => !s.is_empty(),
            _ => true,
        }
    }
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let error = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .filter(|e| truthy(e));
    let msg = match error {
        Some(e) => match e.get("message").filter(|m| truthy(m)) {
            Some(serde_json::Value::String(m)) => m.clone(),
            Some(m) => m.to_string(),
            None => e.to_string(),
        },
        None if parsed.is_none() => body.to_string(),
        None => String::new(),
    };
    if msg.is_empty() {
        format!("{status} status code (no body)")
    } else {
        format!("{status} {msg}")
    }
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
            provider: String::new(),
            response_id: None,
            response_model: None,
            api: String::new(),
            diagnostics: None,
            model: String::new(),
            content: vec![],
            stop_reason: StopReason::Error,

            usage: Default::default(),
            timestamp: 0,
            error_message: Some(msg.to_string()),
        }
    }

    /// `parseStreamingJson` on incomplete input, against what the
    /// `partial-json` package at the pin returns (recorded with node).
    #[test]
    fn parse_streaming_json_matches_partial_json() {
        let cases = [
            (r#"{"path":"README"#, r#"{"path":"README"}"#),
            (r#"{"a":1,"b":[1,2"#, r#"{"a":1,"b":[1,2]}"#),
            (r#"{"a":{"b":"c"#, r#"{"a":{"b":"c"}}"#),
            (r#"{"a":tr"#, r#"{"a":true}"#),
            (r#"{"a":1."#, r#"{}"#),
            (r#"{"a":"x\"#, r#"{"a":"x"}"#),
            (r#"[1,2,{"k":"#, r#"[1,2,{}]"#),
            (r#"{"a":nu"#, r#"{"a":null}"#),
            (r#"{"a"#, r#"{}"#),
            (r#"{"a":"#, r#"{}"#),
            (r#""str"#, r#""str""#),
            (r#"{"a":-"#, r#"{}"#),
            (r#"{"a":"\u00"#, r#"{"a":""}"#),
            ("", "{}"),
        ];
        let mut failures = Vec::new();
        for (input, expected) in cases {
            let got: serde_json::Value = parse_streaming_json(Some(input));
            let expected: serde_json::Value = serde_json::from_str(expected).unwrap();
            if got != expected {
                failures.push(format!("{input:?}: got {got}, want {expected}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    // --- overflow.test.ts ---

    fn ollama_error(msg: &str) -> AssistantMessage {
        let mut m = make_error_msg(msg);
        m.api = "openai-completions".into();
        m.provider = "ollama".into();
        m.model = "qwen3.5:35b".into();
        m
    }

    #[test]
    fn detects_explicit_ollama_prompt_too_long_errors() {
        let m = ollama_error("400 `prompt too long; exceeded max context length by 100918 tokens`");
        assert!(is_context_overflow(&m, Some(32768)));
    }

    #[test]
    fn detects_together_ai_context_length_errors() {
        let m = ollama_error(
            "400 The input (516368 tokens) is longer than the model's context length (262144 tokens).",
        );
        assert!(is_context_overflow(&m, Some(262144)));
        let m = ollama_error(
            "400 The input (516368 tokens) is longer than the models context length (262144 tokens).",
        );
        assert!(is_context_overflow(&m, Some(262144)));
    }

    #[test]
    fn non_overflow_errors_are_not_overflow() {
        for msg in [
            "500 `model runner crashed unexpectedly`",
            "Throttling error: Too many tokens, please wait before trying again.",
            "Service unavailable: The service is temporarily unavailable.",
            "Rate limit exceeded, please retry after 30 seconds.",
            "Too many requests. Please slow down.",
        ] {
            assert!(
                !is_context_overflow(&ollama_error(msg), Some(200_000)),
                "{msg}"
            );
        }
    }

    fn length_stop(input: u64, cache_read: u64, output: u64) -> AssistantMessage {
        let mut m = make_error_msg("");
        m.error_message = None;
        m.stop_reason = StopReason::Length;
        m.usage.input = input;
        m.usage.cache_read = cache_read;
        m.usage.output = output;
        m
    }

    #[test]
    fn detects_xiaomi_style_length_stop_overflow() {
        assert!(is_context_overflow(
            &length_stop(58, 1_048_512, 0),
            Some(1_048_576)
        ));
        assert!(!is_context_overflow(
            &length_stop(1000, 0, 4096),
            Some(200_000)
        ));
        assert!(!is_context_overflow(&length_stop(100, 0, 0), Some(200_000)));
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
            provider: String::new(),
            response_id: None,
            response_model: None,
            api: String::new(),
            diagnostics: None,
            model: String::new(),
            content: vec![],
            stop_reason: StopReason::Stop,

            usage: Default::default(),
            timestamp: 0,
            error_message: None,
        };
        assert!(!is_context_overflow(&msg, Some(100_000)));
    }

    #[test]
    fn test_silent_overflow_z_ai() {
        let msg = AssistantMessage {
            provider: String::new(),
            response_id: None,
            response_model: None,
            api: String::new(),
            diagnostics: None,
            model: String::new(),
            content: vec![],
            stop_reason: StopReason::Stop,

            usage: cortexcode_ai_types::Usage {
                input: 150_000,
                output: 100,
                cache_read: 0,
                cache_write: 0,
                total_tokens: 150_100,
                cost: cortexcode_ai_types::Cost::default(),
            },
            timestamp: 0,
            error_message: None,
        };
        assert!(is_context_overflow(&msg, Some(100_000)));
        assert!(!is_context_overflow(&msg, Some(200_000)));
    }

    #[test]
    fn cache_retention_prefers_explicit_then_env_then_long() {
        use cortexcode_ai_types::CacheRetention;
        let no_env = |_: &str| None;
        assert_eq!(
            resolve_cache_retention_with(None, no_env),
            CacheRetention::Long
        );
        assert_eq!(
            resolve_cache_retention_with(Some(CacheRetention::None), |_| Some("long".into())),
            CacheRetention::None
        );
        let hoocode_short = |v: &str| (v == "HOOCODE_CACHE_RETENTION").then(|| "short".into());
        assert_eq!(
            resolve_cache_retention_with(None, hoocode_short),
            CacheRetention::Short
        );
        let both = |v: &str| {
            Some(
                if v == "CORTEXCODE_CACHE_RETENTION" {
                    "none"
                } else {
                    "short"
                }
                .to_string(),
            )
        };
        assert_eq!(
            resolve_cache_retention_with(None, both),
            CacheRetention::None
        );
        // Unknown values are ignored.
        assert_eq!(
            resolve_cache_retention_with(None, |_| Some("forever".into())),
            CacheRetention::Long
        );
    }
}
