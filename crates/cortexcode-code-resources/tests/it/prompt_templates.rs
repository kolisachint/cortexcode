//! Port of hoocode `packages/coding-agent/test/prompt-templates.test.ts` (v0.5.89).
//! The table-driven cases are every literal `substituteArgs` / `parseCommandArgs`
//! expectation of the TS file, extracted verbatim.

use cortexcode_code_resources::source_info::{SourceInfo, SourceOrigin, SourceScope};
use cortexcode_code_resources::{
    load_prompt_templates, parse_command_args, substitute_args, try_expand_prompt_template,
    LoadPromptTemplatesOptions, PromptTemplate, PromptTemplateType,
};

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

#[test]
fn substitute_args_literal_cases() {
    let cases: &[(&str, &[&str], &str)] = &[
        ("Test: $ARGUMENTS", &["a", "b", "c"], "Test: a b c"),
        ("Test: $@", &["a", "b", "c"], "Test: a b c"),
        ("$ARGUMENTS", &["$1", "$ARGUMENTS"], "$1 $ARGUMENTS"),
        ("$@", &["$100", "$1"], "$100 $1"),
        ("$ARGUMENTS", &["$100", "$1"], "$100 $1"),
        (
            "$1: $ARGUMENTS",
            &["prefix", "a", "b"],
            "prefix: prefix a b",
        ),
        ("$1: $@", &["prefix", "a", "b"], "prefix: prefix a b"),
        ("Test: $ARGUMENTS", &[], "Test: "),
        ("Test: $@", &[], "Test: "),
        ("Test: $1", &[], "Test: "),
        ("$ARGUMENTS and $ARGUMENTS", &["a", "b"], "a b and a b"),
        ("$@ and $@", &["a", "b"], "a b and a b"),
        ("$@ and $ARGUMENTS", &["a", "b"], "a b and a b"),
        (
            "$1 $2: $ARGUMENTS",
            &["arg100", "@user"],
            "arg100 @user: arg100 @user",
        ),
        ("$1 $2 $3 $4 $5", &["a", "b"], "a b   "),
        ("$ARGUMENTS", &["日本語", "🎉", "café"], "日本語 🎉 café"),
        (
            "$1 $2",
            &["line1\nline2", "tab\tthere"],
            "line1\nline2 tab\tthere",
        ),
        ("$1$2", &["a", "b"], "ab"),
        (
            "$ARGUMENTS",
            &["first arg", "second arg"],
            "first arg second arg",
        ),
        ("Test: $ARGUMENTS", &["only"], "Test: only"),
        ("Test: $@", &["only"], "Test: only"),
        ("$0", &["a", "b"], ""),
        ("$1.5", &["a"], "a.5"),
        ("pre$ARGUMENTS", &["a", "b"], "prea b"),
        ("pre$@", &["a", "b"], "prea b"),
        ("$ARGUMENTS", &["a", "", "c"], "a  c"),
        (
            "$ARGUMENTS",
            &["  leading  ", "trailing  "],
            "  leading   trailing  ",
        ),
        (
            "Prefix $ARGUMENTS suffix",
            &["ARGUMENTS"],
            "Prefix ARGUMENTS suffix",
        ),
        ("$A $$ $ $ARGS", &["a"], "$A $$ $ $ARGS"),
        (
            "$arguments $Arguments $ARGUMENTS",
            &["a", "b"],
            "$arguments $Arguments a b",
        ),
        ("$1 $2 $3", &["a", "b", "c"], "a b c"),
        ("Price: \\$100", &[], "Price: \\"),
        (
            "$1: $@ ($ARGUMENTS)",
            &["first", "second", "third"],
            "first: first second third (first second third)",
        ),
        ("Just plain text", &["a", "b"], "Just plain text"),
        ("$1 $2 $@", &["a", "b", "c"], "a b a b c"),
        ("${@:2}", &["a", "b", "c", "d"], "b c d"),
        ("${@:1}", &["a", "b", "c"], "a b c"),
        ("${@:3}", &["a", "b", "c", "d"], "c d"),
        ("${@:2:2}", &["a", "b", "c", "d"], "b c"),
        ("${@:1:1}", &["a", "b", "c"], "a"),
        ("${@:3:1}", &["a", "b", "c", "d"], "c"),
        ("${@:2:3}", &["a", "b", "c", "d", "e"], "b c d"),
        ("${@:99}", &["a", "b"], ""),
        ("${@:5}", &["a", "b"], ""),
        ("${@:10:5}", &["a", "b"], ""),
        ("${@:2:0}", &["a", "b", "c"], ""),
        ("${@:1:0}", &["a", "b"], ""),
        ("${@:2:99}", &["a", "b", "c"], "b c"),
        ("${@:1:10}", &["a", "b"], "a b"),
        ("${@:2} vs $@", &["a", "b", "c"], "b c vs a b c"),
        (
            "First: ${@:1:1}, All: $@",
            &["x", "y", "z"],
            "First: x, All: x y z",
        ),
        ("${@:1}", &["${@:2}", "test"], "${@:2} test"),
        ("${@:2}", &["a", "${@:3}", "c"], "${@:3} c"),
        ("$1: ${@:2}", &["cmd", "arg1", "arg2"], "cmd: arg1 arg2"),
        ("$1 $2 ${@:3}", &["a", "b", "c", "d"], "a b c d"),
        ("${@:0}", &["a", "b", "c"], "a b c"),
        ("${@:2}", &[], ""),
        ("${@:1}", &[], ""),
        ("${@:1}", &["only"], "only"),
        ("${@:2}", &["only"], ""),
        (
            "Process ${@:2} with $1",
            &["tool", "file1", "file2"],
            "Process file1 file2 with tool",
        ),
        ("${@:1:1} and ${@:2}", &["a", "b", "c"], "a and b c"),
        (
            "${@:1:2} vs ${@:3:2}",
            &["a", "b", "c", "d", "e"],
            "a b vs c d",
        ),
        (
            "${@:2}",
            &["cmd", "first arg", "second arg"],
            "first arg second arg",
        ),
        (
            "${@:2}",
            &["cmd", "$100", "@user", "#tag"],
            "$100 @user #tag",
        ),
        ("${@:1}", &["日本語", "🎉", "café"], "日本語 🎉 café"),
        ("prefix${@:2}suffix", &["a", "b", "c"], "prefixb csuffix"),
    ];
    for (template, args, expected) in cases {
        assert_eq!(
            substitute_args(template, &s(args)),
            *expected,
            "substituteArgs({template:?}, {args:?})"
        );
    }
}

