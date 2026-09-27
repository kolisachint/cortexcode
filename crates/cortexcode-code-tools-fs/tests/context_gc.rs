//! Port of hoocode `test/context-gc.test.ts`, the "context GC x dedup
//! pointers" cases of `test/read-dedup.test.ts`,
//! `test/read-edit-gc-sequence-matrix.test.ts`, and the GC case of
//! `test/edit-encoding-recovery-loop.test.ts` (v0.5.89).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cortexcode_agent_types::AgentMessage;
use cortexcode_code_tool_api::{SessionBranch, ToolContext};
use cortexcode_code_tools_fs::*;
use serde_json::{json, Value};

fn msg(value: Value) -> AgentMessage {
    serde_json::from_value(value).expect("agent message")
}

fn assistant_call(id: &str, name: &str, path: &str) -> AgentMessage {
    msg(
        json!({"role": "assistant", "content": [{"type": "toolCall", "id": id, "name": name, "arguments": {"path": path}}]}),
    )
}

fn read_range_call(id: &str, path: &str, offset: Option<u64>, limit: Option<u64>) -> AgentMessage {
    let mut args = json!({"path": path});
    if let Some(o) = offset {
        args["offset"] = o.into();
    }
    if let Some(l) = limit {
        args["limit"] = l.into();
    }
    msg(
        json!({"role": "assistant", "content": [{"type": "toolCall", "id": id, "name": "read", "arguments": args}]}),
    )
}

fn bash_call(id: &str, command: &str) -> AgentMessage {
    msg(
        json!({"role": "assistant", "content": [{"type": "toolCall", "id": id, "name": "bash", "arguments": {"command": command}}]}),
    )
}

fn result(id: &str, tool: &str, text: &str, is_error: bool) -> AgentMessage {
    msg(
        json!({"role": "toolResult", "toolCallId": id, "toolName": tool, "content": [{"type": "text", "text": text}], "isError": is_error}),
    )
}

