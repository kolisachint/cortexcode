//! Level-1 fixture replay (migration task 13.2).
//!
//! Each scenario in `migration/tui-parity/replay.json` was recorded from the pinned
//! hoocode by `harness.py record`: the app ran headless (stdin a pipe or /dev/null,
//! stdout/stderr to files) against the scripted mock LLM. The recording holds
//! hoocode's raw output under `tests/fixtures/hoocode-0.5.89/replay/<scenario>/` and
//! the normalized rendering as the insta snapshot `snapshots/replay__<scenario>.snap`.
//!
//! This test runs `cortex` the same way against a port of `mockllm.py` and asserts
//! the same rendering: exit status, stdout (JSON lines masked like Level 2),
//! stderr, model requests (where the scenario compares them), session files (entry
//! ids remapped, timestamps masked) and work files.
//!
//! The snapshots are hoocode's output. Never accept cortex output into them
//! (`cargo insta accept`); re-record with `harness.py record` when the pin moves.
//! Each test first re-normalizes the raw hoocode recording with the Rust
//! normalizer below and checks it matches the snapshot, so this port and
//! `harness.py` cannot drift apart silently.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Map, Value};

const APP_CONFIG_DIR: &str = ".cortexcode";
const CHUNK_SIZE: usize = 8;
const CHUNK_DELAY: Duration = Duration::from_millis(10);
const DEFAULT_REQUEST_FIELDS: [&str; 4] = ["messages", "tools", "tool_choice", "model"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn parity_dir() -> PathBuf {
    root().join("migration/tui-parity")
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hoocode-0.5.89/replay")
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn manifest() -> Value {
    read_json(&parity_dir().join("replay.json"))
}

fn scenario(name: &str) -> Value {
    read_json(&parity_dir().join(format!("scenarios/{name}.json")))
}

// ---------------------------------------------------------------------------
// Python compatibility helpers
// ---------------------------------------------------------------------------

/// `str.splitlines()`.
fn py_splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let end = match c {
            '\r' => {
                if let Some(&(_, '\n')) = chars.peek() {
                    chars.next();
                    i + 2
                } else {
                    i + 1
                }
            }
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => i + c.len_utf8(),
            _ => continue,
        };
        out.push(&text[start..i]);
        start = end;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// `json.dumps(value)` with Python's defaults (`", "`/`": "` separators,
/// `ensure_ascii`): the mock streams tool-call arguments in this form.
fn py_dumps(value: &Value) -> String {
    fn string(s: &str, out: &mut String) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\x08' => out.push_str("\\b"),
                '\x0c' => out.push_str("\\f"),
                c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                    let mut buf = [0u16; 2];
                    for unit in c.encode_utf16(&mut buf) {
                        out.push_str(&format!("\\u{unit:04x}"));
                    }
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::Object(map) => {
                out.push('{');
                for (i, (k, x)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    string(k, out);
                    out.push_str(": ");
                    walk(x, out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (i, x) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    walk(x, out);
                }
                out.push(']');
            }
            Value::String(s) => string(s, out),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Null => out.push_str("null"),
            Value::Number(n) => out.push_str(&n.to_string()),
        }
    }
    let mut out = String::new();
    walk(value, &mut out);
    out
}

/// `json.dumps(v, ensure_ascii=False, separators=(",", ":"))` with key order kept.
fn compact(value: &Value) -> String {
    serde_json::to_string(value).expect("serialize")
}

fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = Map::new();
            for k in keys {
                sorted.insert(k.clone(), sort_keys(&map[k]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// `json.dumps(v, indent=1, sort_keys=True, ensure_ascii=False)`.
fn pretty_sorted(value: &Value) -> String {
    use serde::Serialize;
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(
        &mut buf,
        serde_json::ser::PrettyFormatter::with_indent(b" "),
    );
    sort_keys(value).serialize(&mut ser).expect("serialize");
    String::from_utf8(buf).expect("utf8")
}

// ---------------------------------------------------------------------------
// Normalizer (port of harness.py `Normalizer.apply_text`, `normalize_jsonl`)
// ---------------------------------------------------------------------------

struct Normalizer {
    rules: Vec<(Regex, String)>,
}

impl Normalizer {
    fn load(extra: Option<&Value>, paths: &[(&str, &str)]) -> Self {
        let global = read_json(&parity_dir().join("normalize.json"));
        let mut spec: Vec<Value> = global["rules"].as_array().cloned().unwrap_or_default();
        if let Some(Value::Array(extra)) = extra {
            spec.extend(extra.iter().cloned());
        }
        // Replay-only: the harness runs both apps on one day, but a snapshot
        // is compared on every later day, and the system prompt carries the date.
        spec.push(serde_json::json!({
            "pattern": r"Current date: \d{4}-\d{2}-\d{2}",
            "replace": "Current date: <DATE>",
        }));
        let rules = spec
            .iter()
            .map(|r| {
                let mut pattern = r["pattern"].as_str().expect("pattern").to_string();
                for (key, value) in paths {
                    pattern = pattern.replace(&format!("{{{key}}}"), &regex::escape(value));
                }
                let regex = Regex::new(&pattern).unwrap_or_else(|e| panic!("rule {pattern}: {e}"));
                (regex, py_template(r["replace"].as_str().expect("replace")))
            })
            .collect();
        Self { rules }
    }

    /// Rules apply line by line, in order; then each line is right-trimmed and
    /// trailing blank lines are dropped (`grid_text`).
    fn apply_text(&self, text: &str) -> String {
        let lines: Vec<String> = text
            .split('\n')
            .map(|line| {
                let mut line = line.to_string();
                for (regex, replace) in &self.rules {
                    line = regex.replace_all(&line, replace.as_str()).into_owned();
                }
                line.trim_end().to_string()
            })
            .collect();
        format!("{}\n", lines.join("\n").trim_end_matches('\n'))
    }
}

/// A Python `re` replacement template (`\1`) as a `regex` one (`${1}`).
fn py_template(template: &str) -> String {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' => out.push_str("$$"),
            '\\' => match chars.peek() {
                Some(d) if d.is_ascii_digit() => {
                    let mut group = String::new();
                    while let Some(d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                        group.push(*d);
                        chars.next();
                    }
                    out.push_str(&format!("${{{group}}}"));
                }
                Some('\\') => {
                    chars.next();
                    out.push('\\');
                }
                _ => out.push('\\'),
            },
            c => out.push(c),
        }
    }
    out
}

fn strs(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `normalize_jsonl`: one JSON value per line, key order kept; scalars masked by
/// key anywhere (`mask_keys`, default timestamp) and whole fields per event type
/// (`mask_fields`); then the text rules.
fn normalize_jsonl(raw: &str, normalizer: &Normalizer, opts: &Value) -> String {
    let mask: Vec<String> = match opts.get("mask_keys") {
        Some(keys) => strs(keys),
        None => vec!["timestamp".to_string()],
    };
    let fields = opts
        .get("mask_fields")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));

    fn walk(v: &Value, mask: &[String]) -> Value {
        match v {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, x)| {
                        let masked = mask.contains(k) && !x.is_object() && !x.is_array();
                        (
                            k.clone(),
                            if masked {
                                Value::from("<masked>")
                            } else {
                                walk(x, mask)
                            },
                        )
                    })
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(|x| walk(x, mask)).collect()),
            other => other.clone(),
        }
    }

    let mut lines = Vec::new();
    for line in py_splitlines(raw) {
        let Ok(mut value) = serde_json::from_str::<Value>(line) else {
            lines.push(format!("<not json> {line}"));
            continue;
        };
        if value.is_object() {
            let ty = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            for path in strs(fields.get(&ty).unwrap_or(&Value::Null)) {
                let pointer: String = path.split('.').map(|key| format!("/{key}")).collect();
                if let Some(slot) = value.pointer_mut(&pointer) {
                    *slot = Value::from("<masked>");
                }
            }
        }
        lines.push(compact(&walk(&value, &mask)));
    }
    let joined = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    normalizer.apply_text(&joined)
}

