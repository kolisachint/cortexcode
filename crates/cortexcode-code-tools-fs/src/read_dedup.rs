//! Read de-duplication primitives (`core/tools/read-dedup.ts`).
//!
//! Two mechanisms share this module:
//!
//! - The post-hoc context GC, which stubs out a `read` result once a later
//!   overlapping read or an edit/write has superseded it.
//! - The at-call-time guard in the `read` tool, which short-circuits a read whose
//!   requested range is already fully covered by an earlier, still-live read in
//!   the current session, returning a pointer instead of re-fetching the file.
//!
//! Both reason about half-open line ranges `[start, end)`. Ranges are `f64`
//! because they come straight from JSON args (and a whole-file read ends at
//! infinity), exactly as in hoocode.

use cortexcode_code_tool_api::js_number;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Half-open line interval `[start, end)` a read call covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadRange {
    pub start: f64,
    pub end: f64,
}

/// A read with no offset/limit covers the whole file (open-ended).
pub const WHOLE_FILE_RANGE: ReadRange = ReadRange {
    start: 1.0,
    end: f64::INFINITY,
};

/// Derive the line range a read call covers from its `offset`/`limit` args.
/// Missing offset means "from line 1"; missing limit means "to end of file".
pub fn read_range_from_args(args: Option<&Value>) -> ReadRange {
    let number = |key: &str| {
        args.and_then(|a| a.get(key))
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
    };
    let offset = number("offset").filter(|o| *o > 0.0).unwrap_or(1.0);
    let end = number("limit")
        .map(|limit| offset + limit.max(0.0))
        .unwrap_or(f64::INFINITY);
    ReadRange { start: offset, end }
}

/// Whether two half-open line ranges intersect.
pub fn ranges_overlap(a: ReadRange, b: ReadRange) -> bool {
    a.start < b.end && b.start < a.end
}

/// Whether `outer` fully contains `inner`.
fn range_contains(outer: ReadRange, inner: ReadRange) -> bool {
    outer.start <= inner.start && outer.end >= inner.end
}

/// Marker prefix for the at-call dedup pointer. Kept stable so the context GC
/// can recognise a pointer result.
pub const DEDUP_POINTER_PREFIX: &str = "[Already in context:";

/// Whether a tool-result text is an at-call dedup pointer.
pub fn is_dedup_pointer_text(text: &str) -> bool {
    text.trim_start().starts_with(DEDUP_POINTER_PREFIX)
}

// ---------------------------------------------------------------------------
// Content stamps
// ---------------------------------------------------------------------------

/// The pointer claims the file "has not changed since" the read it names, so
/// each delivered read records a stamp (sha1) of the bytes it saw, keyed by
/// tool call id. Bounded; a missing entry counts as "cannot prove unchanged".
const MAX_TRACKED_READS: usize = 500;

/// Insertion-ordered like a JS `Map`.
fn stamps() -> &'static Mutex<Vec<(String, String)>> {
    static STAMPS: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();
    STAMPS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Remember the stamp of the file a read delivered.
pub fn record_read_stamp(call_id: &str, stamp: &str) {
    let mut map = stamps().lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= MAX_TRACKED_READS {
        map.remove(0);
    }
    match map.iter_mut().find(|(id, _)| id == call_id) {
        Some(entry) => entry.1 = stamp.to_string(),
        None => map.push((call_id.to_string(), stamp.to_string())),
    }
}

/// Whether the file a read delivered still stamps the same. False when no
/// stamp was recorded.
pub fn read_stamp_matches(call_id: &str, stamp: &str) -> bool {
    let map = stamps().lock().unwrap_or_else(|e| e.into_inner());
    map.iter()
        .find(|(id, _)| id == call_id)
        .is_some_and(|(_, recorded)| recorded == stamp)
}

