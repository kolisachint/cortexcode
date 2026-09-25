//! Port of the `read tool` cases in hoocode `test/tools.test.ts`, plus edge
//! cases checked against the pinned hoocode (float/negative offset and limit,
//! directories, documents, dedup through a session branch).

use cortexcode_agent_types::{AgentTool, AgentToolResult};
use cortexcode_ai_types::{Content, Model};
use cortexcode_code_tool_api::{SessionBranch, ToolContext, ToolContextFactory, DEFAULT_MAX_LINES};
use cortexcode_code_tools_fs::read_dedup::clear_read_stamps;
use cortexcode_code_tools_fs::{create_read_tool, ReadToolOptions};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};

const PNG_1X1_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAX+XDSwAAAABJRU5ErkJggg==";

fn read_tool() -> AgentTool {
    create_read_tool(
        std::env::current_dir().unwrap(),
        ReadToolOptions::default(),
        None,
    )
}

fn exec(tool: &AgentTool, id: &str, args: Value) -> Result<AgentToolResult, String> {
    (tool.execute)(id.to_string(), args, None, None).map_err(|e| e.to_string())
}

/// Text blocks joined with "\n" (`getTextOutput`).
fn text_output(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn lines(n: usize, f: impl Fn(usize) -> String) -> String {
    (1..=n).map(f).collect::<Vec<_>>().join("\n")
}

fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn should_read_file_contents_that_fit_within_limits() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    let content = "Hello, world!\nLine 2\nLine 3";
    std::fs::write(&file, content).unwrap();

    let result = exec(&read_tool(), "test-call-1", json!({ "path": p(&file) })).unwrap();
    assert_eq!(text_output(&result), content);
    assert!(!text_output(&result).contains("Use offset="));
    assert!(result.details.is_null());
}

#[test]
fn should_handle_non_existent_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("nonexistent.txt");
    let err = exec(&read_tool(), "test-call-2", json!({ "path": p(&file) })).unwrap_err();
    assert_eq!(
        err,
        format!("ENOENT: no such file or directory, access '{}'", p(&file))
    );
}

#[test]
fn should_truncate_files_exceeding_line_limit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("large.txt");
    std::fs::write(&file, lines(2500, |i| format!("Line {i}"))).unwrap();

    let output =
        text_output(&exec(&read_tool(), "test-call-3", json!({ "path": p(&file) })).unwrap());
    assert!(output.contains("Line 1"));
    assert!(output.contains(&format!("Line {DEFAULT_MAX_LINES}")));
    assert!(!output.contains(&format!("Line {}", DEFAULT_MAX_LINES + 1)));
    assert!(output.contains(&format!(
        "[Showing lines 1-{DEFAULT_MAX_LINES} of 2500. Use offset={} to continue.]",
        DEFAULT_MAX_LINES + 1
    )));
}

#[test]
fn should_truncate_when_byte_limit_exceeded() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("large-bytes.txt");
    std::fs::write(
        &file,
        lines(500, |i| format!("Line {i}: {}", "x".repeat(200))),
    )
    .unwrap();

    let output =
        text_output(&exec(&read_tool(), "test-call-4", json!({ "path": p(&file) })).unwrap());
    assert!(output.contains("Line 1:"));
    let re = regex_lite::Regex::new(
        r"\[Showing lines 1-\d+ of 500 \(.* limit\)\. Use offset=\d+ to continue\.\]",
    )
    .unwrap();
    assert!(re.is_match(&output), "{}", &output[output.len() - 120..]);
    // 32KB fits 155 of these 208/209-byte lines.
    assert!(output
        .ends_with("[Showing lines 1-155 of 500 (32.0KB limit). Use offset=156 to continue.]"));
}

