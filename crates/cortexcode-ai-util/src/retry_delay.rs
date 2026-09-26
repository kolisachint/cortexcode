//! Server-requested retry delays, the cap that keeps them sane, and the retry
//! policy of the `openai` / `@anthropic-ai/sdk` clients.
//!
//! Port of hoocode `utils/retry-delay.ts` (v0.5.89). The TS providers get
//! client retries from the vendor SDKs (2 by default: 408, 409, 429 and 5xx,
//! `x-should-retry` obeyed, `retry-after-ms` / `retry-after` honoured, else
//! 0.5s..8s exponential backoff with jitter) and cap long server-requested
//! waits with `createRetryDelayCapFetch`, which stamps `x-should-retry: false`.
//! [`send_with_sdk_retries`] is that loop with the cap built in.

use std::future::Future;

/// `MAX_TIMER_DELAY_MS`: the largest delay a JS timer can hold.
pub const MAX_TIMER_DELAY_MS: f64 = 2_147_483_647.0;

/// `DEFAULT_MAX_RETRY_DELAY_MS`.
pub const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// The SDKs' default `maxRetries`.
pub const DEFAULT_SDK_MAX_RETRIES: u32 = 2;

/// Phrase every capped-delay message carries.
const LONG_DELAY_MARKER: &str = "asked to wait";

/// JS `parseFloat`: the longest numeric prefix, `None` for NaN.
fn parse_float(s: &str) -> Option<f64> {
    let s = s.trim_start();
    let mut end = 0;
    let bytes = s.as_bytes();
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    while end < bytes.len() {
        let c = bytes[end] as char;
        match c {
            '0'..='9' => seen_digit = true,
            '+' | '-' if end == 0 || matches!(bytes[end - 1] as char, 'e' | 'E') => {}
            '.' if !seen_dot && !seen_exp => seen_dot = true,
            'e' | 'E' if seen_digit && !seen_exp => seen_exp = true,
            _ => break,
        }
        end += 1;
    }
    let mut candidate = &s[..end];
    while !candidate.is_empty() {
        if let Ok(v) = candidate.parse::<f64>() {
            return Some(v);
        }
        candidate = &candidate[..candidate.len() - 1];
    }
    if s.starts_with("Infinity") || s.starts_with("+Infinity") {
        return Some(f64::INFINITY);
    }
    if s.starts_with("-Infinity") {
        return Some(f64::NEG_INFINITY);
    }
    None
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}

/// `Date.parse` for the HTTP-date form of `retry-after`.
fn parse_http_date_ms(s: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc2822(s.trim())
        .ok()
        .map(|d| d.timestamp_millis() as f64)
}

/// `parseRetryAfterMs`: `retry-after-ms`, else `retry-after` as seconds,
/// else as an HTTP date (relative to now). `get` looks a header up
/// case-insensitively.
pub fn parse_retry_after_ms(get: impl Fn(&str) -> Option<String>) -> Option<f64> {
    if let Some(after_ms) = get("retry-after-ms").filter(|v| !v.is_empty()) {
        if let Some(ms) = parse_float(&after_ms) {
            return Some(ms);
        }
    }
    let after = get("retry-after").filter(|v| !v.is_empty())?;
    if let Some(seconds) = parse_float(&after) {
        return Some(seconds * 1000.0);
    }
    parse_http_date_ms(&after).map(|at| at - now_ms())
}

/// Header lookup over an ordered `(name, value)` list, case-insensitive.
pub fn header_lookup(headers: &[(String, String)]) -> impl Fn(&str) -> Option<String> + '_ {
    move |name| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

/// `formatDelay`: the two largest meaningful units ("28d 14h", "2m 30s").
pub fn format_delay(ms: f64) -> String {
    if !ms.is_finite() || ms < 0.0 {
        return "an unknown time".to_string();
    }
    // Math.round: halves round up.
    let total_seconds = (ms / 1000.0 + 0.5).floor() as u64;
    if total_seconds < 60 {
        return format!("{total_seconds}s");
    }
    let mut parts = Vec::new();
    let mut remaining = total_seconds;
    for (size, suffix) in [(86_400, "d"), (3_600, "h"), (60, "m"), (1, "s")] {
        let value = remaining / size;
        remaining -= value * size;
        if value > 0 {
            parts.push(format!("{value}{suffix}"));
        }
        if parts.len() == 2 {
            break;
        }
    }
    parts.join(" ")
}

