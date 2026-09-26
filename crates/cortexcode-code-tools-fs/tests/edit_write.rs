//! Port of the write/edit cases in hoocode `test/tools.test.ts` ("write
//! tool", "edit tool", "edit tool fuzzy matching", "edit tool CRLF
//! handling"), `test/edit-tool-legacy-input.test.ts`,
//! `test/edit-tool-preserves-untouched-lines.test.ts`,
//! `test/edit-encoding-recovery-loop.test.ts` (all but the context-GC case,
//! which is 10.2g's) and `test/file-mutation-queue.test.ts` (v0.5.89).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cortexcode_agent_types::AgentToolResult;
use cortexcode_ai_types::Content;
use cortexcode_code_tool_api::{SessionBranch, ToolContext, ToolDefinition, ToolError};
use cortexcode_code_tools_fs::*;
use serde_json::{json, Value};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("coding-agent-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run(tool: &ToolDefinition, args: Value) -> Result<AgentToolResult, ToolError> {
    (tool.execute)("call".into(), args, None, None, None)
}

fn edit_tool(cwd: &Path) -> ToolDefinition {
    create_edit_tool_definition(cwd, EditToolOptions::default())
}

fn write_tool(cwd: &Path) -> ToolDefinition {
    create_write_tool_definition(cwd, WriteToolOptions::default())
}