#[test]
fn substitute_args_variable_cases() {
    let args = s(&["foo", "bar", "baz"]);
    assert_eq!(
        substitute_args("Test: $@", &args),
        substitute_args("Test: $ARGUMENTS", &args)
    );
    let args = s(&["x", "y", "z"]);
    let r1 = substitute_args("$@ and $ARGUMENTS", &args);
    assert_eq!(r1, substitute_args("$ARGUMENTS and $@", &args));
    assert_eq!(r1, "x y z and x y z");
    let many: Vec<String> = (0..100).map(|i| format!("arg{i}")).collect();
    assert_eq!(substitute_args("$ARGUMENTS", &many), many.join(" "));
    let fifteen: Vec<String> = (0..15).map(|i| format!("val{i}")).collect();
    assert_eq!(substitute_args("$10 $12 $15", &fifteen), "val9 val11 val14");
    let template = "Run $1 on ${@:2:2}, then process $@";
    let args = s(&["eslint", "file1.ts", "file2.ts", "file3.ts"]);
    assert_eq!(
        substitute_args(template, &args),
        "Run eslint on file1.ts file2.ts, then process eslint file1.ts file2.ts file3.ts"
    );
    let ten: Vec<String> = (1..=10).map(|i| format!("arg{i}")).collect();
    assert_eq!(
        substitute_args("${@:5:100}", &ten),
        "arg5 arg6 arg7 arg8 arg9 arg10"
    );
}

#[test]
fn substitute_args_expands_js_replacement_patterns_like_string_replace() {
    // `result.replace(/\$ARGUMENTS/g, allArgs)`: `$$` and `$&` in an argument
    // are replacement patterns in JavaScript.
    assert_eq!(substitute_args("$ARGUMENTS", &s(&["a$$b"])), "a$b");
    assert_eq!(substitute_args("x $@", &s(&["[$&]"])), "x [$@]");
    assert_eq!(substitute_args("${@:0:1} $1x", &s(&["q"])), "q qx");
}