/// Test seam: drop all recorded stamps.
pub fn clear_read_stamps() {
    stamps().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

// ---------------------------------------------------------------------------
// Covering read search
// ---------------------------------------------------------------------------

/// A covering earlier read, described for the pointer message.
#[derive(Debug, Clone, PartialEq)]
pub struct CoveringRead {
    /// Tool call id of the read being pointed at, used to check its stamp.
    pub call_id: String,
    /// The path as the earlier read spelled it.
    pub display: String,
    /// Delivered range start (1-indexed line).
    pub start: f64,
    /// Delivered range end (exclusive; infinity for a whole-file read).
    pub end: f64,
}

/// Options for [`find_covering_read`].
pub struct FindCoveringReadOptions<'a> {
    /// Resolved absolute path of the current read.
    pub resolved_path: &'a str,
    /// Range the current read is asking for.
    pub requested_range: ReadRange,
    /// Tool call id of the current read.
    pub current_call_id: &'a str,
    /// Resolve a raw read-arg path the same way the current read resolved its path.
    pub resolve_path: &'a dyn Fn(&str) -> String,
}

const READ_TOOL: &str = "read";

fn is_mutate_tool(name: &str) -> bool {
    name == "edit" || name == "write"
}

/// A session compaction entry: the boundary that trims the live context.
fn is_compaction_entry(entry: &Value) -> bool {
    entry.get("type").and_then(Value::as_str) == Some("compaction")
}

/// Accept either raw messages or session entries wrapping `.message`.
fn to_message(entry: &Value) -> Option<&Value> {
    if !entry.is_object() {
        return None;
    }
    if let Some(message) = entry.get("message").filter(|m| m.is_object()) {
        return Some(message);
    }
    entry.get("role").filter(|r| r.is_string()).map(|_| entry)
}

fn result_text(m: &Value) -> String {
    m.get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
                .map(|c| c.get("text").and_then(Value::as_str).unwrap_or(""))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn showing_regex() -> &'static regex_lite::Regex {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex_lite::Regex::new(r"\n\n\[Showing lines (\d+)-(\d+) of \d+[^\]]*\]\s*$").unwrap()
    })
}

fn first_line_regex() -> &'static regex_lite::Regex {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    RE.get_or_init(|| regex_lite::Regex::new(r"^\[Line \d+ is .+ exceeds .+ limit\.").unwrap())
}

/// Recover the range a read result actually delivered from its text. A
/// cap-truncated read announces `[Showing lines A-B of N ...]` (anchored to the
/// end so file content can't spoof it); a read whose first line alone exceeded
/// the byte cap delivered nothing; anything else delivered its declared range.
fn delivered_range(text: &str, declared: ReadRange) -> ReadRange {
    if let Some(caps) = showing_regex().captures(text) {
        let a: f64 = caps[1].parse().unwrap_or(f64::NAN);
        let b: f64 = caps[2].parse().unwrap_or(f64::NAN);
        if a.is_finite() && b.is_finite() && b >= a {
            return ReadRange {
                start: a,
                end: b + 1.0,
            };
        }
    }
    if first_line_regex().is_match(text) {
        return ReadRange {
            start: 1.0,
            end: 1.0,
        };
    }
    declared
}

struct PriorRead {
    index: usize,
    declared: ReadRange,
    delivered: ReadRange,
    display: String,
    call_id: String,
}

struct ReadCallInfo {
    resolved: String,
    display: String,
    declared: ReadRange,
}