fn edit(tool: &ToolDefinition, path: &Path, edits: Value) -> Result<AgentToolResult, ToolError> {
    run(
        tool,
        json!({"path": path.to_string_lossy(), "edits": edits}),
    )
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn err(result: Result<AgentToolResult, ToolError>) -> String {
    result.unwrap_err().to_string()
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap()
}

// --- write tool ---

#[test]
fn write_writes_file_contents_and_creates_parents() {
    let d = TestDir::new();
    let file = d.file("write-test.txt");
    let result = run(
        &write_tool(&cwd()),
        json!({"path": file.to_string_lossy(), "content": "Test content"}),
    )
    .unwrap();
    assert!(text(&result).contains("Successfully wrote"));
    assert!(text(&result).contains(&*file.to_string_lossy()));
    assert_eq!(
        text(&result),
        format!("Successfully wrote 12 bytes to {}", file.display())
    );
    assert_eq!(result.details, Value::Null);

    let nested = d.file("nested/dir/test.txt");
    let result = run(
        &write_tool(&cwd()),
        json!({"path": nested.to_string_lossy(), "content": "Nested content"}),
    )
    .unwrap();
    assert!(text(&result).contains("Successfully wrote"));
    assert_eq!(read(&nested), "Nested content");
}

// --- edit tool ---

#[test]
fn edit_replaces_text_and_returns_a_diff() {
    let d = TestDir::new();
    let file = d.file("edit-test.txt");
    std::fs::write(&file, "Hello, world!").unwrap();
    let result = edit(
        &edit_tool(&cwd()),
        &file,
        json!([{"oldText": "world", "newText": "testing"}]),
    )
    .unwrap();
    assert!(text(&result).contains("Successfully replaced"));
    assert!(result.details["diff"].as_str().unwrap().contains("testing"));
    assert_eq!(result.details["firstChangedLine"], 1);
}

#[test]
fn edit_failures() {
    let d = TestDir::new();
    let tool = edit_tool(&cwd());
    let file = d.file("edit-test.txt");
    std::fs::write(&file, "Hello, world!").unwrap();
    assert!(err(edit(
        &tool,
        &file,
        json!([{"oldText": "nonexistent", "newText": "testing"}])
    ))
    .contains("Could not find the exact text"));

    let missing = d.file("missing.txt");
    assert_eq!(
        err(edit(
            &tool,
            &missing,
            json!([{"oldText": "hello", "newText": "world"}])
        )),
        format!(
            "Could not edit file: {}. Error code: ENOENT.",
            missing.display()
        )
    );

    std::fs::write(&file, "foo foo foo").unwrap();
    assert!(err(edit(
        &tool,
        &file,
        json!([{"oldText": "foo", "newText": "bar"}])
    ))
    .contains("Found 3 occurrences"));
    assert!(err(edit(
        &tool,
        &file,
        json!([{"oldText": "foo", "newText": "bar", "replaceAll": false}])
    ))
    .contains("Found 3 occurrences"));
    let result = edit(
        &tool,
        &file,
        json!([{"oldText": "foo", "newText": "bar", "replaceAll": true}]),
    )
    .unwrap();
    assert!(text(&result).contains("Successfully replaced"));
    assert_eq!(read(&file), "bar bar bar");

    std::fs::write(&file, "hello\nworld\n").unwrap();
    assert!(
        err(edit(&tool, &file, json!([]))).contains("edits must contain at least one replacement")
    );
}

#[test]
fn edit_multiple_disjoint_regions_against_the_original() {
    let d = TestDir::new();
    let tool = edit_tool(&cwd());
    let file = d.file("edit-multi.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\ndelta\n").unwrap();
    let result = edit(
        &tool,
        &file,
        json!([{"oldText": "alpha\n", "newText": "ALPHA\n"}, {"oldText": "gamma\n", "newText": "GAMMA\n"}]),
    )
    .unwrap();
    assert!(text(&result).contains("Successfully replaced 2 block(s)"));
    assert_eq!(read(&file), "ALPHA\nbeta\nGAMMA\ndelta\n");
    let diff = result.details["diff"].as_str().unwrap();
    assert!(diff.contains("ALPHA") && diff.contains("GAMMA"));

    std::fs::write(&file, "foo\nbar\nbaz\n").unwrap();
    edit(
        &tool,
        &file,
        json!([{"oldText": "foo\n", "newText": "foo bar\n"}, {"oldText": "bar\n", "newText": "BAR\n"}]),
    )
    .unwrap();
    assert_eq!(read(&file), "foo bar\nBAR\nbaz\n");

    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    assert!(err(edit(
        &tool,
        &file,
        json!([{"oldText": "one\ntwo\n", "newText": "ONE\nTWO\n"}, {"oldText": "two\nthree\n", "newText": "TWO\nTHREE\n"}]),
    ))
    .contains("overlap"));

    std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
    assert!(err(edit(
        &tool,
        &file,
        json!([{"oldText": "alpha\n", "newText": "ALPHA\n"}, {"oldText": "missing\n", "newText": "MISSING\n"}]),
    ))
    .contains("Could not find"));
    assert_eq!(read(&file), "alpha\nbeta\ngamma\n");
}

#[test]
fn edit_collapses_large_unchanged_gaps_in_multi_edit_diffs() {
    let d = TestDir::new();
    let file = d.file("edit-multi-large-gap.txt");
    let lines: Vec<String> = (1..=600).map(|i| format!("line {i:03}")).collect();
    std::fs::write(&file, format!("{}\n", lines.join("\n"))).unwrap();
    let result = edit(
        &edit_tool(&cwd()),
        &file,
        json!([
            {"oldText": "line 100\n", "newText": "LINE 100\n"},
            {"oldText": "line 300\n", "newText": "LINE 300\n"},
            {"oldText": "line 500\n", "newText": "LINE 500\n"}
        ]),
    )
    .unwrap();
    let diff = result.details["diff"].as_str().unwrap();
    for needle in ["LINE 100", "LINE 300", "LINE 500", "..."] {
        assert!(diff.contains(needle));
    }
    assert!(!diff.contains("line 250"));
    assert!(diff.split('\n').count() < 50);
}

#[cfg(unix)]
#[test]
fn edit_reports_access_codes_and_plain_errors() {
    use std::os::unix::fs::PermissionsExt;
    let d = TestDir::new();
    // SAFETY: geteuid has no preconditions.
    let is_root = unsafe { libc::geteuid() } == 0;
    if !is_root {
        let file = d.file("edit-readonly.txt");
        std::fs::write(&file, "hello\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert_eq!(
            err(edit(
                &edit_tool(&cwd()),
                &file,
                json!([{"oldText": "hello", "newText": "world"}])
            )),
            format!(
                "Could not edit file: {}. Error code: EACCES.",
                file.display()
            )
        );
        let unreadable = d.file("unreadable-preview.txt");
        std::fs::write(&unreadable, "hello\n").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o222)).unwrap();
        assert_eq!(
            compute_edits_diff(
                &unreadable.to_string_lossy(),
                &[Edit::new("hello", "world")],
                &d.0
            ),
            Err(format!(
                "Could not edit file: {}. Error code: EACCES.",
                unreadable.display()
            ))
        );
    }

    struct Offline;
    impl EditOperations for Offline {
        fn read_file(&self, _: &Path) -> Result<Vec<u8>, ToolError> {
            Ok(b"hello\n".to_vec())
        }
        fn write_file(&self, _: &Path, _: &str) -> Result<(), ToolError> {
            Ok(())
        }
        fn access(&self, _: &Path) -> Result<(), ToolError> {
            Err("disk offline".into())
        }
    }
    let tool = create_edit_tool_definition(
        &d.0,
        EditToolOptions {
            operations: Some(Arc::new(Offline)),
        },
    );
    assert_eq!(
        err(run(
            &tool,
            json!({"path": "broken.txt", "edits": [{"oldText": "hello", "newText": "world"}]})
        )),
        "Could not edit file: broken.txt. Error: disk offline."
    );

    let missing = d.file("missing-preview.txt");
    assert_eq!(
        compute_edits_diff(
            &missing.to_string_lossy(),
            &[Edit::new("hello", "world")],
            &d.0
        ),
        Err(format!(
            "Could not edit file: {}. Error code: ENOENT.",
            missing.display()
        ))
    );
}