#[test]
fn parse_command_args_cases() {
    let cases: &[(&str, &[&str])] = &[
        ("a b c", &["a", "b", "c"]),
        ("\"first arg\" second", &["first arg", "second"]),
        ("'first arg' second", &["first arg", "second"]),
        (
            "\"double\" 'single' \"double again\"",
            &["double", "single", "double again"],
        ),
        ("", &[]),
        ("a  b   c", &["a", "b", "c"]),
        ("a\tb\tc", &["a", "b", "c"]),
        ("\"\" \" \"", &[" "]),
        ("$100 @user #tag", &["$100", "@user", "#tag"]),
        ("日本語 🎉 café", &["日本語", "🎉", "café"]),
        ("\"line1\nline2\" second", &["line1\nline2", "second"]),
        ("\"quoted \\\"text\\\"\"", &["quoted \\text\\"]),
        ("a b c   ", &["a", "b", "c"]),
        ("   a b c", &["a", "b", "c"]),
    ];
    for (input, expected) in cases {
        assert_eq!(
            parse_command_args(input),
            s(expected),
            "parseCommandArgs({input:?})"
        );
    }
}

#[test]
fn parse_and_substitute_integration() {
    let args = parse_command_args("Button \"onClick handler\" \"disabled support\"");
    assert_eq!(
        substitute_args("Create component $1 with features: $ARGUMENTS", &args),
        "Create component Button with features: Button onClick handler disabled support"
    );
    assert_eq!(
        substitute_args(
            "Create a React component named $1 with features: $ARGUMENTS",
            &args
        ),
        "Create a React component named Button with features: Button onClick handler disabled support"
    );
    let args = parse_command_args("feature1 feature2 feature3");
    assert_eq!(
        substitute_args("Implement: $@", &args),
        substitute_args("Implement: $ARGUMENTS", &args)
    );
}

// loadPromptTemplates

struct Dir(tempfile::TempDir);

impl Dir {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn path(&self) -> String {
        self.0.path().to_string_lossy().into_owned()
    }
    fn write(&self, file: &str, content: &str) {
        std::fs::write(self.0.path().join(file), content).unwrap();
    }
}

fn load(prompt_paths: Vec<String>, slash_command_paths: Vec<String>) -> Vec<PromptTemplate> {
    load_prompt_templates(&LoadPromptTemplatesOptions {
        cwd: std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        agent_dir: cortexcode_code_paths::agent_dir()
            .to_string_lossy()
            .into_owned(),
        prompt_paths,
        slash_command_paths,
        include_defaults: false,
    })
}

fn find<'a>(templates: &'a [PromptTemplate], name: &str) -> &'a PromptTemplate {
    templates
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("template {name}"))
}

#[test]
fn argument_hint_from_frontmatter() {
    let dir = Dir::new();
    dir.write("pr.md", "---\ndescription: Review PRs from URLs with structured issue and code analysis\nargument-hint: \"<PR-URL>\"\n---\nYou are given one or more GitHub PR URLs: $@");
    dir.write("wr.md", "---\ndescription: Finish the current task end-to-end with changelog, commit, and push\nargument-hint: \"[instructions]\"\n---\nWrap it. Additional instructions: $ARGUMENTS");
    dir.write("cl.md", "---\ndescription: Audit changelog entries before release\n---\nAudit changelog entries for all commits since the last release.");
    dir.write(
        "empty-hint.md",
        "---\ndescription: A command with empty hint\nargument-hint: \"\"\n---\nDo something",
    );
    dir.write("is.md", "---\ndescription: Analyze GitHub issues (bugs or feature requests)\nargument-hint: \"<issue>\"\n---\nAnalyze GitHub issue(s): $ARGUMENTS");
    let templates = load(vec![dir.path()], vec![]);

    let pr = find(&templates, "pr");
    assert_eq!(pr.argument_hint.as_deref(), Some("<PR-URL>"));
    assert_eq!(
        pr.description,
        "Review PRs from URLs with structured issue and code analysis"
    );
    let wr = find(&templates, "wr");
    assert_eq!(wr.argument_hint.as_deref(), Some("[instructions]"));
    assert_eq!(
        wr.description,
        "Finish the current task end-to-end with changelog, commit, and push"
    );
    assert_eq!(find(&templates, "cl").argument_hint, None);
    assert_eq!(find(&templates, "empty-hint").argument_hint, None);
    assert_eq!(
        find(&templates, "is").argument_hint.as_deref(),
        Some("<issue>")
    );
}