/// Find the latest earlier read that (a) is for the same resolved path, (b)
/// delivered a range containing the requested range, and (c) is still live in
/// the outgoing context (no later edit/write and no later overlapping content
/// read supersedes it). `None` when the current read must actually run.
pub fn find_covering_read(
    entries: &[Value],
    opts: &FindCoveringReadOptions<'_>,
) -> Option<CoveringRead> {
    let mut resolve_cache: HashMap<String, String> = HashMap::new();
    let mut resolve = |raw: &str| -> String {
        if let Some(hit) = resolve_cache.get(raw) {
            return hit.clone();
        }
        let resolved = (opts.resolve_path)(raw);
        resolve_cache.insert(raw.to_string(), resolved.clone());
        resolved
    };

    let mut read_call: HashMap<String, ReadCallInfo> = HashMap::new();
    let mut mutate_call_path: HashMap<String, String> = HashMap::new();
    let mut reads: Vec<PriorRead> = Vec::new();
    let mut last_mutate_index: Option<usize> = None;
    // A failed edit/write means the model's copy of the text likely doesn't
    // match the file: that is when it most needs the real bytes.
    let mut last_failed_mutate_index: Option<usize> = None;
    let mut order = 0usize;

    for entry in entries {
        // Everything before a compaction boundary is summarized away.
        if is_compaction_entry(entry) {
            read_call.clear();
            mutate_call_path.clear();
            reads.clear();
            last_mutate_index = None;
            order = 0;
            continue;
        }
        let Some(m) = to_message(entry) else {
            continue;
        };
        let i = order;
        order += 1;
        let role = m.get("role").and_then(Value::as_str);
        let is_error = m.get("isError").and_then(Value::as_bool).unwrap_or(false);
        let tool_call_id = m
            .get("toolCallId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let tool_name = m.get("toolName").and_then(Value::as_str);

        if role == Some("assistant") {
            let Some(blocks) = m.get("content").and_then(Value::as_array) else {
                continue;
            };
            for b in blocks {
                if b.get("type").and_then(Value::as_str) != Some("toolCall") {
                    continue;
                }
                let Some(id) = b
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                else {
                    continue;
                };
                let arguments = b.get("arguments");
                let Some(raw) = arguments
                    .and_then(|a| a.get("path"))
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty())
                else {
                    continue;
                };
                let resolved = resolve(raw);
                let name = b.get("name").and_then(Value::as_str);
                if name == Some(READ_TOOL) {
                    read_call.insert(
                        id.to_string(),
                        ReadCallInfo {
                            resolved,
                            display: raw.to_string(),
                            declared: read_range_from_args(arguments),
                        },
                    );
                } else if name.is_some_and(is_mutate_tool) {
                    mutate_call_path.insert(id.to_string(), resolved);
                }
            }
        } else if role == Some("toolResult") && is_error {
            let Some(call_id) = tool_call_id else {
                continue;
            };
            if tool_name.is_some_and(is_mutate_tool)
                && mutate_call_path.get(call_id).map(String::as_str) == Some(opts.resolved_path)
            {
                last_failed_mutate_index = Some(i);
            }
        } else if role == Some("toolResult") {
            let Some(call_id) = tool_call_id else {
                continue;
            };
            if tool_name == Some(READ_TOOL) {
                if call_id == opts.current_call_id {
                    continue;
                }
                let Some(info) = read_call.get(call_id) else {
                    continue;
                };
                if info.resolved != opts.resolved_path {
                    continue;
                }
                let text = result_text(m);
                // A pointer fetched nothing: neither a candidate nor a superseder.
                if is_dedup_pointer_text(&text) {
                    continue;
                }
                reads.push(PriorRead {
                    index: i,
                    declared: info.declared,
                    delivered: delivered_range(&text, info.declared),
                    display: info.display.clone(),
                    call_id: call_id.to_string(),
                });
            } else if tool_name.is_some_and(is_mutate_tool)
                && mutate_call_path.get(call_id).map(String::as_str) == Some(opts.resolved_path)
            {
                last_mutate_index = Some(i);
            }
        }
    }

    // A read survives the GC iff no later mutate and no later read overlaps its
    // declared range, exactly the GC's own test.
    let survives_gc = |r: &PriorRead| -> bool {
        last_mutate_index.is_none_or(|m| m <= r.index)
            && !reads
                .iter()
                .any(|o| o.index > r.index && ranges_overlap(o.declared, r.declared))
    };

    if last_failed_mutate_index.is_some() {
        return None;
    }

    let mut best: Option<&PriorRead> = None;
    for r in &reads {
        if !range_contains(r.delivered, opts.requested_range) {
            continue;
        }
        if !survives_gc(r) {
            continue;
        }
        if best.is_none_or(|b| r.index > b.index) {
            best = Some(r);
        }
    }
    best.map(|b| CoveringRead {
        call_id: b.call_id.clone(),
        display: b.display.clone(),
        start: b.delivered.start,
        end: b.delivered.end,
    })
}