/// `normalize_session`: entry ids become `<id-N>` by first appearance, then
/// `normalize_jsonl`.
fn normalize_session(raw: &str, normalizer: &Normalizer, opts: &Value) -> String {
    let remap = strs(&opts["remap_keys"]);
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut lines = Vec::new();
    for line in py_splitlines(raw) {
        let mut value: Value = serde_json::from_str(line).expect("session line");
        for key in &remap {
            if let Some(Value::String(id)) = value.get(key) {
                let next = format!("<id-{}>", ids.len() + 1);
                let mapped = ids.entry(id.clone()).or_insert(next).clone();
                value[key] = Value::from(mapped);
            }
        }
        lines.push(compact(&value));
    }
    normalize_jsonl(&lines.join("\n"), normalizer, opts)
}

// ---------------------------------------------------------------------------
// Rendering (port of harness.py `render_replay`)
// ---------------------------------------------------------------------------

struct RawRun {
    paths: Vec<(String, String)>,
    exit_status: i32,
    stdout: String,
    stderr: String,
    /// Request log lines: `{"path": ..., "body": ...}`.
    requests: Vec<String>,
    sessions: Vec<String>,
    /// `work_files` in scenario order; `None` when the app left none.
    files: Vec<(String, Option<String>)>,
}

fn render(sc: &Value, raw: &RawRun) -> String {
    let paths: Vec<(&str, &str)> = raw
        .paths
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let normalizer = Normalizer::load(sc.get("normalize"), &paths);
    let mut parts = vec![format!("## exit\n{}\n", raw.exit_status)];
    match sc.get("stdout_jsonl") {
        Some(opts) if !opts.is_null() => parts.push(format!(
            "## stdout (jsonl)\n{}",
            normalize_jsonl(&raw.stdout, &normalizer, opts)
        )),
        _ => parts.push(format!("## stdout\n{}", normalizer.apply_text(&raw.stdout))),
    }
    parts.push(format!("## stderr\n{}", normalizer.apply_text(&raw.stderr)));
    if sc
        .get("compare_requests")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let fields: Vec<String> = match sc.get("request_fields") {
            Some(f) if !f.is_null() => strs(f),
            _ => DEFAULT_REQUEST_FIELDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        let reqs: Vec<Value> = raw
            .requests
            .iter()
            .map(|line| {
                let entry: Value = serde_json::from_str(line).expect("request log line");
                let body = &entry["body"];
                let mut picked = Map::new();
                for f in &fields {
                    if let Some(v) = body.get(f) {
                        picked.insert(f.clone(), v.clone());
                    }
                }
                Value::Object(picked)
            })
            .collect();
        parts.push(format!(
            "## requests\n{}",
            normalizer.apply_text(&pretty_sorted(&Value::Array(reqs)))
        ));
    }
    let session_opts = manifest()["session"].clone();
    let mut sessions: Vec<String> = raw
        .sessions
        .iter()
        .map(|s| normalize_session(s, &normalizer, &session_opts))
        .collect();
    sessions.sort();
    for (i, s) in sessions.iter().enumerate() {
        parts.push(format!("## session {}/{}\n{s}", i + 1, sessions.len()));
    }
    for (rel, text) in &raw.files {
        let body = match text {
            None => "<missing>\n".to_string(),
            Some(text) => {
                let value: Value = serde_json::from_str(text).expect("work file json");
                normalize_jsonl(&compact(&value), &normalizer, &sc["work_files"][rel])
            }
        };
        parts.push(format!("## file {rel}\n{body}"));
    }
    parts.join("\n")
}