/// `describeProviderError`: the error message, plus how long the provider
/// asked us to wait when that is past `max_retry_delay_ms` (default 60s).
/// `headers` are the failed response's headers, when the error had one.
pub fn describe_provider_error(
    message: &str,
    headers: Option<&[(String, String)]>,
    max_retry_delay_ms: Option<u64>,
) -> String {
    let max = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS) as f64;
    let Some(delay) = headers.and_then(|h| parse_retry_after_ms(header_lookup(h))) else {
        return message.to_string();
    };
    if delay <= max.max(0.0) {
        return message.to_string();
    }
    format!(
        "{message} (the provider {LONG_DELAY_MARKER} {} before retrying, so no retry was attempted)",
        format_delay(delay)
    )
}

/// `isLongRetryDelayError`.
pub fn is_long_retry_delay_error(message: Option<&str>) -> bool {
    message.is_some_and(|m| m.contains(&format!("provider {LONG_DELAY_MARKER}")))
}

/// `createRetryDelayCapFetch`: whether a failed response asks to wait
/// longer than the cap, so the SDK must not retry it. A cap of zero (or
/// none) disables the check; a response with its own `x-should-retry`
/// outranks it.
pub fn exceeds_retry_delay_cap(
    status: u16,
    get: &dyn Fn(&str) -> Option<String>,
    max_retry_delay_ms: Option<u64>,
) -> bool {
    let Some(cap) = max_retry_delay_ms.filter(|&c| c > 0) else {
        return false;
    };
    if (200..300).contains(&status) || get("x-should-retry").is_some() {
        return false;
    }
    parse_retry_after_ms(get).is_some_and(|delay| delay > cap as f64)
}

/// The SDKs' `shouldRetry` for a failed response.
pub fn sdk_should_retry(status: u16, get: &dyn Fn(&str) -> Option<String>) -> bool {
    match get("x-should-retry").as_deref() {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }
    matches!(status, 408 | 409 | 429) || status >= 500
}

/// The SDKs' `retryRequest` delay: the server's `retry-after-ms` /
/// `retry-after` when given, else `calculateDefaultRetryTimeoutMillis`
/// (0.5s doubling per retry, at most 8s, minus up to 25% jitter).
pub fn sdk_retry_timeout_ms(
    get: &dyn Fn(&str) -> Option<String>,
    retries_remaining: u32,
    max_retries: u32,
    jitter: f64,
) -> f64 {
    let mut timeout = get("retry-after-ms")
        .filter(|v| !v.is_empty())
        .and_then(|v| parse_float(&v));
    if timeout.is_none_or(|t| t == 0.0) {
        if let Some(after) = get("retry-after").filter(|v| !v.is_empty()) {
            timeout = match parse_float(&after) {
                Some(seconds) => Some(seconds * 1000.0),
                None => Some(parse_http_date_ms(&after).map_or(f64::NAN, |at| at - now_ms())),
            };
        }
    }
    timeout.unwrap_or_else(|| {
        let num_retries = max_retries.saturating_sub(retries_remaining);
        let sleep_seconds = (0.5 * 2f64.powi(num_retries as i32)).min(8.0);
        sleep_seconds * (1.0 - jitter * 0.25) * 1000.0
    })
}

/// A pseudo-random jitter in `[0, 1)` (`Math.random()`).
fn jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

/// Why a request produced no response.
#[derive(Debug, Clone, PartialEq)]
pub enum SendFailure {
    /// The request timed out (`APIConnectionTimeoutError`).
    Timeout,
    /// Any other transport failure (`APIConnectionError`).
    Connection,
}

impl SendFailure {
    /// The SDK error message.
    pub fn message(&self) -> &'static str {
        match self {
            SendFailure::Timeout => "Request timed out.",
            SendFailure::Connection => "Connection error.",
        }
    }
}