/// The parts of a covering read the pointer message renders.
#[derive(Debug, Clone, PartialEq)]
pub struct CoveringReadDisplay {
    pub display: String,
    pub start: f64,
    pub end: f64,
}

impl From<&CoveringRead> for CoveringReadDisplay {
    fn from(c: &CoveringRead) -> Self {
        Self {
            display: c.display.clone(),
            start: c.start,
            end: c.end,
        }
    }
}

/// Build the pointer text returned in place of a re-fetch.
pub fn build_dedup_pointer_text(covering: &CoveringReadDisplay) -> String {
    let where_ = if covering.end == f64::INFINITY {
        "the entire file".to_string()
    } else if covering.end - 1.0 > covering.start {
        format!(
            "lines {}-{}",
            js_number(covering.start),
            js_number(covering.end - 1.0)
        )
    } else {
        format!("line {}", js_number(covering.start))
    };
    format!(
        "{DEDUP_POINTER_PREFIX} {} ({where_}) was already read earlier in this session and has not changed since. Not re-fetched to save tokens — pass a different offset/limit, or edit the file, if you need other or newer content.]",
        covering.display
    )
}

#[cfg(test)]
mod tests {
    //! Port of `test/read-dedup.test.ts` (the `findCoveringRead` and pointer
    //! parts; the context-GC half arrives with the GC port).
    use super::*;
    use serde_json::json;

    fn read_call(id: &str, path: &str, args: Option<Value>) -> Value {
        let mut arguments = json!({ "path": path });
        if let Some(Value::Object(extra)) = args {
            for (k, v) in extra {
                arguments[k] = v;
            }
        }
        json!({
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": id, "name": "read", "arguments": arguments }]
        })
    }

    fn mutate_call(id: &str, path: &str, name: &str) -> Value {
        json!({
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": id, "name": name, "arguments": { "path": path } }]
        })
    }

    fn result(tool_call_id: &str, tool_name: &str, text: &str, is_error: bool) -> Value {
        json!({
            "role": "toolResult",
            "toolCallId": tool_call_id,
            "toolName": tool_name,
            "content": [{ "type": "text", "text": text }],
            "isError": is_error
        })
    }

    fn read_result(id: &str, text: &str) -> Value {
        result(id, "read", text, false)
    }

    fn cover(
        entries: &[Value],
        path: &str,
        args: Option<Value>,
        current: &str,
    ) -> Option<CoveringRead> {
        let identity = |raw: &str| raw.to_string();
        find_covering_read(
            entries,
            &FindCoveringReadOptions {
                resolved_path: path,
                requested_range: read_range_from_args(args.as_ref()),
                current_call_id: current,
                resolve_path: &identity,
            },
        )
    }

    #[test]
    fn covers_a_whole_file_re_read_from_a_prior_untruncated_whole_file_read() {
        let entries = [
            read_call("r1", "/f.txt", None),
            read_result("r1", "line1\nline2\nline3"),
        ];
        let covering = cover(&entries, "/f.txt", None, "cur").unwrap();
        assert_eq!(covering.end, f64::INFINITY);
    }

    #[test]
    fn covers_a_sub_range_request_from_a_prior_wider_read() {
        let entries = [
            read_call("r1", "/f.txt", None),
            read_result("r1", "content"),
        ];
        assert!(cover(
            &entries,
            "/f.txt",
            Some(json!({"offset": 10, "limit": 5})),
            "cur"
        )
        .is_some());
    }