#[test]
fn should_handle_offset_parameter() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("offset-test.txt");
    std::fs::write(&file, lines(100, |i| format!("Line {i}"))).unwrap();

    let output = text_output(
        &exec(
            &read_tool(),
            "test-call-5",
            json!({ "path": p(&file), "offset": 51 }),
        )
        .unwrap(),
    );
    assert!(!output.contains("Line 50"));
    assert!(output.contains("Line 51"));
    assert!(output.contains("Line 100"));
    assert!(!output.contains("Use offset="));
}

#[test]
fn should_handle_limit_parameter() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("limit-test.txt");
    std::fs::write(&file, lines(100, |i| format!("Line {i}"))).unwrap();

    let output = text_output(
        &exec(
            &read_tool(),
            "test-call-6",
            json!({ "path": p(&file), "limit": 10 }),
        )
        .unwrap(),
    );
    assert!(output.contains("Line 1"));
    assert!(output.contains("Line 10"));
    assert!(!output.contains("Line 11"));
    assert!(output.contains("[90 more lines in file. Use offset=11 to continue.]"));
}

#[test]
fn should_handle_offset_and_limit_together() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("offset-limit-test.txt");
    std::fs::write(&file, lines(100, |i| format!("Line {i}"))).unwrap();

    let output = text_output(
        &exec(
            &read_tool(),
            "test-call-7",
            json!({ "path": p(&file), "offset": 41, "limit": 20 }),
        )
        .unwrap(),
    );
    assert!(!output.contains("Line 40"));
    assert!(output.contains("Line 41"));
    assert!(output.contains("Line 60"));
    assert!(!output.contains("Line 61"));
    assert!(output.contains("[40 more lines in file. Use offset=61 to continue.]"));
}

#[test]
fn should_show_error_when_offset_is_beyond_file_length() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("short.txt");
    std::fs::write(&file, "Line 1\nLine 2\nLine 3").unwrap();

    let err = exec(
        &read_tool(),
        "test-call-8",
        json!({ "path": p(&file), "offset": 100 }),
    )
    .unwrap_err();
    assert_eq!(err, "Offset 100 is beyond end of file (3 lines total)");
}

#[test]
fn should_include_truncation_details_when_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("large-file.txt");
    std::fs::write(&file, lines(2500, |i| format!("Line {i}"))).unwrap();

    let result = exec(&read_tool(), "test-call-9", json!({ "path": p(&file) })).unwrap();
    let truncation = &result.details["truncation"];
    assert_eq!(truncation["truncated"], true);
    assert_eq!(truncation["truncatedBy"], "lines");
    assert_eq!(truncation["totalLines"], 2500);
    assert_eq!(truncation["outputLines"], DEFAULT_MAX_LINES);
    assert_eq!(truncation["maxLines"], DEFAULT_MAX_LINES);
}

#[test]
fn should_detect_image_mime_type_from_file_magic_not_extension() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("image.txt");
    let png = base64::engine::general_purpose::STANDARD
        .decode(PNG_1X1_BASE64)
        .unwrap();
    std::fs::write(&file, png).unwrap();

    let result = exec(&read_tool(), "test-call-img-1", json!({ "path": p(&file) })).unwrap();
    assert!(matches!(result.content[0], Content::Text(_)));
    assert!(text_output(&result).contains("Read image file [image/png]"));
    let image = result
        .content
        .iter()
        .find_map(|c| match c {
            Content::Image(i) => Some(i),
            _ => None,
        })
        .expect("image block");
    assert_eq!(image.media_type, "image/png");
    // Within limits: passed through unchanged.
    assert_eq!(image.data, PNG_1X1_BASE64);
}

#[test]
fn should_treat_files_with_image_extension_but_non_image_content_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-an-image.png");
    std::fs::write(&file, "definitely not a png").unwrap();

    let result = exec(&read_tool(), "test-call-img-2", json!({ "path": p(&file) })).unwrap();
    assert!(text_output(&result).contains("definitely not a png"));
    assert!(!result
        .content
        .iter()
        .any(|c| matches!(c, Content::Image(_))));
}