/// The recorded hoocode run, from the fixture directory.
fn load_recording(name: &str, sc: &Value) -> RawRun {
    let dir = fixtures_dir().join(name);
    let meta = read_json(&dir.join("meta.json"));
    let read = |rel: &str| {
        fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| panic!("{name}/{rel}: {e}"))
    };
    let paths = ["HOME", "WORK", "TMP"]
        .iter()
        .map(|k| {
            (
                k.to_string(),
                meta["paths"][*k].as_str().expect("path").to_string(),
            )
        })
        .collect();
    let mut sessions = Vec::new();
    for i in 0.. {
        let p = dir.join(format!("sessions/{i}.jsonl"));
        if !p.exists() {
            break;
        }
        sessions.push(fs::read_to_string(p).expect("session"));
    }
    let files = work_file_keys(sc)
        .into_iter()
        .map(|rel| {
            let text = meta["files"][&rel].as_str().map(&read);
            (rel, text)
        })
        .collect();
    RawRun {
        paths,
        exit_status: meta["exit_status"].as_i64().expect("exit_status") as i32,
        stdout: read("stdout"),
        stderr: read("stderr"),
        requests: read("requests.jsonl").lines().map(str::to_string).collect(),
        sessions,
        files,
    }
}

fn work_file_keys(sc: &Value) -> Vec<String> {
    sc.get("work_files")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// The snapshot body (what follows the insta header).
fn snapshot_body(name: &str) -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/snapshots/replay__{name}.snap"));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run harness.py record {name})", path.display()));
    let rest = text.strip_prefix("---\n").expect("insta header");
    let end = rest.find("\n---\n").expect("insta header end");
    rest[end + 5..].to_string()
}

