// The built-in and registered tools' renderCall/renderResult output, rendered
// at a fixed width, for a set of calls and results.
// Output: crates/cortexcode-code-tui-widgets/tests/fixtures/tool-renderers-gold.json
import { setKeybindings } from "@kolisachint/hoocode-tui";
import { KeybindingsManager } from "./core/keybindings.js";
import { initTheme, theme } from "./modes/interactive/theme/theme.js";
import { createReadToolDefinition } from "./core/tools/read.js";
import { createBashToolDefinition } from "./core/tools/bash.js";
import { createEditToolDefinition } from "./core/tools/edit.js";
import { createWriteToolDefinition } from "./core/tools/write.js";
import { createSearchToolDefinition } from "./core/tools/search.js";
import { createWebFetchToolDefinition } from "./core/tools/webfetch.js";
import { createWebSearchToolDefinition } from "./core/tools/websearch.js";
import { createTaskToolDefinition, createTaskOutputToolDefinition } from "./core/tools/subagent.js";
import {
  createSearchPluginsToolDefinition,
  createListPluginsToolDefinition,
  createInstallPluginToolDefinition,
} from "./core/tools/plugins.js";

initTheme("dark");
setKeybindings(new KeybindingsManager());
const cwd = "/work/project";
const defs = {
  read: createReadToolDefinition(cwd),
  bash: createBashToolDefinition(cwd),
  edit: createEditToolDefinition(cwd),
  write: createWriteToolDefinition(cwd),
  SearchCodebase: createSearchToolDefinition(cwd),
  webfetch: createWebFetchToolDefinition(cwd),
  websearch: createWebSearchToolDefinition(cwd),
  Task: createTaskToolDefinition(cwd),
  TaskOutput: createTaskOutputToolDefinition(),
  SearchPlugins: createSearchPluginsToolDefinition(),
  ListPlugins: createListPluginsToolDefinition(),
  InstallPlugin: createInstallPluginToolDefinition(),
};
const txt = (t) => ({ content: [{ type: "text", text: t }], details: {} });
const many = (n) => Array.from({ length: n }, (_, i) => `row ${i + 1}`).join("\n");
const cases = [
  ["bash", { command: "ls -la" }, txt("a\nb\nc")],
  ["bash", { command: "seq 30", timeout: 10 }, txt(many(30))],
  ["bash", { command: 5 }, undefined],
  ["bash", { command: "" }, txt("")],
  ["bash", { command: "big" }, { content: [{ type: "text", text: "tail" }], details: { truncation: { truncated: true, truncatedBy: "lines", outputLines: 800, totalLines: 5000 }, fullOutputPath: "/tmp/out.log" } }],
  ["bash", { command: "big2" }, { content: [{ type: "text", text: "tail" }], details: { truncation: { truncated: true, truncatedBy: "bytes", outputLines: 12 } } }],
  ["bash", { command: "boom" }, txt("boom\n\nCommand exited with code 3"), { isError: true }],
  ["edit", { path: "missing.txt", edits: [{ oldText: "a", newText: "b" }] }, { content: [{ type: "text", text: "Could not edit file: missing.txt. Error code: ENOENT." }], details: {} }, { isError: true }],
  ["edit", { path: "x.ts", oldText: "before", newText: "after" }, { content: [], details: { diff: "-1 before\n+1 after", firstChangedLine: 1 } }],
  ["read", { path: "notes.txt" }, txt("alpha\nbeta\n")],
  ["read", { path: "/work/project/src/a.txt", offset: 10, limit: 3 }, txt("x\ny\nz")],
  ["read", { file_path: "big.txt" }, txt(many(12))],
  ["read", { path: 42 }, undefined],
  ["read", { path: "" }, undefined],
  ["read", { path: "trunc.txt" }, { content: [{ type: "text", text: "a\nb" }], details: { truncation: { truncated: true, truncatedBy: "lines", outputLines: 2, totalLines: 900, maxLines: 800 } } }],
  ["read", { path: "trunc2.txt" }, { content: [{ type: "text", text: "a" }], details: { truncation: { truncated: true, truncatedBy: "bytes", outputLines: 1, maxBytes: 32768 } } }],
  ["read", { path: "trunc3.txt" }, { content: [{ type: "text", text: "a" }], details: { truncation: { truncated: true, firstLineExceedsLimit: true } } }],
  ["read", { path: "/work/project/.agents/skills/deploy/SKILL.md" }, txt("hidden")],
  ["read", { path: "AGENTS.md", offset: 5 }, txt("hidden")],
  ["write", { path: "out.txt", content: "one\ntwo\n\n" }, undefined],
  ["write", { path: "long.txt", content: many(9) }, undefined],
  ["write", { path: "x.txt", content: 5 }, undefined],
  ["write", { path: "x.txt", content: "" }, { content: [{ type: "text", text: "EACCES: denied" }], details: {} }, { isError: true }],
  ["write", { path: "x.txt", content: "ok" }, txt("Successfully wrote 2 bytes to x.txt")],
  ["SearchCodebase", { query: "session store", mode: "lexical", limit: 3 }, txt(many(7))],
  ["SearchCodebase", { query: 7 }, txt("")],
  ["webfetch", { url: "https://example.com/doc", output: "markdown" }, { content: [{ type: "text", text: many(8) }], details: { tokenEstimate: 1200, truncated: true, maxTokens: 1000, sectionCount: 4 } }],
  ["webfetch", { url: "" }, { content: [{ type: "text", text: "body" }], details: { tokenEstimate: 90, matchCount: 1 } }],
  ["webfetch", { url: "https://x.y" }, { content: [{ type: "text", text: "body" }], details: { tokenEstimate: 90, matchCount: 3 } }],
  ["websearch", { query: "rust tui", maxResults: 5 }, { content: [{ type: "text", text: many(3) }], details: { tokenEstimate: 420 } }],
  ["websearch", { query: "" }, txt("")],
  ["Task", { subagent_type: "explore", prompt: "look" }, undefined],
  ["Task", { prompt: "look" }, undefined],
  ["TaskOutput", { task_id: "explore#1", wait: true }, { content: [{ type: "text", text: "found it\nsecond line" }], details: { task_id: "explore#1", status: "done", ok: true } }],
  ["TaskOutput", { task_id: "abc123" }, { content: [{ type: "text", text: "boom" }], details: { task_id: "abc123", status: "failed", ok: false } }],
  ["TaskOutput", { list: true }, { content: [{ type: "text", text: "2 tasks:\n- explore#1  running (12s)\n- general#2  done (uncollected) 3s\n- x  cancelled" }], details: { status: "list" } }],
  ["SearchPlugins", { query: "mail" }, txt("3 plugins:\n  a\n  b\n  c")],
  ["ListPlugins", {}, txt(many(9))],
  ["InstallPlugin", { name: "x", reason: "y" }, txt("")],
];
const out = [];
for (const [tool, args, result, extra] of cases) {
  for (const expanded of [false, true]) {
    const isError = extra?.isError ?? false;
    const state = {};
    const ctx = (last) => ({ args, toolCallId: "t", invalidate() {}, lastComponent: last, state, cwd, executionStarted: true, argsComplete: true, isPartial: false, expanded, showImages: false, isError });
    const def = defs[tool];
    let call = null, res = null;
    // Settled state: renderers that compute in the background (edit's
    // preview) invalidate when done, which re-runs both slots.
    if (def.renderCall) {
      const first = def.renderCall(args, theme, ctx(undefined));
      await new Promise((r) => setTimeout(r, 50));
      call = def.renderCall(args, theme, ctx(first));
    }
    if (result && def.renderResult) res = def.renderResult(result, { expanded, isPartial: false }, theme, ctx(undefined)).render(120);
    if (call) call = call.render(120);
    out.push({ tool, args, result: result ?? null, isError, expanded, call, res });
  }
}
console.log(JSON.stringify(out, null, 1));