/// The SDK request loop: send, and while retries remain, retry transport
/// failures and retryable error responses after the SDK's delay. A failed
/// response asking to wait past `max_retry_delay_ms` is not retried. The
/// final response (success or not) is returned; a final transport failure
/// is `Err`. `status` / `header` read a response.
pub async fn send_with_sdk_retries<R, Fut>(
    max_retries: Option<u32>,
    max_retry_delay_ms: Option<u64>,
    mut send: impl FnMut() -> Fut,
    status: impl Fn(&R) -> u16,
    header: impl Fn(&R, &str) -> Option<String>,
) -> Result<R, SendFailure>
where
    Fut: Future<Output = Result<R, SendFailure>>,
{
    let max_retries = max_retries.unwrap_or(DEFAULT_SDK_MAX_RETRIES);
    let mut retries_remaining = max_retries;
    loop {
        match send().await {
            Err(failure) => {
                if retries_remaining == 0 {
                    return Err(failure);
                }
                let no_headers = |_: &str| None;
                let delay =
                    sdk_retry_timeout_ms(&no_headers, retries_remaining, max_retries, jitter());
                sleep_ms(delay).await;
            }
            Ok(response) => {
                let code = status(&response);
                if (200..300).contains(&code) || retries_remaining == 0 {
                    return Ok(response);
                }
                let get = |name: &str| header(&response, name);
                let capped = exceeds_retry_delay_cap(code, &get, max_retry_delay_ms);
                if capped || !sdk_should_retry(code, &get) {
                    return Ok(response);
                }
                let delay = sdk_retry_timeout_ms(&get, retries_remaining, max_retries, jitter());
                drop(response);
                sleep_ms(delay).await;
            }
        }
        retries_remaining -= 1;
    }
}

/// POST `body` as JSON with `headers`, through [`send_with_sdk_retries`].
pub async fn post_json_with_sdk_retries(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    body: &serde_json::Value,
    max_retries: Option<u32>,
    max_retry_delay_ms: Option<u64>,
) -> Result<reqwest::Response, SendFailure> {
    send_with_sdk_retries(
        max_retries,
        max_retry_delay_ms,
        || {
            let mut request = client.post(url);
            for (k, v) in headers {
                request = request.header(k, v);
            }
            let request = request.json(body);
            async move {
                request.send().await.map_err(|e| {
                    if e.is_timeout() {
                        SendFailure::Timeout
                    } else {
                        SendFailure::Connection
                    }
                })
            }
        },
        |r: &reqwest::Response| r.status().as_u16(),
        |r: &reqwest::Response, name| {
            r.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        },
    )
    .await
}

/// A response's headers as an ordered `(name, value)` list.
pub fn response_headers(response: &reqwest::Response) -> Vec<(String, String)> {
    response
        .headers()
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
        .collect()
}

/// `{ status, headers: headersToRecord(response.headers) }` for the
/// `onResponse` hook.
pub fn provider_response(response: &reqwest::Response) -> cortexcode_ai_types::ProviderResponse {
    cortexcode_ai_types::ProviderResponse::from_pairs(
        response.status().as_u16(),
        response.headers().iter().map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        }),
    )
}