// --- Edge cases (outputs recorded from the pinned hoocode) -----------------

fn abc_file() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rtest.txt");
    std::fs::write(&file, "a\nb\nc").unwrap();
    let path = p(&file);
    (dir, path)
}

fn read_text(args: Value) -> String {
    text_output(&exec(&read_tool(), "edge", args).unwrap())
}

#[test]
fn float_offset_slices_like_js() {
    let (_d, path) = abc_file();
    assert_eq!(read_text(json!({ "path": path, "offset": 2.5 })), "b\nc");
}

#[test]
fn negative_limit_slices_from_the_end_like_js() {
    let (_d, path) = abc_file();
    assert_eq!(
        read_text(json!({ "path": path, "limit": -1 })),
        "a\nb\n\n[4 more lines in file. Use offset=0 to continue.]"
    );
}

#[test]
fn negative_offset_starts_at_the_first_line() {
    let (_d, path) = abc_file();
    assert_eq!(
        read_text(json!({ "path": path, "offset": -3, "limit": 1 })),
        "a\n\n[2 more lines in file. Use offset=2 to continue.]"
    );
}

#[test]
fn fractional_limit_prints_js_numbers() {
    let (_d, path) = abc_file();
    assert_eq!(
        read_text(json!({ "path": path, "offset": 0, "limit": 1.5 })),
        "a\n\n[1.5 more lines in file. Use offset=2.5 to continue.]"
    );
}

#[test]
fn a_directory_fails_like_node() {
    let dir = tempfile::tempdir().unwrap();
    let err = exec(&read_tool(), "dir", json!({ "path": p(dir.path()) })).unwrap_err();
    assert_eq!(err, "EISDIR: illegal operation on a directory, read");
}

#[test]
fn structured_documents_get_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rdoc.PDF");
    std::fs::write(&file, "x").unwrap();
    assert_eq!(
        read_text(json!({ "path": p(&file) })),
        "[PDF document — not plain text; read cannot show it. Extract it with a converter via bash if you need its contents.]"
    );
}

#[test]
fn relative_paths_resolve_against_cwd() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let tool = create_read_tool(dir.path(), ReadToolOptions::default(), None);
    let result = exec(&tool, "rel", json!({ "path": "notes.txt" })).unwrap();
    // Byte-for-byte, trailing newline included.
    assert_eq!(text_output(&result), "alpha\nbeta\ngamma\n");
    let result = exec(
        &tool,
        "rel2",
        json!({ "path": "@notes.txt", "offset": "2", "limit": 1 }),
    )
    .unwrap();
    assert_eq!(
        text_output(&result),
        "beta\n\n[2 more lines in file. Use offset=3 to continue.]"
    );
}

#[test]
fn first_line_over_the_byte_limit_points_at_bash() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("wide.txt");
    std::fs::write(&file, format!("{}\nshort", "x".repeat(2048))).unwrap();
    let options = ReadToolOptions {
        max_output_bytes: 1024,
        ..Default::default()
    };
    let tool = create_read_tool(dir.path(), options, None);
    let result = exec(&tool, "wide", json!({ "path": "wide.txt" })).unwrap();
    assert_eq!(
        text_output(&result),
        "[Line 1 is 2.0KB, exceeds 1.0KB limit. Use bash: sed -n '1p' wide.txt | head -c 1024]"
    );
    assert_eq!(result.details["truncation"]["firstLineExceedsLimit"], true);
}

#[test]
fn description_reflects_the_output_caps() {
    let tool = read_tool();
    assert_eq!(
        tool.description,
        "Read the contents of a file. Supports text files and images (jpg, png, gif, webp). Images are sent as attachments. For text files, output is truncated to 800 lines or 32KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete."
    );
    assert_eq!(tool.parameters["required"], json!(["path"]));
}