// --- edit tool fuzzy matching ---

fn fuzzy_case(content: &str, edits: Value) -> Result<String, String> {
    let d = TestDir::new();
    let file = d.file("fuzzy.txt");
    std::fs::write(&file, content).unwrap();
    match edit(&edit_tool(&cwd()), &file, edits) {
        Ok(result) => {
            assert!(text(&result).contains("Successfully replaced"));
            Ok(read(&file))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[test]
fn fuzzy_matching() {
    assert_eq!(
        fuzzy_case(
            "line one   \nline two  \nline three\n",
            json!([{"oldText": "line one\nline two\n", "newText": "replaced\n"}])
        ),
        Ok("replaced\nline three\n".into())
    );
    assert_eq!(
        fuzzy_case(
            "你好，世界\n你好（世界）\n",
            json!([{"oldText": "你好,世界\n你好(世界)\n", "newText": "你好，hoo\n你好(hoo)\n"}])
        ),
        Ok("你好，hoo\n你好(hoo)\n".into())
    );
    assert_eq!(
        fuzzy_case(
            "ＡＢＣ１２３\ncafe\u{301}\n",
            json!([{"oldText": "ABC123\ncafé\n", "newText": "XYZ789\ncoffee\n"}])
        ),
        Ok("XYZ789\ncoffee\n".into())
    );
    assert!(fuzzy_case(
        "console.log(\u{2018}hello\u{2019});\n",
        json!([{"oldText": "console.log('hello');", "newText": "console.log('world');"}])
    )
    .unwrap()
    .contains("world"));
    assert!(fuzzy_case(
        "const msg = \u{201c}Hello World\u{201d};\n",
        json!([{"oldText": "const msg = \"Hello World\";", "newText": "const msg = \"Goodbye\";"}])
    )
    .unwrap()
    .contains("Goodbye"));
    assert!(fuzzy_case(
        "range: 1\u{2013}5\nbreak\u{2014}here\n",
        json!([{"oldText": "range: 1-5\nbreak-here", "newText": "range: 10-50\nbreak--here"}])
    )
    .unwrap()
    .contains("10-50"));
    assert!(fuzzy_case(
        "hello\u{a0}world\n",
        json!([{"oldText": "hello world", "newText": "hello universe"}])
    )
    .unwrap()
    .contains("universe"));
    assert_eq!(
        fuzzy_case(
            "const x = 'exact';\nconst y = 'other';\n",
            json!([{"oldText": "const x = 'exact';", "newText": "const x = 'changed';"}])
        ),
        Ok("const x = 'changed';\nconst y = 'other';\n".into())
    );
    assert!(fuzzy_case(
        "completely different content\n",
        json!([{"oldText": "this does not exist", "newText": "replacement"}])
    )
    .unwrap_err()
    .contains("Could not find the exact text"));
    assert!(fuzzy_case(
        "hello world   \nhello world\n",
        json!([{"oldText": "hello world", "newText": "replaced"}])
    )
    .unwrap_err()
    .contains("Found 2 occurrences"));
    assert_eq!(
        fuzzy_case(
            "console.log(\u{2018}hello\u{2019});\nhello\u{a0}world\n",
            json!([
                {"oldText": "console.log('hello');\n", "newText": "console.log('world');\n"},
                {"oldText": "hello world\n", "newText": "hello universe\n"}
            ])
        ),
        Ok("console.log('world');\nhello universe\n".into())
    );
    assert!(fuzzy_case(
        "function f() {\n\tif (x) {\n\t\treturn 1;\n\t}\n}\n",
        json!([{"oldText": "  if (x) {\n    return 1;\n  }", "newText": "  if (x) {\n    return 2;\n  }"}])
    )
    .unwrap()
    .contains("return 2;"));
    assert!(fuzzy_case(
        "\tfoo();\n\tbar();\n  foo();\n  bar();\n",
        json!([{"oldText": "foo();\nbar();", "newText": "baz();"}])
    )
    .unwrap_err()
    .contains("Found 2 occurrences"));
}

// --- edit tool CRLF handling ---

#[test]
fn crlf_and_bom_handling() {
    let second = json!([{"oldText": "second\n", "newText": "REPLACED\n"}]);
    assert_eq!(
        fuzzy_case(
            "line one\r\nline two\r\nline three\r\n",
            json!([{"oldText": "line two\n", "newText": "replaced line\n"}])
        ),
        Ok("line one\r\nreplaced line\r\nline three\r\n".into())
    );
    assert_eq!(
        fuzzy_case("first\r\nsecond\r\nthird\r\n", second.clone()),
        Ok("first\r\nREPLACED\r\nthird\r\n".into())
    );
    assert_eq!(
        fuzzy_case("first\nsecond\nthird\n", second.clone()),
        Ok("first\nREPLACED\nthird\n".into())
    );
    assert!(fuzzy_case(
        "hello\r\nworld\r\n---\r\nhello\nworld\n",
        json!([{"oldText": "hello\nworld\n", "newText": "replaced\n"}])
    )
    .unwrap_err()
    .contains("Found 2 occurrences"));
    assert_eq!(
        fuzzy_case("\u{feff}first\r\nsecond\r\nthird\r\n", second),
        Ok("\u{feff}first\r\nREPLACED\r\nthird\r\n".into())
    );
    assert_eq!(
        fuzzy_case(
            "\u{feff}first\r\nsecond\r\nthird\r\nfourth\r\n",
            json!([{"oldText": "second\n", "newText": "SECOND\n"}, {"oldText": "fourth\n", "newText": "FOURTH\n"}])
        ),
        Ok("\u{feff}first\r\nSECOND\r\nthird\r\nFOURTH\r\n".into())
    );
}

// --- edit-tool-legacy-input.test.ts ---

#[test]
fn prepare_arguments_folds_legacy_fields_and_parses_stringified_edits() {
    let definition = edit_tool(&cwd());
    let props = definition.parameters["properties"].as_object().unwrap();
    assert!(!props.contains_key("oldText") && !props.contains_key("newText"));
    let prepare = definition.prepare_arguments.clone().unwrap();
    assert_eq!(
        prepare(json!({"path": "file.txt", "oldText": "before", "newText": "after"})),
        json!({"path": "file.txt", "edits": [{"oldText": "before", "newText": "after"}]})
    );
    assert_eq!(
        prepare(
            json!({"path": "file.txt", "edits": [{"oldText": "a", "newText": "b"}], "oldText": "c", "newText": "d"})
        ),
        json!({"path": "file.txt", "edits": [{"oldText": "a", "newText": "b"}, {"oldText": "c", "newText": "d"}]})
    );
    let valid = json!({"path": "file.txt", "edits": [{"oldText": "a", "newText": "b"}]});
    assert_eq!(prepare(valid.clone()), valid);
    for garbage in [Value::Null, json!("garbage")] {
        assert_eq!(prepare(garbage.clone()), garbage);
    }
    assert_eq!(
        prepare(json!({"path": "file.txt", "edits": "[{\"oldText\":\"a\",\"newText\":\"b\"}]"})),
        json!({"path": "file.txt", "edits": [{"oldText": "a", "newText": "b"}]})
    );
    assert_eq!(
        prepare(json!({"path": "file.txt", "edits": "not json"})),
        json!({"path": "file.txt", "edits": "not json"})
    );

    let d = TestDir::new();
    std::fs::write(d.file("legacy.txt"), "before\n").unwrap();
    let definition = edit_tool(&d.0);
    let prepared = (definition.prepare_arguments.clone().unwrap())(
        json!({"path": "legacy.txt", "oldText": "before", "newText": "after"}),
    );
    let result = (definition.execute)(
        "tool-1".into(),
        prepared,
        None,
        None,
        Some(&ToolContext::default()),
    )
    .unwrap();
    assert_eq!(
        text(&result),
        "Successfully replaced 1 block(s) in legacy.txt."
    );
    assert_eq!(read(&d.file("legacy.txt")), "after\n");
}

// --- edit-tool-preserves-untouched-lines.test.ts ---

#[test]
fn fuzzy_edits_leave_untouched_lines_byte_identical() {
    let d = TestDir::new();
    let tool = edit_tool(&d.0);
    let file = d.file("aligned.ts");
    let original = "function fmt() {\n\tconst x = 1;\n\tconst pad = \"col1    col2    col3\";\n\tconst re = /a  b/;\n\treturn pad;\n}\n";
    std::fs::write(&file, original).unwrap();
    let result = edit(
        &tool,
        &file,
        json!([{"oldText": "  const x = 1;", "newText": "  const x = 2;"}]),
    )
    .unwrap();
    assert!(text(&result).contains("Successfully replaced"));
    let updated = read(&file);
    let (o, u): (Vec<&str>, Vec<&str>) = (
        original.split('\n').collect(),
        updated.split('\n').collect(),
    );
    assert_eq!(o.len(), u.len());
    for i in (0..o.len()).filter(|&i| i != 1) {
        assert_eq!(o[i], u[i]);
    }
    assert!(
        updated.contains("\"col1    col2    col3\"")
            && updated.contains("/a  b/")
            && updated.contains("const x = 2;")
    );

    let same_line = d.file("same-line.ts");
    std::fs::write(
        &same_line,
        "const a = \u{2018}x\u{2019}; const b = \u{2018}x\u{2019};\nconst c = 1;\n",
    )
    .unwrap();
    edit(
        &tool,
        &same_line,
        json!([{"oldText": "'x'", "newText": "'y'", "replaceAll": true}]),
    )
    .unwrap();
    assert_eq!(
        read(&same_line),
        "const a = 'y'; const b = 'y';\nconst c = 1;\n"
    );

    std::fs::write(
        &same_line,
        "const a = \u{2018}x\u{2019}; const b = \u{2018}x\u{2019};\n",
    )
    .unwrap();
    assert!(err(edit(
        &tool,
        &same_line,
        json!([{"oldText": "'x'", "newText": "'y'"}])
    ))
    .contains("Found 2 occurrences"));

    let noop = d.file("noop.ts");
    let original = "function f() {\n\tconst x = 1;\n\treturn x;\n}\n";
    std::fs::write(&noop, original).unwrap();
    assert!(err(edit(
        &tool,
        &noop,
        json!([{"oldText": "  const x = 1;", "newText": "  const x = 1;"}])
    ))
    .contains("No changes made"));
    assert_eq!(read(&noop), original);

    let reindent = d.file("reindent.ts");
    std::fs::write(&reindent, "function f() {\n\tconst x = 1;\n}\n").unwrap();
    edit(
        &tool,
        &reindent,
        json!([{"oldText": "\tconst x = 1;", "newText": "    const x = 1;"}]),
    )
    .unwrap();
    assert_eq!(read(&reindent), "function f() {\n    const x = 1;\n}\n");

    let diagram = d.file("diagram.md");
    let original = "# Title\n\n\tconst a = 1;\n\n  entry:  0     1     2\n\ndone\n";
    std::fs::write(&diagram, original).unwrap();
    let result = edit(
        &tool,
        &diagram,
        json!([{"oldText": "  const a = 1;", "newText": "  const a = 9;"}]),
    )
    .unwrap();
    let diff = result.details["diff"].as_str().unwrap();
    let changed_in_diff = diff
        .split('\n')
        .filter(|l| l.starts_with('+') || l.starts_with('-'))
        .count();
    let updated = read(&diagram);
    let changed_on_disk = original
        .split('\n')
        .zip(updated.split('\n'))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(changed_in_diff, 2);
    assert_eq!(changed_on_disk, 1);
    assert!(updated.contains("  entry:  0     1     2"));
}

#[test]
fn round_trips_text_the_read_tool_handed_back() {
    let d = TestDir::new();
    let file = d.file("doc.md");
    let mut lines = vec![
        "# Doc".to_string(),
        String::new(),
        "```ts".into(),
        "hoo.on('event', async () => {".into(),
        "  const { preparation } = event;".into(),
        "  ".into(),
        "  return preparation;".into(),
        "});".into(),
        "```".into(),
        String::new(),
        "     ↑         ↑      └────────┬────────┘".into(),
        "  prompt   from cmp      messages kept".into(),
        String::new(),
    ];
    lines.extend(
        (0..40).map(|i| format!("Filler line {i} to push this document past one kilobyte.")),
    );
    lines.push(String::new());
    let original = lines.join("\n");
    std::fs::write(&file, &original).unwrap();

    let read_tool = create_read_tool_definition(&d.0, ReadToolOptions::default());
    let shown = text(&run(&read_tool, json!({"path": "doc.md"})).unwrap());
    let shown_lines: Vec<&str> = shown.split('\n').collect();
    let start = shown_lines
        .iter()
        .position(|l| l.contains("hoo.on('event'"))
        .unwrap();
    let old_text = shown_lines[start..start + 4].join("\n");
    let new_text = old_text.replace(
        "const { preparation } = event;",
        "const { preparation } = event; // noted",
    );
    let result = run(
        &edit_tool(&d.0),
        json!({"path": "doc.md", "edits": [{"oldText": old_text, "newText": new_text}]}),
    )
    .unwrap();
    assert!(text(&result).contains("Successfully replaced"));
    let updated = read(&file);
    let changed = original
        .split('\n')
        .zip(updated.split('\n'))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(changed, 1);
    assert!(updated.contains("     ↑         ↑      └────────┬────────┘"));
    assert!(updated.contains("  prompt   from cmp      messages kept"));
}

// --- edit-encoding-recovery-loop.test.ts ---

/// A transcript plus read/edit drivers that append to it.
struct Harness {
    cwd: PathBuf,
    msgs: Arc<Mutex<Vec<Value>>>,
    seq: usize,
}

struct Branch(Arc<Mutex<Vec<Value>>>);

impl SessionBranch for Branch {
    fn get_branch(&self) -> Vec<Value> {
        self.0.lock().unwrap().clone()
    }
}

impl Harness {
    fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            msgs: Arc::default(),
            seq: 0,
        }
    }
    fn call(&mut self, name: &str, args: Value) -> String {
        self.seq += 1;
        // Unique across tests: read-dedup keeps a process-wide per-call-id stamp.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = format!(
            "{name}-{}-{}",
            self.seq,
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        self.msgs.lock().unwrap().push(json!({"role": "assistant", "content": [{"type": "toolCall", "id": id, "name": name, "arguments": args}]}));
        id
    }
    fn record(&self, id: &str, tool: &str, text: &str, is_error: bool) {
        self.msgs.lock().unwrap().push(json!({"role": "toolResult", "toolCallId": id, "toolName": tool, "content": [{"type": "text", "text": text}], "isError": is_error}));
    }
    fn read(&mut self, path: &str) -> String {
        let id = self.call("read", json!({"path": path}));
        let tool = create_read_tool_definition(
            &self.cwd,
            ReadToolOptions {
                dedup_reads: true,
                ..Default::default()
            },
        );
        let ctx = ToolContext {
            model: None,
            session_manager: Some(Arc::new(Branch(self.msgs.clone()))),
        };
        let result =
            (tool.execute)(id.clone(), json!({"path": path}), None, None, Some(&ctx)).unwrap();
        let out: String = result
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        self.record(&id, "read", &out, false);
        out
    }
    fn edit(&mut self, path: &str, edits: Value) -> (bool, String) {
        let args = json!({"path": path, "edits": edits});
        let id = self.call("edit", args.clone());
        match run(&edit_tool(&self.cwd), args) {
            Ok(result) => {
                let t = text(&result);
                self.record(&id, "edit", &t, false);
                (true, t)
            }
            Err(e) => {
                self.record(&id, "edit", &e.to_string(), true);
                (false, e.to_string())
            }
        }
    }
}

/// A model retyping a line: kept code points stay, the rest drift to ASCII.
fn retype(line: &str, keep: &[u32]) -> String {
    let mut out = line.to_owned();
    if !keep.contains(&0x00a0) {
        out = out.replace('\u{a0}', " ");
    }
    if !keep.contains(&0x2019) {
        out = out.replace(['\u{2018}', '\u{2019}'], "'");
    }
    if !keep.contains(&0x201d) {
        out = out.replace(['\u{201c}', '\u{201d}'], "\"");
    }
    if !keep.contains(&0x0009) {
        out = out.replace('\t', "  ");
    }
    out
}

fn code_points_named_in(message: &str) -> Vec<u32> {
    let re = regex_lite::Regex::new(r"U\+([0-9A-F]{4,6})").unwrap();
    re.captures_iter(message)
        .map(|c| u32::from_str_radix(&c[1], 16).unwrap())
        .collect()
}

#[test]
fn recovery_loop_converges_when_the_goal_is_the_character_the_model_cannot_retype() {
    let d = TestDir::new();
    std::fs::write(
        d.file("greet.ts"),
        "export function greet(name: string) {\n\tconst msg = \"Hello\u{a0}world\";\n}\n",
    )
    .unwrap();
    let mut h = Harness::new(&d.0);
    let mut keep = Vec::new();
    let mut applied = false;
    let mut rounds = 0;
    while !applied && rounds < 5 {
        rounds += 1;
        let seen = h.read("greet.ts");
        assert!(!seen.contains("[Already in context:"));
        let typed = retype(seen.split('\n').nth(1).unwrap(), &keep);
        let (ok, message) = h.edit(
            "greet.ts",
            json!([{"oldText": typed, "newText": typed.replace('\u{a0}', " ")}]),
        );
        if ok {
            applied = true;
            break;
        }
        keep.extend(code_points_named_in(&message));
    }
    assert!(applied);
    assert!(rounds <= 3);
    assert_eq!(
        read(&d.file("greet.ts")),
        "export function greet(name: string) {\n\tconst msg = \"Hello world\";\n}\n"
    );
}

#[test]
fn recovery_loop_error_texts_and_rereads() {
    let d = TestDir::new();
    std::fs::write(
        d.file("quote.ts"),
        "const label = \u{201c}widget\u{201d};\n",
    )
    .unwrap();
    let mut h = Harness::new(&d.0);
    h.read("quote.ts");
    let (ok, _) = h.edit(
        "quote.ts",
        json!([{"oldText": "const label = \"widget\";", "newText": "const label = \"widget\";"}]),
    );
    assert!(!ok);
    let again = h.read("quote.ts");
    assert!(!again.contains("[Already in context:"));
    assert!(again.contains("\u{201c}widget\u{201d}"));

    std::fs::write(d.file("nbsp.ts"), "\tconst msg = \"Hello\u{a0}world\";\n").unwrap();
    let (ok, message) = h.edit("nbsp.ts", json!([{"oldText": "  const msg = \"Hello world\";", "newText": "  const msg = \"Hello world\";"}]));
    assert!(!ok);
    assert!(message.contains("No changes made"));
    let named = code_points_named_in(&message);
    assert!(named.contains(&0x0009) && named.contains(&0x00a0));
    assert!(message.contains("NO-BREAK SPACE"));

    let original = "const a = 1;\nconst b = \u{201c}2\u{201d};\nconst c = 3;\n";
    std::fs::write(d.file("batch.ts"), original).unwrap();
    let (ok, message) = h.edit(
        "batch.ts",
        json!([{"oldText": "const a = 1;", "newText": "const a = 10;"}, {"oldText": "const b = \"2\";", "newText": "const b = \"2\";"}]),
    );
    assert!(!ok);
    assert!(message.contains("edits[1]"));
    assert_eq!(read(&d.file("batch.ts")), original);

    let original = "def handler(req):\n    return check(req)\n\ndef fallback(req):\n        return validate(req)\n";
    std::fs::write(d.file("nesting.py"), original).unwrap();
    let (ok, _) = h.edit("nesting.py", json!([{"oldText": "    return validate(req)", "newText": "    return validate(req, strict=True)"}]));
    assert!(!ok);
    assert_eq!(read(&d.file("nesting.py")), original);

    std::fs::write(
        d.file("ok.py"),
        "def handler(req):\n    return validate(req)\n",
    )
    .unwrap();
    let (ok, _) = h.edit("ok.py", json!([{"oldText": "    return validate(req)", "newText": "    return validate(req, strict=True)"}]));
    assert!(ok);
    assert_eq!(
        read(&d.file("ok.py")),
        "def handler(req):\n    return validate(req, strict=True)\n"
    );
}

// --- file-mutation-queue.test.ts ---

fn spawn_queued(
    path: PathBuf,
    order: Arc<Mutex<Vec<String>>>,
    name: &'static str,
    sleep_ms: u64,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        with_file_mutation_queue(&path, || {
            order.lock().unwrap().push(format!("{name}:start"));
            std::thread::sleep(Duration::from_millis(sleep_ms));
            order.lock().unwrap().push(format!("{name}:end"));
        })
    })
}