fn text_of(m: &AgentMessage) -> String {
    match m {
        AgentMessage::ToolResult(r) => r
            .content
            .iter()
            .filter_map(|c| match c {
                cortexcode_ai_types::Content::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

fn gc(messages: &[AgentMessage], pressure: f64) -> Vec<AgentMessage> {
    evict_superseded_reads(
        messages,
        &ContextGcOptions {
            cwd: PathBuf::from("/project"),
            budget_pressure: pressure,
        },
    )
    .unwrap_or_else(|| messages.to_vec())
}

fn changed(messages: &[AgentMessage]) -> bool {
    evict_superseded_reads(
        messages,
        &ContextGcOptions {
            cwd: PathBuf::from("/project"),
            budget_pressure: 0.0,
        },
    )
    .is_some()
}

// --- evictSupersededReads ---

#[test]
fn stubs_reads_superseded_by_edits_or_rereads() {
    let msgs = vec![
        assistant_call("r1", "read", "src/a.ts"),
        result("r1", "read", "old contents", false),
        assistant_call("e1", "edit", "src/a.ts"),
        result("e1", "edit", "ok", false),
    ];
    let out = gc(&msgs, 0.0);
    assert!(text_of(&out[1]).contains("[Superseded read of src/a.ts elided"));
    assert!(text_of(&out[1]).contains("modified after this read"));
    assert_eq!(text_of(&out[3]), "ok");

    let msgs = vec![
        assistant_call("r1", "read", "src/a.ts"),
        result("r1", "read", "first", false),
        assistant_call("r2", "read", "src/a.ts"),
        result("r2", "read", "second", false),
    ];
    let out = gc(&msgs, 0.0);
    assert!(text_of(&out[1]).contains("read again later"));
    assert_eq!(text_of(&out[3]), "second");

    let msgs = vec![
        assistant_call("r1", "read", "src/a.ts"),
        result("r1", "read", "contents", false),
        assistant_call("e1", "edit", "src/a.ts"),
        result("e1", "edit", "Could not find text", true),
    ];
    assert!(!changed(&msgs));

    let msgs = vec![
        assistant_call("r1", "read", "src/a.ts"),
        result("r1", "read", "a", false),
        assistant_call("e1", "edit", "src/b.ts"),
        result("e1", "edit", "ok", false),
    ];
    assert!(!changed(&msgs));

    let msgs = vec![
        assistant_call("r1", "read", "src/a.ts"),
        result("r1", "read", "a", false),
        assistant_call("e1", "edit", "/project/src/a.ts"),
        result("e1", "edit", "ok", false),
    ];
    assert!(text_of(&gc(&msgs, 0.0)[1]).starts_with("[Superseded read"));

    let msgs = vec![
        assistant_call("r1", "read", "src/a.ts"),
        result("r1", "read", "a", false),
    ];
    assert!(!changed(&msgs));
}

#[test]
fn read_ranges_decide_supersession() {
    let disjoint = vec![
        read_range_call("r1", "big.ts", Some(1), Some(40)),
        result("r1", "read", "lines 1-40", false),
        read_range_call("r2", "big.ts", Some(200), Some(60)),
        result("r2", "read", "lines 200-260", false),
    ];
    assert!(!changed(&disjoint));

    let alternating = vec![
        read_range_call("r1", "big.ts", Some(1), Some(40)),
        result("r1", "read", "A1", false),
        read_range_call("r2", "big.ts", Some(200), Some(60)),
        result("r2", "read", "B1", false),
        read_range_call("r3", "big.ts", Some(1), Some(40)),
        result("r3", "read", "A2", false),
        read_range_call("r4", "big.ts", Some(200), Some(60)),
        result("r4", "read", "B2", false),
    ];
    let out = gc(&alternating, 0.0);
    assert!(text_of(&out[1]).starts_with("[Superseded read"));
    assert!(text_of(&out[3]).starts_with("[Superseded read"));
    assert_eq!(text_of(&out[5]), "A2");
    assert_eq!(text_of(&out[7]), "B2");

    let overlap = vec![
        read_range_call("r1", "big.ts", Some(1), Some(40)),
        result("r1", "read", "1-40", false),
        read_range_call("r2", "big.ts", Some(30), Some(20)),
        result("r2", "read", "30-49", false),
    ];
    assert!(text_of(&gc(&overlap, 0.0)[1]).starts_with("[Superseded read"));

    let whole = vec![
        read_range_call("r1", "big.ts", Some(500), Some(10)),
        result("r1", "read", "500-509", false),
        read_range_call("r2", "big.ts", None, None),
        result("r2", "read", "whole", false),
    ];
    assert!(text_of(&gc(&whole, 0.0)[1]).starts_with("[Superseded read"));

    let mutated = vec![
        read_range_call("r1", "big.ts", Some(1), Some(40)),
        result("r1", "read", "1-40", false),
        read_range_call("r2", "big.ts", Some(200), Some(60)),
        result("r2", "read", "200-259", false),
        assistant_call("w1", "write", "big.ts"),
        result("w1", "write", "ok", false),
    ];
    let out = gc(&mutated, 0.0);
    assert!(
        text_of(&out[1]).starts_with("[Superseded read")
            && text_of(&out[3]).starts_with("[Superseded read")
    );
}

#[test]
fn bash_output_eviction_under_pressure() {
    let big = "x".repeat(3000);
    let run = |cmd: &str, text: &str, pressure: f64, is_error: bool| {
        text_of(
            &gc(
                &[bash_call("b1", cmd), result("b1", "bash", text, is_error)],
                pressure,
            )[1],
        )
    };
    assert_eq!(run("cat big.log", &big, 0.0, false), big);
    assert_eq!(run("ls", "small", 0.7, false), "small");
    assert_eq!(
        run("cat big.log", &big, 0.7, false),
        "[Bash output elided at 70% token budget — re-run if needed.]"
    );
    assert_eq!(run("psql -c 'select 1'", &big, 0.7, false), big);
    assert_eq!(run("rm -rf build", "removed", 0.9, false), "removed");
    assert_eq!(run("echo hi > out.txt", "", 0.9, false), "");
    assert_eq!(
        run("ls", "small", 0.85, false),
        "[Bash output elided at 85% token budget — re-run if needed.]"
    );
    assert_eq!(run("cat big.log", &big, 0.9, true), big);
}

#[test]
fn pressure_latch_tracks_rises_and_resets_after_a_collapse() {
    let mut latch = BudgetPressureLatch::new();
    assert_eq!(latch.update(50, 100), 0.5);
    assert_eq!(latch.update(40, 100), 0.5);
    assert_eq!(latch.update(30, 100), 0.3);
    assert_eq!(latch.update(500, 100), 1.0);
    assert_eq!(latch.update(5, 0), 0.0);
}

// --- read-dedup.test.ts: context GC x dedup pointers ---

#[test]
fn dedup_pointers_neither_supersede_nor_get_stubbed() {
    let pointer = "[Already in context: src/a.ts (lines 1-3) is unchanged since the read in call r1 — reuse that output instead of re-reading.]";
    let msgs = vec![
        assistant_call("r1", "read", "src/a.ts"),
        result("r1", "read", "contents", false),
        assistant_call("r2", "read", "src/a.ts"),
        result("r2", "read", pointer, false),
    ];
    assert!(!changed(&msgs));
    let msgs = vec![
        assistant_call("r1", "read", "src/a.ts"),
        result("r1", "read", "contents", false),
        assistant_call("r2", "read", "src/a.ts"),
        result("r2", "read", "newer", false),
    ];
    assert!(text_of(&gc(&msgs, 0.0)[1]).starts_with("[Superseded read"));
}

// --- read-edit-gc-sequence-matrix.test.ts ---

const INITIAL: &str = "line1 alpha\nline2 beta\nline3 gamma\nline4\u{a0}delta\n";

struct Branch(Arc<Mutex<Vec<AgentMessage>>>);

impl SessionBranch for Branch {
    fn get_branch(&self) -> Vec<Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|m| serde_json::to_value(m).unwrap())
            .collect()
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What `read` should deliver for `(offset, limit)` against `disk`.
fn delivered(disk: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    let lines: Vec<&str> = disk.split('\n').collect();
    let start = offset.unwrap_or(1) - 1;
    let end = limit.map_or(lines.len(), |l| (start + l).min(lines.len()));
    lines[start.min(lines.len())..end].join("\n")
}

fn strip_notice(text: &str) -> String {
    let re = regex::Regex::new(r"\n\n\[(Showing lines|\d+ more lines) [^\]]*\]\s*$").unwrap();
    re.replace(text, "").into_owned()
}

struct Transcript {
    cwd: PathBuf,
    msgs: Arc<Mutex<Vec<AgentMessage>>>,
    seq: usize,
}

/// A process-unique call id. read-dedup keeps its content stamps per call id
/// in a process-wide map (as hoocode does per module), and these tests run
/// in parallel, so ids must not repeat across transcripts.
fn next_call_id() -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    format!(
        "call-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

impl Transcript {
    fn push(&self, m: AgentMessage) -> usize {
        let mut msgs = self.msgs.lock().unwrap();
        msgs.push(m);
        msgs.len() - 1
    }

    fn read(&mut self, offset: Option<usize>, limit: Option<usize>) -> (usize, String) {
        self.seq += 1;
        let id = next_call_id();
        let mut args = json!({"path": "f.txt"});
        if let Some(o) = offset {
            args["offset"] = o.into();
        }
        if let Some(l) = limit {
            args["limit"] = l.into();
        }
        self.push(msg(json!({"role": "assistant", "content": [{"type": "toolCall", "id": id, "name": "read", "arguments": args}]})));
        let tool = create_read_tool_definition(
            &self.cwd,
            ReadToolOptions {
                dedup_reads: true,
                ..Default::default()
            },
        );
        let ctx = ToolContext {
            available_models: Vec::new(),
            cwd: None,
            session_file: None,
            model: None,
            session_manager: Some(Arc::new(Branch(self.msgs.clone()))),
        };
        let res = (tool.execute)(id.clone(), args, None, None, Some(&ctx)).unwrap();
        let text: String = res
            .content
            .iter()
            .filter_map(|c| match c {
                cortexcode_ai_types::Content::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        let index = self.push(result(&id, "read", &text, false));
        (index, text)
    }

    fn edit(&mut self, edits: Value) -> bool {
        self.seq += 1;
        let id = next_call_id();
        let args = json!({"path": "f.txt", "edits": edits});
        self.push(msg(json!({"role": "assistant", "content": [{"type": "toolCall", "id": id, "name": "edit", "arguments": args}]})));
        let tool = create_edit_tool_definition(&self.cwd, EditToolOptions::default());
        match (tool.execute)(id.clone(), args, None, None, None) {
            Ok(_) => {
                self.push(result(&id, "edit", "ok", false));
                true
            }
            Err(e) => {
                self.push(result(&id, "edit", &e.to_string(), true));
                false
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Op {
    WholeRead,
    RangeRead,
    Edit,
    InvisibleEdit,
    Gc,
    Rewrite,
}

const OPS: [Op; 6] = [
    Op::WholeRead,
    Op::RangeRead,
    Op::Edit,
    Op::InvisibleEdit,
    Op::Gc,
    Op::Rewrite,
];

fn label(seq: &[Op]) -> String {
    seq.iter()
        .map(|op| match op {
            Op::WholeRead => 'R',
            Op::RangeRead => 'r',
            Op::Edit => 'E',
            Op::InvisibleEdit => 'X',
            Op::Gc => 'G',
            Op::Rewrite => 'U',
        })
        .collect()
}

struct ReadLog {
    index: usize,
    offset: Option<usize>,
    limit: Option<usize>,
    text: String,
}

fn run_sequence(seq: &[Op]) -> Vec<String> {
    let dir = TempDir::new("sequence-matrix-");
    let abs = dir.0.join("f.txt");
    std::fs::write(&abs, INITIAL).unwrap();
    let mut t = Transcript {
        cwd: dir.0.clone(),
        msgs: Arc::default(),
        seq: 0,
    };
    let mut view: Option<Vec<AgentMessage>> = None;
    let mut log: Vec<ReadLog> = Vec::new();
    let mut edit_counter = 0;
    let mut last_edit_failed = false;
    let mut found = Vec::new();
    let name = label(seq);
    let mut flag = |rule: &str| found.push(format!("{name}: {rule}"));
    let is_stub = |s: &str| s.starts_with("[Superseded read");

    for op in seq {
        let disk = std::fs::read_to_string(&abs).unwrap();
        match op {
            Op::WholeRead | Op::RangeRead => {
                let (offset, limit) = if matches!(op, Op::WholeRead) {
                    (None, None)
                } else {
                    (Some(2), Some(2))
                };
                let (index, text) = t.read(offset, limit);
                if text.starts_with("[Already in context:") {
                    let covering = log.iter().rev().find(|r| {
                        let rs = r.offset.unwrap_or(1) as f64;
                        let re = r.limit.map_or(f64::INFINITY, |l| rs + l as f64);
                        let qs = offset.unwrap_or(1) as f64;
                        let qe = limit.map_or(f64::INFINITY, |l| qs + l as f64);
                        rs <= qs && re >= qe
                    });
                    match covering {
                        None => flag("a pointer named no earlier read that covers the request"),
                        Some(c) => {
                            if strip_notice(&c.text) != delivered(&disk, c.offset, c.limit) {
                                flag("a pointer claimed the file was unchanged when it was not");
                            }
                            let current = view
                                .clone()
                                .unwrap_or_else(|| t.msgs.lock().unwrap().clone());
                            if is_stub(&text_of(&current[c.index])) {
                                flag("a pointer named a read that GC had already elided");
                            }
                        }
                    }
                    if last_edit_failed {
                        flag("a re-read after a failed edit was answered with a pointer");
                    }
                } else {
                    if strip_notice(&text) != delivered(&disk, offset, limit) {
                        flag("a read returned content that is not what is on disk");
                    }
                    log.push(ReadLog {
                        index,
                        offset,
                        limit,
                        text,
                    });
                }
                last_edit_failed = false;
            }
            Op::Edit | Op::InvisibleEdit => {
                let targets = ["line1 alpha", "line2 beta", "line3 gamma"];
                let target = targets[edit_counter % targets.len()];
                edit_counter += 1;
                let edits = if matches!(op, Op::Edit) {
                    json!([{"oldText": target, "newText": format!("{target} EDIT{edit_counter}")}])
                } else {
                    json!([{"oldText": "line4 delta", "newText": "line4 delta"}])
                };
                let ok = t.edit(edits);
                let after = std::fs::read_to_string(&abs).unwrap();
                if ok && after == disk {
                    flag("an edit reported success without changing the file");
                }
                if !ok && after != disk {
                    flag("an edit that failed still wrote to the file");
                }
                if matches!(op, Op::InvisibleEdit) && ok {
                    flag("an edit invisible to matching reported success");
                }
                if matches!(op, Op::Edit) && !ok {
                    flag("a well-formed edit was rejected");
                }
                last_edit_failed = !ok;
            }
            Op::Rewrite => {
                std::fs::write(&abs, format!("{disk}line5 written by someone else\n")).unwrap();
                last_edit_failed = false;
            }
            Op::Gc => {
                let msgs = t.msgs.lock().unwrap().clone();
                let rewritten = evict_superseded_reads(
                    &msgs,
                    &ContextGcOptions {
                        cwd: dir.0.clone(),
                        budget_pressure: 0.0,
                    },
                );
                let out = rewritten.clone().unwrap_or_else(|| msgs.clone());
                for r in &log {
                    if !is_stub(&text_of(&out[r.index])) {
                        continue;
                    }
                    let later_mutate = msgs.iter().enumerate().any(|(j, m)| {
                        j > r.index
                            && matches!(m, AgentMessage::ToolResult(res) if (res.tool_name == "edit" || res.tool_name == "write") && !res.is_error)
                    });
                    let later_read = log.iter().any(|o| o.index > r.index);
                    if !later_mutate && !later_read {
                        flag("GC elided a read that nothing had superseded");
                    }
                }
                // Unchanged: hoocode gets the same (live) array back.
                view = rewritten;
            }
        }
    }
    found
}

fn all_sequences(max: usize) -> Vec<Vec<Op>> {
    let mut out = Vec::new();
    fn walk(prefix: Vec<Op>, max: usize, out: &mut Vec<Vec<Op>>) {
        if !prefix.is_empty() {
            out.push(prefix.clone());
        }
        if prefix.len() == max {
            return;
        }
        for op in OPS {
            let mut next = prefix.clone();
            next.push(op);
            walk(next, max, out);
        }
    }
    walk(Vec::new(), max, &mut out);
    out
}

#[test]
fn sequence_matrix_holds_every_invariant() {
    let sequences = all_sequences(3);
    assert_eq!(sequences.len(), 258);
    let violations: Vec<String> = sequences.iter().flat_map(|s| run_sequence(s)).collect();
    assert_eq!(violations, Vec::<String>::new());
}

#[test]
fn pointer_fires_while_unchanged_and_stops_when_changed() {
    let dir = TempDir::new("sequence-matrix-stamp-");
    let abs = dir.0.join("f.txt");
    std::fs::write(&abs, "line1\nline2\nline3\nline4\n").unwrap();
    let mut t = Transcript {
        cwd: dir.0.clone(),
        msgs: Arc::default(),
        seq: 0,
    };
    let kind = |text: String| {
        if text.starts_with("[Already in context:") {
            "pointer"
        } else {
            "content"
        }
    };
    assert_eq!(kind(t.read(None, None).1), "content");
    assert_eq!(kind(t.read(None, None).1), "pointer");
    assert_eq!(kind(t.read(Some(2), Some(2)).1), "pointer");
    std::fs::write(&abs, "line1\nline2 changed\nline3\nline4\n").unwrap();
    assert_eq!(kind(t.read(None, None).1), "content");
    assert_eq!(kind(t.read(None, None).1), "pointer");
    std::fs::write(&abs, "line1\nline2 CHANGED\nline3\nline4\n").unwrap();
    assert_eq!(kind(t.read(None, None).1), "content");
}

// --- edit-encoding-recovery-loop.test.ts: GC keeps evicting ---

#[test]
fn gc_evicts_the_superseded_read_once_the_loop_moves_on() {
    let dir = TempDir::new("edit-recovery-loop-");
    std::fs::write(dir.0.join("gc.ts"), "const v = 1;\n").unwrap();
    let mut t = Transcript {
        cwd: dir.0.clone(),
        msgs: Arc::default(),
        seq: 0,
    };
    // Transcript::read/edit target f.txt; drive gc.ts directly.
    let path = Path::new("gc.ts");
    let read = |t: &mut Transcript| {
        t.seq += 1;
        let id = next_call_id();
        let args = json!({"path": path.to_string_lossy()});
        t.push(msg(json!({"role": "assistant", "content": [{"type": "toolCall", "id": id, "name": "read", "arguments": args}]})));
        let tool = create_read_tool_definition(
            &t.cwd,
            ReadToolOptions {
                dedup_reads: true,
                ..Default::default()
            },
        );
        let ctx = ToolContext {
            available_models: Vec::new(),
            cwd: None,
            session_file: None,
            model: None,
            session_manager: Some(Arc::new(Branch(t.msgs.clone()))),
        };
        let res = (tool.execute)(id.clone(), args, None, None, Some(&ctx)).unwrap();
        let text = match &res.content[0] {
            cortexcode_ai_types::Content::Text(x) => x.text.clone(),
            _ => unreachable!(),
        };
        t.push(result(&id, "read", &text, false));
    };
    read(&mut t);
    t.seq += 1;
    let id = next_call_id();
    let args =
        json!({"path": "gc.ts", "edits": [{"oldText": "const v = 1;", "newText": "const v = 2;"}]});
    t.push(msg(json!({"role": "assistant", "content": [{"type": "toolCall", "id": id, "name": "edit", "arguments": args}]})));
    (create_edit_tool_definition(&t.cwd, EditToolOptions::default()).execute)(
        id.clone(),
        args,
        None,
        None,
        None,
    )
    .unwrap();
    t.push(result(&id, "edit", "ok", false));
    read(&mut t);
    let msgs = t.msgs.lock().unwrap().clone();
    let out = evict_superseded_reads(
        &msgs,
        &ContextGcOptions {
            cwd: dir.0.clone(),
            budget_pressure: 0.0,
        },
    )
    .unwrap();
    let reads: Vec<String> = out
        .iter()
        .filter(|m| matches!(m, AgentMessage::ToolResult(r) if r.tool_name == "read"))
        .map(text_of)
        .collect();
    assert!(reads[0].contains("Superseded read"));
    assert!(reads[1].contains("const v = 2;"));
}