#[test]
fn non_vision_models_get_a_note_with_the_image() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("pic.png");
    std::fs::write(
        &file,
        base64::engine::general_purpose::STANDARD
            .decode(PNG_1X1_BASE64)
            .unwrap(),
    )
    .unwrap();
    let factory: ToolContextFactory = Arc::new(|| ToolContext {
        model: Some(text_only_model()),
        session_manager: None,
    });
    let tool = create_read_tool(dir.path(), ReadToolOptions::default(), Some(factory));
    let result = exec(&tool, "img", json!({ "path": "pic.png" })).unwrap();
    assert_eq!(
        text_output(&result),
        "Read image file [image/png]\n[Current model does not support images. The image will be omitted from this request.]"
    );
    assert!(result
        .content
        .iter()
        .any(|c| matches!(c, Content::Image(_))));
}

fn text_only_model() -> Model {
    Model {
        id: "m".into(),
        name: "m".into(),
        api: "faux".into(),
        provider: "faux".into(),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: Default::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

// --- Dedup through a session branch ----------------------------------------

#[derive(Default)]
struct Branch(Mutex<Vec<Value>>);

impl SessionBranch for Branch {
    fn get_branch(&self) -> Vec<Value> {
        self.0.lock().unwrap().clone()
    }
}

impl Branch {
    fn record(&self, id: &str, args: Value, result: &AgentToolResult) {
        let mut entries = self.0.lock().unwrap();
        entries.push(json!({
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": id, "name": "read", "arguments": args }]
        }));
        entries.push(json!({
            "role": "toolResult",
            "toolCallId": id,
            "toolName": "read",
            "content": [{ "type": "text", "text": text_output(result) }],
            "isError": false
        }));
    }
}

#[test]
fn dedup_points_at_a_covering_read_while_the_file_is_unchanged() {
    clear_read_stamps();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\nthree").unwrap();
    let branch = Arc::new(Branch::default());
    let for_ctx = branch.clone();
    let factory: ToolContextFactory = Arc::new(move || ToolContext {
        model: None,
        session_manager: Some(for_ctx.clone()),
    });
    let options = ReadToolOptions {
        dedup_reads: true,
        ..Default::default()
    };
    let tool = create_read_tool(dir.path(), options, Some(factory));

    let args = json!({ "path": "f.txt" });
    let first = exec(&tool, "dd-1", args.clone()).unwrap();
    assert_eq!(text_output(&first), "one\ntwo\nthree");
    branch.record("dd-1", args.clone(), &first);

    let second = exec(
        &tool,
        "dd-2",
        json!({ "path": "f.txt", "offset": 2, "limit": 1 }),
    )
    .unwrap();
    assert_eq!(
        text_output(&second),
        "[Already in context: f.txt (the entire file) was already read earlier in this session and has not changed since. Not re-fetched to save tokens — pass a different offset/limit, or edit the file, if you need other or newer content.]"
    );

    // Changed on disk: the stamp no longer matches, so the read runs.
    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nthree").unwrap();
    let third = exec(
        &tool,
        "dd-3",
        json!({ "path": "f.txt", "offset": 2, "limit": 1 }),
    )
    .unwrap();
    assert_eq!(
        text_output(&third),
        "TWO\n\n[1 more lines in file. Use offset=3 to continue.]"
    );
}

#[test]
fn dedup_is_off_by_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "x").unwrap();
    let branch = Arc::new(Branch::default());
    let for_ctx = branch.clone();
    let factory: ToolContextFactory = Arc::new(move || ToolContext {
        model: None,
        session_manager: Some(for_ctx.clone()),
    });
    let tool = create_read_tool(dir.path(), ReadToolOptions::default(), Some(factory));
    let args = json!({ "path": "f.txt" });
    let first = exec(&tool, "off-1", args.clone()).unwrap();
    branch.record("off-1", args.clone(), &first);
    assert_eq!(text_output(&exec(&tool, "off-2", args).unwrap()), "x");
}