#[test]
fn queue_serializes_same_file_and_parallelizes_different_files() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let first = spawn_queued(
        "/tmp/file-mutation-queue-same".into(),
        order.clone(),
        "first",
        60,
    );
    std::thread::sleep(Duration::from_millis(10));
    let second = spawn_queued(
        "/tmp/file-mutation-queue-same".into(),
        order.clone(),
        "second",
        0,
    );
    first.join().unwrap();
    second.join().unwrap();
    assert_eq!(
        *order.lock().unwrap(),
        ["first:start", "first:end", "second:start", "second:end"]
    );

    let order = Arc::new(Mutex::new(Vec::new()));
    let a = spawn_queued("/tmp/file-mutation-queue-a".into(), order.clone(), "a", 60);
    let b = spawn_queued("/tmp/file-mutation-queue-b".into(), order.clone(), "b", 60);
    a.join().unwrap();
    b.join().unwrap();
    let order = order.lock().unwrap();
    let pos = |s: &str| order.iter().position(|x| x == s).unwrap();
    assert!(pos("a:start") < pos("a:end"));
    assert!(pos("b:start") < pos("b:end"));
    assert!(pos("b:start") < pos("a:end"));
}

#[cfg(unix)]
#[test]
fn queue_is_shared_by_symlink_aliases() {
    let d = TestDir::new();
    let target = d.file("target.txt");
    let alias = d.file("alias.txt");
    std::fs::write(&target, "hello\n").unwrap();
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let t = spawn_queued(target, order.clone(), "target", 60);
    std::thread::sleep(Duration::from_millis(10));
    let a = spawn_queued(alias, order.clone(), "alias", 0);
    t.join().unwrap();
    a.join().unwrap();
    assert_eq!(
        *order.lock().unwrap(),
        ["target:start", "target:end", "alias:start", "alias:end"]
    );
}