    #[test]
    fn does_not_cover_when_the_prior_read_was_cap_truncated_below_the_request() {
        let entries = [
            read_call("r1", "/f.txt", None),
            read_result(
                "r1",
                "body\n\n[Showing lines 1-400 of 1000 (16.0KB limit). Use offset=401 to continue.]",
            ),
        ];
        assert!(cover(&entries, "/f.txt", None, "cur").is_none());
        assert!(cover(
            &entries,
            "/f.txt",
            Some(json!({"offset": 1, "limit": 50})),
            "cur"
        )
        .is_some());
    }

    #[test]
    fn does_not_cover_once_the_file_is_edited_after_the_read() {
        let entries = [
            read_call("r1", "/f.txt", None),
            read_result("r1", "content"),
            mutate_call("e1", "/f.txt", "edit"),
            result("e1", "edit", "edited", false),
        ];
        assert!(cover(&entries, "/f.txt", None, "cur").is_none());
    }

    #[test]
    fn does_not_cover_when_a_later_overlapping_read_has_superseded_the_covering_read() {
        let entries = [
            read_call("r1", "/f.txt", None),
            read_result("r1", "full content"),
            read_call("r2", "/f.txt", Some(json!({"offset": 5, "limit": 3}))),
            read_result("r2", "partial"),
        ];
        assert!(cover(&entries, "/f.txt", None, "cur").is_none());
    }

    #[test]
    fn ignores_an_earlier_dedup_pointer_as_a_candidate_and_as_a_superseder() {
        let pointer = build_dedup_pointer_text(&CoveringReadDisplay {
            display: "/f.txt".into(),
            start: 1.0,
            end: f64::INFINITY,
        });
        let entries = [
            read_call("r1", "/f.txt", None),
            read_result("r1", "full content"),
            read_call("r2", "/f.txt", None),
            read_result("r2", &pointer),
        ];
        let covering = cover(&entries, "/f.txt", None, "cur").unwrap();
        assert_eq!(covering.end, f64::INFINITY);
        assert_eq!(covering.call_id, "r1");
    }

    #[test]
    fn is_not_spoofed_by_a_showing_lines_string_inside_the_file_content() {
        let body = "intro\n[Showing lines 1-5 of 9999] appears in this doc\nmore body\nconclusion";
        let entries = [read_call("r1", "/f.txt", None), read_result("r1", body)];
        let covering = cover(&entries, "/f.txt", None, "cur").unwrap();
        assert_eq!(covering.end, f64::INFINITY);
    }

    #[test]
    fn predicts_gc_supersession_by_declared_range_not_delivered_range() {
        let entries = [
            read_call("r1", "/f.txt", None),
            read_result(
                "r1",
                "head\n\n[Showing lines 1-400 of 1000. Use offset=401 to continue.]",
            ),
            read_call("r2", "/f.txt", Some(json!({"offset": 500, "limit": 60}))),
            read_result("r2", "tail"),
        ];
        assert!(cover(
            &entries,
            "/f.txt",
            Some(json!({"offset": 1, "limit": 50})),
            "cur"
        )
        .is_none());
    }

    #[test]
    fn does_not_match_a_different_file() {
        let entries = [
            read_call("r1", "/a.txt", None),
            read_result("r1", "content"),
        ];
        assert!(cover(&entries, "/b.txt", None, "cur").is_none());
    }

    #[test]
    fn ignores_reads_before_a_compaction_boundary() {
        let entries = [
            read_call("r0", "/f.txt", None),
            read_result("r0", "pre-compaction content"),
            json!({ "type": "compaction", "id": "c1", "summary": "..." }),
        ];
        assert!(cover(&entries, "/f.txt", None, "cur").is_none());
    }

    #[test]
    fn covers_a_read_that_follows_the_last_compaction_boundary() {
        let entries = [
            read_call("r0", "/f.txt", None),
            read_result("r0", "pre-compaction content"),
            json!({ "type": "compaction", "id": "c1", "summary": "..." }),
            read_call("r1", "/f.txt", None),
            read_result("r1", "fresh content"),
        ];
        assert!(cover(&entries, "/f.txt", None, "cur").is_some());
    }