#[test]
fn type_from_frontmatter() {
    let dir = Dir::new();
    dir.write("default.md", "---\ndescription: Default type\n---\nContent");
    dir.write(
        "system.md",
        "---\ndescription: System type\ntype: system\n---\nThink step by step",
    );
    dir.write(
        "context.md",
        "---\ndescription: Context type\ntype: context\n---\nContext info",
    );
    dir.write(
        "invalid.md",
        "---\ndescription: Invalid type\ntype: banana\n---\nContent",
    );
    let templates = load(vec![dir.path()], vec![]);
    assert_eq!(find(&templates, "default").kind, PromptTemplateType::User);
    assert_eq!(find(&templates, "system").kind, PromptTemplateType::System);
    assert_eq!(
        find(&templates, "context").kind,
        PromptTemplateType::Context
    );
    assert_eq!(find(&templates, "invalid").kind, PromptTemplateType::User);
}

#[test]
fn slash_command_paths() {
    let prompts = Dir::new();
    let slash = Dir::new();
    slash.write(
        "deploy.md",
        "---\ndescription: Deploy the app\n---\nRun deploy script",
    );
    let templates = load(vec![], vec![slash.path()]);
    assert_eq!(find(&templates, "deploy").description, "Deploy the app");

    prompts.write("review.md", "Review code");
    let templates = load(vec![prompts.path()], vec![slash.path()]);
    assert!(templates.iter().any(|t| t.name == "review"));
    assert!(templates.iter().any(|t| t.name == "deploy"));
    // The first non-empty body line is the description when none is given.
    assert_eq!(find(&templates, "review").description, "Review code");

    slash.write(
        "greet.prompt.md",
        "---\ndescription: Say hi\n---\nSay hello",
    );
    let templates = load(vec![], vec![slash.path()]);
    assert!(templates.iter().any(|t| t.name == "greet"));
    assert!(!templates.iter().any(|t| t.name == "greet.prompt"));
}

// tryExpandPromptTemplate

fn virtual_template(name: &str, kind: PromptTemplateType, content: &str) -> PromptTemplate {
    let path = format!("/virtual/{name}.md");
    PromptTemplate {
        name: name.into(),
        description: name.into(),
        argument_hint: None,
        kind,
        content: content.into(),
        source_info: SourceInfo {
            path: path.clone(),
            source: "local".into(),
            scope: SourceScope::Project,
            origin: SourceOrigin::TopLevel,
            base_dir: None,
        },
        file_path: path,
    }
}

fn templates() -> Vec<PromptTemplate> {
    vec![
        virtual_template("review", PromptTemplateType::User, "Review: $ARGUMENTS"),
        virtual_template(
            "system",
            PromptTemplateType::System,
            "Think step by step: $ARGUMENTS",
        ),
    ]
}

#[test]
fn should_return_template_metadata_when_matched() {
    let e = try_expand_prompt_template("/review some code", &templates());
    assert_eq!(e.text, "Review: some code");
    let template = e.template.unwrap();
    assert_eq!(template.name, "review");
    assert_eq!(template.kind, PromptTemplateType::User);
    assert_eq!(e.args, ["some", "code"]);
    assert_eq!(e.args_string, "some code");
}

#[test]
fn should_return_system_template_type() {
    let e = try_expand_prompt_template("/system hello", &templates());
    assert_eq!(e.text, "Think step by step: hello");
    assert_eq!(e.template.unwrap().kind, PromptTemplateType::System);
}

#[test]
fn should_return_undefined_template_when_not_matched() {
    let e = try_expand_prompt_template("/unknown", &templates());
    assert_eq!(e.text, "/unknown");
    assert!(e.template.is_none());
    assert!(e.args.is_empty());
}

#[test]
fn should_return_empty_args_for_non_slash_input() {
    let e = try_expand_prompt_template("hello world", &templates());
    assert_eq!(e.text, "hello world");
    assert!(e.template.is_none());
    assert_eq!(e.args_string, "");
}