// ---------------------------------------------------------------------------
// Mock LLM (port of mockllm.py: OpenAI chat completions, streamed)
// ---------------------------------------------------------------------------

struct MockLlm {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
}

impl MockLlm {
    fn start(script: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let port = listener.local_addr().expect("addr").port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let turns = Arc::new(Mutex::new((script, 0usize)));
        let log2 = log.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, turns) = (log2.clone(), turns.clone());
                thread::spawn(move || {
                    let _ = serve(stream, &log, &turns);
                });
            }
        });
        Self { port, log }
    }

    fn requests(&self) -> Vec<String> {
        self.log.lock().expect("log").clone()
    }
}

type Turns = Mutex<(Vec<Value>, usize)>;

fn serve(stream: TcpStream, log: &Mutex<Vec<String>>, turns: &Turns) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut out = stream;
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line)? == 0 {
            return Ok(());
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        let mut length = 0usize;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header)?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((k, v)) = header.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut raw = vec![0u8; length];
        reader.read_exact(&mut raw)?;
        let trimmed = path.trim_end_matches('/');
        if method == "GET" {
            if trimmed.ends_with("/models") {
                send_json(
                    &mut out,
                    200,
                    &serde_json::json!({"object": "list", "data": [{"id": "mock-model", "object": "model"}]}),
                )?;
            } else {
                send_json(&mut out, 404, &serde_json::json!({"error": "not found"}))?;
            }
            continue;
        }
        let body: Value = if raw.is_empty() {
            Value::Object(Map::new())
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(
                |_| serde_json::json!({"_unparseable": String::from_utf8_lossy(&raw)}),
            )
        };
        log.lock()
            .expect("log")
            .push(compact(&serde_json::json!({"path": path, "body": body})));
        if !trimmed.ends_with("/chat/completions") {
            send_json(
                &mut out,
                404,
                &serde_json::json!({"error": format!("unsupported path {path}")}),
            )?;
            continue;
        }
        let turn = {
            let mut t = turns.lock().expect("turns");
            let turn =
                t.0.get(t.1)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"text": "[mockllm: script exhausted]"}));
            t.1 += 1;
            turn
        };
        if let Some(delay) = turn.get("delay_s").and_then(Value::as_f64) {
            thread::sleep(Duration::from_secs_f64(delay));
        }
        if let Some(error) = turn.get("error").and_then(Value::as_str) {
            let status = turn.get("status").and_then(Value::as_u64).unwrap_or(500) as u16;
            send_json(
                &mut out,
                status,
                &serde_json::json!({"error": {"message": error}}),
            )?;
            continue;
        }
        if !body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
            send_json(
                &mut out,
                400,
                &serde_json::json!({"error": {"message": "mockllm only supports stream=true"}}),
            )?;
            continue;
        }
        stream_turn(&mut out, &turn, &body)?;
    }
}

fn send_json(out: &mut TcpStream, status: u16, payload: &Value) -> std::io::Result<()> {
    let data = py_dumps(payload);
    write!(
        out,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{data}",
        data.len()
    )?;
    out.flush()
}

fn chunks(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(CHUNK_SIZE)
        .map(|c| c.iter().collect())
        .collect()
}