/// Local fs with delays, as the TS tests' slow operations.
struct SlowFs {
    read_delay: u64,
    write_delay: u64,
}

impl EditOperations for SlowFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        let data = std::fs::read(path)?;
        std::thread::sleep(Duration::from_millis(self.read_delay));
        Ok(data)
    }
    fn write_file(&self, path: &Path, content: &str) -> Result<(), ToolError> {
        std::thread::sleep(Duration::from_millis(self.write_delay));
        Ok(std::fs::write(path, content)?)
    }
    fn access(&self, path: &Path) -> Result<(), ToolError> {
        LocalEditOperations.access(path)
    }
}

impl WriteOperations for SlowFs {
    fn write_file(&self, path: &Path, content: &str) -> Result<(), ToolError> {
        std::thread::sleep(Duration::from_millis(self.write_delay));
        Ok(std::fs::write(path, content)?)
    }
    fn mkdir(&self, _: &Path) -> Result<(), ToolError> {
        Ok(())
    }
}

#[test]
fn parallel_edits_and_writes_on_one_file_are_serialized() {
    let d = TestDir::new();
    let file = d.file("parallel-edit.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
    let edit_tool = Arc::new(create_edit_tool_definition(
        &d.0,
        EditToolOptions {
            operations: Some(Arc::new(SlowFs {
                read_delay: 30,
                write_delay: 30,
            })),
        },
    ));
    let handles: Vec<_> = [("alpha", "ALPHA"), ("beta", "BETA")]
        .into_iter()
        .map(|(old, new)| {
            let (tool, path) = (edit_tool.clone(), file.clone());
            std::thread::spawn(move || {
                edit(&tool, &path, json!([{"oldText": old, "newText": new}])).unwrap()
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(read(&file), "ALPHA\nBETA\ngamma\n");

    let mixed = d.file("mixed.txt");
    std::fs::write(&mixed, "original\n").unwrap();
    let write_tool = create_write_tool_definition(
        &d.0,
        WriteToolOptions {
            operations: Some(Arc::new(SlowFs {
                read_delay: 0,
                write_delay: 10,
            })),
        },
    );
    let (tool, path) = (edit_tool.clone(), mixed.clone());
    let editing = std::thread::spawn(move || {
        edit(
            &tool,
            &path,
            json!([{"oldText": "original", "newText": "edited"}]),
        )
        .unwrap()
    });
    std::thread::sleep(Duration::from_millis(5));
    run(
        &write_tool,
        json!({"path": mixed.to_string_lossy(), "content": "replacement\n"}),
    )
    .unwrap();
    editing.join().unwrap();
    assert_eq!(read(&mixed), "replacement\n");
}