    #[test]
    fn ignores_error_results() {
        let entries = [
            read_call("r1", "/f.txt", None),
            result("r1", "read", "boom", true),
        ];
        assert!(cover(&entries, "/f.txt", None, "cur").is_none());
    }

    #[test]
    fn a_failed_edit_forces_a_real_read() {
        let entries = [
            read_call("r1", "/f.txt", None),
            read_result("r1", "content"),
            mutate_call("e1", "/f.txt", "edit"),
            result("e1", "edit", "Could not find the exact text", true),
        ];
        assert!(cover(&entries, "/f.txt", None, "cur").is_none());
    }

    #[test]
    fn accepts_session_entries_wrapping_messages() {
        let entries = [
            json!({ "type": "message", "message": read_call("r1", "/f.txt", None) }),
            json!({ "type": "message", "message": read_result("r1", "content") }),
        ];
        assert!(cover(&entries, "/f.txt", None, "cur").is_some());
    }

    #[test]
    fn first_line_exceeding_the_limit_delivered_nothing() {
        let entries = [
            read_call("r1", "/f.txt", None),
            read_result(
                "r1",
                "[Line 1 is 40.0KB, exceeds 32.0KB limit. Use bash: sed -n '1p' f | head -c 32768]",
            ),
        ];
        assert!(cover(&entries, "/f.txt", None, "cur").is_none());
    }

    #[test]
    fn round_trips_the_pointer_marker_for_whole_file_range_and_single_line() {
        let whole = CoveringReadDisplay {
            display: "/f.txt".into(),
            start: 1.0,
            end: f64::INFINITY,
        };
        let range = CoveringReadDisplay {
            display: "/f.txt".into(),
            start: 10.0,
            end: 21.0,
        };
        let single = CoveringReadDisplay {
            display: "/f.txt".into(),
            start: 7.0,
            end: 8.0,
        };
        assert!(build_dedup_pointer_text(&whole).contains("the entire file"));
        assert!(build_dedup_pointer_text(&range).contains("lines 10-20"));
        assert!(build_dedup_pointer_text(&single).contains("line 7"));
        for c in [&whole, &range, &single] {
            assert!(is_dedup_pointer_text(&build_dedup_pointer_text(c)));
        }
        assert!(!is_dedup_pointer_text("just some file contents"));
        assert_eq!(
            build_dedup_pointer_text(&whole),
            "[Already in context: /f.txt (the entire file) was already read earlier in this session and has not changed since. Not re-fetched to save tokens — pass a different offset/limit, or edit the file, if you need other or newer content.]"
        );
    }

    #[test]
    fn read_range_from_args_defaults() {
        assert_eq!(read_range_from_args(None), WHOLE_FILE_RANGE);
        let r = read_range_from_args(Some(&json!({"offset": 0, "limit": -5})));
        assert_eq!(
            r,
            ReadRange {
                start: 1.0,
                end: 1.0
            }
        );
        let r = read_range_from_args(Some(&json!({"offset": 3, "limit": 2})));
        assert_eq!(
            r,
            ReadRange {
                start: 3.0,
                end: 5.0
            }
        );
    }

    #[test]
    fn stamps_are_bounded_and_keyed_by_call_id() {
        // Unique ids so other tests sharing the global map are unaffected.
        record_read_stamp("stamp-test-a", "s1");
        assert!(read_stamp_matches("stamp-test-a", "s1"));
        assert!(!read_stamp_matches("stamp-test-a", "s2"));
        assert!(!read_stamp_matches("stamp-test-missing", "s1"));
        record_read_stamp("stamp-test-a", "s2");
        assert!(read_stamp_matches("stamp-test-a", "s2"));
        for i in 0..MAX_TRACKED_READS {
            record_read_stamp(&format!("stamp-test-fill-{i}"), "x");
        }
        assert!(!read_stamp_matches("stamp-test-a", "s2"));
        assert!(stamps().lock().unwrap().len() <= MAX_TRACKED_READS);
    }
}