fn stream_turn(out: &mut TcpStream, turn: &Value, body: &Value) -> std::io::Result<()> {
    write!(
        out,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n"
    )?;
    let model = body
        .get("model")
        .cloned()
        .unwrap_or(Value::from("mock-model"));
    let delta = |d: Value, finish: Value| {
        serde_json::json!({"id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 0, "model": model,
            "choices": [{"index": 0, "delta": d, "finish_reason": finish}]})
    };
    let mut send = |line: String| -> std::io::Result<()> {
        let data = format!("data: {line}\n\n");
        write!(out, "{:x}\r\n{data}\r\n", data.len())?;
        out.flush()?;
        thread::sleep(CHUNK_DELAY);
        Ok(())
    };
    send(compact(&delta(
        serde_json::json!({"role": "assistant", "content": ""}),
        Value::Null,
    )))?;
    for (key, field) in [("thinking", "reasoning_content"), ("text", "content")] {
        if let Some(text) = turn
            .get(key)
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        {
            for part in chunks(text) {
                send(compact(&delta(
                    serde_json::json!({ field: part }),
                    Value::Null,
                )))?;
            }
        }
    }
    let calls = turn
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (i, call) in calls.iter().enumerate() {
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(format!("call_{i}"));
        let args = py_dumps(call.get("arguments").unwrap_or(&Value::Object(Map::new())));
        let start = serde_json::json!({"tool_calls": [{"index": i, "id": id, "type": "function",
            "function": {"name": call["name"], "arguments": ""}}]});
        send(compact(&delta(start, Value::Null)))?;
        for part in chunks(&args) {
            let d =
                serde_json::json!({"tool_calls": [{"index": i, "function": {"arguments": part}}]});
            send(compact(&delta(d, Value::Null)))?;
        }
    }
    let usage = turn.get("usage").cloned().unwrap_or(
        serde_json::json!({"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}),
    );
    let finish = if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    let mut last = delta(Value::Object(Map::new()), Value::from(finish));
    last["usage"] = usage;
    send(compact(&last))?;
    send("[DONE]".to_string())?;
    write!(out, "0\r\n\r\n")?;
    out.flush()
}

// ---------------------------------------------------------------------------
// Headless run (port of harness.py `run_headless`)
// ---------------------------------------------------------------------------

fn capture<R: Read + Send + 'static>(mut src: R) -> Arc<Mutex<Vec<u8>>> {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let sink = buf.clone();
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = src.read(&mut chunk) {
            if n == 0 {
                break;
            }
            sink.lock().expect("capture").extend_from_slice(&chunk[..n]);
        }
    });
    buf
}

fn text_of(buf: &Mutex<Vec<u8>>) -> String {
    String::from_utf8_lossy(&buf.lock().expect("capture")).into_owned()
}