async fn sleep_ms(ms: f64) {
    // setTimeout semantics: NaN or negative sleeps 0, and a delay past what a
    // timer holds becomes 1ms (the overflow the delay cap exists to prevent).
    let ms = if ms.is_nan() || ms <= 0.0 {
        0.0
    } else if ms > MAX_TIMER_DELAY_MS {
        1.0
    } else {
        ms
    };
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

#[cfg(test)]
mod tests {
    //! Ported from `retry-delay.test.ts` (hoocode v0.5.89); the fetch-wrapper
    //! cases become [`exceeds_retry_delay_cap`] and [`send_with_sdk_retries`].

    use super::*;
    use std::cell::RefCell;

    const QUOTA_RETRY_AFTER_SECONDS: &str = "2472352";
    const QUOTA_DELAY_MS: f64 = 2_472_352_000.0;

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn reads_retry_after_as_seconds() {
        let h = headers(&[("retry-after", QUOTA_RETRY_AFTER_SECONDS)]);
        assert_eq!(
            parse_retry_after_ms(header_lookup(&h)),
            Some(QUOTA_DELAY_MS)
        );
    }

    #[test]
    fn prefers_retry_after_ms() {
        let h = headers(&[("retry-after-ms", "1500"), ("retry-after", "30")]);
        assert_eq!(parse_retry_after_ms(header_lookup(&h)), Some(1500.0));
    }

    #[test]
    fn reads_retry_after_as_an_http_date() {
        let at = chrono::Utc::now() + chrono::Duration::milliseconds(120_000);
        let h = headers(&[("Retry-After", &at.to_rfc2822().replace("+0000", "GMT"))]);
        let parsed = parse_retry_after_ms(header_lookup(&h)).unwrap();
        assert!((parsed - 120_000.0).abs() < 1500.0, "{parsed}");
    }

    #[test]
    fn no_delay_asked_for() {
        assert_eq!(parse_retry_after_ms(header_lookup(&[])), None);
    }

    #[test]
    fn formats_delays_in_units_a_person_acts_on() {
        assert_eq!(format_delay(QUOTA_DELAY_MS), "28d 14h");
        assert_eq!(format_delay(45_000.0), "45s");
        assert_eq!(format_delay(150_000.0), "2m 30s");
        assert_eq!(format_delay(3_600_000.0), "1h");
        assert_eq!(format_delay(-1.0), "an unknown time");
    }

    #[test]
    fn marks_a_too_long_wait_as_not_retryable() {
        let h = headers(&[("retry-after", QUOTA_RETRY_AFTER_SECONDS)]);
        assert!(exceeds_retry_delay_cap(
            429,
            &header_lookup(&h),
            Some(60_000)
        ));
    }

    #[test]
    fn leaves_short_waits_and_missing_retry_after_alone() {
        let h = headers(&[("retry-after", "30")]);
        assert!(!exceeds_retry_delay_cap(
            429,
            &header_lookup(&h),
            Some(60_000)
        ));
        assert!(!exceeds_retry_delay_cap(
            429,
            &header_lookup(&[]),
            Some(60_000)
        ));
    }

    #[test]
    fn a_servers_own_preference_and_successes_are_left_alone() {
        let h = headers(&[
            ("retry-after", QUOTA_RETRY_AFTER_SECONDS),
            ("x-should-retry", "true"),
        ]);
        assert!(!exceeds_retry_delay_cap(
            429,
            &header_lookup(&h),
            Some(60_000)
        ));
        let h = headers(&[("retry-after", QUOTA_RETRY_AFTER_SECONDS)]);
        assert!(!exceeds_retry_delay_cap(
            200,
            &header_lookup(&h),
            Some(60_000)
        ));
    }

    #[test]
    fn a_cap_of_zero_or_none_disables_the_check() {
        let h = headers(&[("retry-after", QUOTA_RETRY_AFTER_SECONDS)]);
        assert!(!exceeds_retry_delay_cap(429, &header_lookup(&h), Some(0)));
        assert!(!exceeds_retry_delay_cap(429, &header_lookup(&h), None));
    }

    #[test]
    fn describes_how_long_the_provider_wants_us_gone() {
        let h = headers(&[("retry-after", QUOTA_RETRY_AFTER_SECONDS)]);
        let described = describe_provider_error("429 quota exceeded", Some(&h), None);
        assert!(described.contains("429 quota exceeded"));
        assert!(described.contains("28d 14h"));
        assert!(is_long_retry_delay_error(Some(&described)));
    }

    #[test]
    fn leaves_short_waits_unannotated() {
        let h = headers(&[("retry-after", "30")]);
        let described = describe_provider_error("429 rate limited", Some(&h), None);
        assert_eq!(described, "429 rate limited");
        assert!(!is_long_retry_delay_error(Some(&described)));
        assert_eq!(
            describe_provider_error("fetch failed", None, None),
            "fetch failed"
        );
    }

    #[test]
    fn the_quota_delay_cannot_be_held_by_a_timer() {
        const { assert!(QUOTA_DELAY_MS > MAX_TIMER_DELAY_MS) };
        assert_eq!(QUOTA_DELAY_MS.min(MAX_TIMER_DELAY_MS), MAX_TIMER_DELAY_MS);
    }

    // --- SDK retry policy ---

    #[test]
    fn sdk_retry_policy_matches_the_clients() {
        let none = |_: &str| None;
        for status in [408, 409, 429, 500, 503] {
            assert!(sdk_should_retry(status, &none), "{status}");
        }
        for status in [400, 401, 404, 422] {
            assert!(!sdk_should_retry(status, &none), "{status}");
        }
        let never = |n: &str| (n == "x-should-retry").then(|| "false".to_string());
        assert!(!sdk_should_retry(500, &never));
        let always = |n: &str| (n == "x-should-retry").then(|| "true".to_string());
        assert!(sdk_should_retry(400, &always));
        // Default backoff: 0.5s, 1s, .. capped at 8s, minus up to 25% jitter.
        assert_eq!(sdk_retry_timeout_ms(&none, 2, 2, 0.0), 500.0);
        assert_eq!(sdk_retry_timeout_ms(&none, 1, 2, 0.0), 1000.0);
        assert_eq!(sdk_retry_timeout_ms(&none, 0, 10, 0.0), 8000.0);
        assert_eq!(sdk_retry_timeout_ms(&none, 2, 2, 1.0), 375.0);
        let after = |n: &str| (n == "retry-after").then(|| "3".to_string());
        assert_eq!(sdk_retry_timeout_ms(&after, 2, 2, 0.0), 3000.0);
    }

    fn run<R>(f: impl Future<Output = R>) -> R {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(f)
    }

    type Fake = (u16, Vec<(String, String)>);

    fn drive(
        script: Vec<Result<Fake, SendFailure>>,
        max_retries: Option<u32>,
    ) -> (Result<Fake, SendFailure>, usize) {
        let calls = RefCell::new(0usize);
        let script = RefCell::new(script.into_iter());
        let out = run(send_with_sdk_retries(
            max_retries,
            Some(60_000),
            || {
                *calls.borrow_mut() += 1;
                let next = script.borrow_mut().next().expect("scripted response");
                async move { next }
            },
            |r: &Fake| r.0,
            |r: &Fake, name| header_lookup(&r.1)(name),
        ));
        let n = *calls.borrow();
        (out, n)
    }

    fn fast(status: u16) -> Result<Fake, SendFailure> {
        Ok((status, headers(&[("retry-after-ms", "1")])))
    }

    #[test]
    fn retries_retryable_statuses_twice_by_default() {
        let (out, calls) = drive(vec![fast(500), fast(429), fast(503)], None);
        assert_eq!(out.unwrap().0, 503);
        assert_eq!(calls, 3);
        let (out, calls) = drive(vec![fast(500), fast(200)], None);
        assert_eq!(out.unwrap().0, 200);
        assert_eq!(calls, 2);
    }

    #[test]
    fn does_not_retry_client_errors_capped_waits_or_when_disabled() {
        let (out, calls) = drive(vec![fast(400)], None);
        assert_eq!((out.unwrap().0, calls), (400, 1));
        let quota = Ok((429, headers(&[("retry-after", QUOTA_RETRY_AFTER_SECONDS)])));
        let (out, calls) = drive(vec![quota], None);
        assert_eq!((out.unwrap().0, calls), (429, 1));
        let (out, calls) = drive(vec![fast(500)], Some(0));
        assert_eq!((out.unwrap().0, calls), (500, 1));
    }

    #[test]
    fn retries_transport_failures_then_reports_them() {
        let (out, calls) = drive(
            vec![
                Err(SendFailure::Connection),
                Err(SendFailure::Connection),
                Err(SendFailure::Timeout),
            ],
            Some(2),
        );
        assert_eq!(out.unwrap_err().message(), "Request timed out.");
        assert_eq!(calls, 3);
    }
}