fn wait_child(child: &mut Child, timeout: Duration) -> Option<i32> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Some(status.code().unwrap_or(-1));
        }
        if Instant::now() > deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn run_cortex(sc: &Value) -> RawRun {
    let tmp = tempfile::Builder::new()
        .prefix("replay-")
        .tempdir()
        .expect("tempdir");
    // The real path: on macOS the temp dir is under the /var -> /private/var
    // symlink and cortex reports its cwd resolved, which the masks must match.
    let root = tmp.path().canonicalize().expect("canonical tempdir");
    let (home, work) = (root.join("home"), root.join("work"));
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&work).expect("work");
    if let Some(files) = sc.get("files").and_then(Value::as_object) {
        for (rel, content) in files {
            let p = work.join(rel);
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(p, content.as_str().expect("file content")).expect("seed file");
        }
    }
    let mock = MockLlm::start(
        sc.get("llm")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    );
    let models = sc.get("models").cloned().unwrap_or(serde_json::json!([
        {"id": "mock-model", "name": "Mock Model", "contextWindow": 128000, "maxTokens": 4096}
    ]));
    let doc = serde_json::json!({"providers": {"mock": {
        "baseUrl": format!("http://127.0.0.1:{}/v1", mock.port),
        "api": "openai-completions", "apiKey": "mock-key", "models": models}}});
    for dir in [".hoocode", APP_CONFIG_DIR] {
        fs::create_dir_all(home.join(dir)).expect("config dir");
        fs::write(
            home.join(dir).join("models.json"),
            serde_json::to_string_pretty(&doc).expect("json"),
        )
        .expect("models");
        if let Some(settings) = sc.get("settings") {
            fs::write(
                home.join(dir).join("settings.json"),
                serde_json::to_string_pretty(settings).expect("json"),
            )
            .expect("settings");
        }
    }
    let (home_s, work_s, tmp_s) = (
        home.display().to_string(),
        work.display().to_string(),
        root.display().to_string(),
    );
    let mut env: Vec<(String, String)> = vec![
        ("HOME".into(), home_s.clone()),
        (
            "PATH".into(),
            std::env::var("PATH").unwrap_or("/usr/bin:/bin".into()),
        ),
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("LANG".into(), "C.UTF-8".into()),
        ("LC_ALL".into(), "C.UTF-8".into()),
        ("TZ".into(), "UTC".into()),
    ];
    if let Some(extra) = sc.get("env").and_then(Value::as_object) {
        for (k, v) in extra {
            let v = v.as_str().expect("env value");
            env.push((
                k.clone(),
                v.replace("{WORK}", &work_s)
                    .replace("{HOME}", &home_s)
                    .replace("{TMP}", &tmp_s),
            ));
        }
    }
    let bin = env!("CARGO_BIN_EXE_cortex");
    let command = |args: &[Value]| {
        let mut cmd = Command::new(bin);
        cmd.args(args.iter().map(|a| a.as_str().expect("arg")))
            .current_dir(&work)
            .env_clear()
            .envs(env.clone());
        cmd
    };
    for pre in sc
        .get("pre_runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let out = command(pre.as_array().expect("pre_run args"))
            .stdin(Stdio::null())
            .output()
            .expect("pre_run");
        assert!(
            out.status.success(),
            "pre_run {pre} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let default_args =
        serde_json::json!(["--offline", "--provider", "mock", "--model", "mock-model"]);
    let args = sc
        .get("args")
        .unwrap_or(&default_args)
        .as_array()
        .expect("args")
        .clone();
    let steps = sc["steps"].as_array().expect("steps").clone();
    let interactive = steps
        .iter()
        .any(|s| s.get("type").is_some() || s.get("keys").is_some());
    let mut child = command(&args)
        .stdin(if interactive {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cortex");
    let stdout = capture(child.stdout.take().expect("stdout"));
    let stderr = capture(child.stderr.take().expect("stderr"));
    let mut stdin = child.stdin.take();
    for (i, step) in steps.iter().enumerate() {
        let timeout =
            Duration::from_secs_f64(step.get("timeout").and_then(Value::as_f64).unwrap_or(15.0));
        if let Some(text) = step.get("type").and_then(Value::as_str) {
            let pipe = stdin.as_mut().expect("stdin open");
            pipe.write_all(text.as_bytes()).expect("write stdin");
            pipe.flush().expect("flush stdin");
        } else if let Some(keys) = step.get("keys") {
            let keys = keys
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![keys.clone()]);
            for key in keys {
                match key.as_str() {
                    Some("C-d") => drop(stdin.take()),
                    Some("Enter") => {
                        let pipe = stdin.as_mut().expect("stdin open");
                        pipe.write_all(b"\n").expect("write stdin");
                        pipe.flush().expect("flush stdin");
                    }
                    other => panic!("key {other:?} is not replayable"),
                }
            }
        } else if let Some(pattern) = step.get("wait_stdout").and_then(Value::as_str) {
            let regex = regex::RegexBuilder::new(pattern)
                .multi_line(true)
                .build()
                .expect("wait_stdout regex");
            let deadline = Instant::now() + timeout;
            while !regex.is_match(&text_of(&stdout)) {
                let exited = child.try_wait().expect("try_wait").is_some();
                if Instant::now() > deadline || (exited && !regex.is_match(&text_of(&stdout))) {
                    let _ = child.kill();
                    panic!(
                        "step {i}: stdout did not match {pattern:?}\nstdout:\n{}\nstderr:\n{}",
                        text_of(&stdout),
                        text_of(&stderr)
                    );
                }
                thread::sleep(Duration::from_millis(50));
            }
        } else if step.get("wait_exit").is_some() {
            if wait_child(&mut child, timeout).is_none() {
                let _ = child.kill();
                panic!(
                    "step {i}: cortex did not exit within {timeout:?}\nstderr:\n{}",
                    text_of(&stderr)
                );
            }
        } else if let Some(secs) = step.get("sleep").and_then(Value::as_f64) {
            thread::sleep(Duration::from_secs_f64(secs));
        } else if step.get("snapshot").is_none() {
            panic!("step {step} is not replayable");
        }
    }
    drop(stdin);
    let Some(exit_status) = wait_child(&mut child, Duration::from_secs(30)) else {
        let _ = child.kill();
        panic!("cortex did not exit\nstderr:\n{}", text_of(&stderr));
    };
    // Let the capture threads drain the closed pipes.
    thread::sleep(Duration::from_millis(100));

    let sessions_dir = home.join(APP_CONFIG_DIR).join("sessions");
    let mut session_paths = Vec::new();
    collect_jsonl(&sessions_dir, &mut session_paths);
    session_paths.sort();
    let files = work_file_keys(sc)
        .into_iter()
        .map(|rel| {
            let text = fs::read_to_string(work.join(rel.replace("{config}", APP_CONFIG_DIR))).ok();
            (rel, text)
        })
        .collect();
    RawRun {
        paths: vec![
            ("HOME".into(), home_s),
            ("WORK".into(), work_s),
            ("TMP".into(), tmp_s),
        ],
        exit_status,
        stdout: text_of(&stdout),
        stderr: text_of(&stderr),
        requests: mock.requests(),
        sessions: session_paths
            .iter()
            .map(|p| fs::read_to_string(p).expect("session"))
            .collect(),
        files,
    }
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn replay(name: &str) {
    let sc = scenario(name);
    let expected = snapshot_body(name);
    let hoocode = render(&sc, &load_recording(name, &sc));
    assert_eq!(
        hoocode.trim_end(),
        expected.trim_end(),
        "normalizer drift: the Rust port renders hoocode's recording of {name} differently from harness.py"
    );
    let cortex = render(&sc, &run_cortex(&sc));
    insta::with_settings!({ snapshot_path => "snapshots", omit_expression => true }, {
        insta::assert_snapshot!(name, cortex);
    });
}

macro_rules! replay_tests {
    ($($test:ident => $name:literal),* $(,)?) => {
        $(#[test] fn $test() { replay($name); })*

        #[test]
        fn every_manifest_scenario_has_a_test() {
            let listed: Vec<String> = strs(&manifest()["scenarios"]);
            assert_eq!(listed, vec![$($name.to_string()),*], "replay.json and replay.rs list different scenarios");
        }
    };
}

replay_tests! {
    json_basic => "json-basic",
    json_error => "json-error",
    json_subagent_child => "json-subagent-child",
    list_models => "list-models",
    print_basic => "print-basic",
    print_continue => "print-continue",
    print_error => "print-error",
    print_retry => "print-retry",
    print_tool_bash_light => "print-tool-bash-light",
    print_tool_edit_light => "print-tool-edit-light",
    print_tool_invalid_light => "print-tool-invalid-light",
    print_tool_read_light => "print-tool-read-light",
    rpc_basic => "rpc-basic",
    rpc_session => "rpc-session",
}

#[test]
fn py_template_converts_groups() {
    assert_eq!(py_template(r"<exited status=\1>"), "<exited status=${1}>");
    assert_eq!(py_template("$5"), "$$5");
}

#[test]
fn py_dumps_matches_python_defaults() {
    let v: Value =
        serde_json::from_str("{\"path\":\"notes.txt\",\"limit\":2,\"s\":\"\u{e9}\\n\"}").unwrap();
    assert_eq!(
        py_dumps(&v),
        "{\"path\": \"notes.txt\", \"limit\": 2, \"s\": \"\\u00e9\\n\"}"
    );
}
