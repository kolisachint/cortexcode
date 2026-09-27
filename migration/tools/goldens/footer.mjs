// Footer rendering for a range of states, with a stub session (as
// footer-width.test.ts builds one). Output: crates/cortexcode-code-tui-app/tests/fixtures/footer-gold.json
import { FooterComponent } from "./modes/interactive/components/footer.js";
import { initTheme } from "./modes/interactive/theme/theme.js";
import { startupProgress } from "./core/startup-progress.js";
initTheme("dark");
function session(o) {
  const entries = o.usage ? [{ type: "message", message: { role: "assistant", usage: o.usage } }] : [];
  return {
    state: { model: o.noModel ? undefined : { id: o.modelId ?? "mock-model", provider: o.provider ?? "mock", contextWindow: o.cw ?? 128000, reasoning: o.reasoning ?? false }, thinkingLevel: o.thinking ?? "off" },
    sessionManager: { getEntries: () => entries, getDisplayName: () => o.name ?? "amber-harbor", getCwd: () => o.cwd ?? "/tmp/project" },
    settingsManager: { getCompactionSettings: () => ({ enabled: true, reserveTokens: o.reserve ?? 16384, keepRecentTokens: 20000 }) },
    getContextUsage: () => o.ctx === undefined ? { contextWindow: o.cw ?? 128000, percent: 0 } : o.ctx,
    modelRegistry: { isUsingOAuth: () => o.oauth ?? false },
  };
}
function data(o) {
  return { getGitBranch: () => o.branch ?? null, getExtensionStatuses: () => new Map(o.statuses ?? []), getAvailableProviderCount: () => o.providers ?? 1, onBranchChange: () => () => {}, getActiveMode: () => o.mode ?? "build", getSubagentEnabled: () => false };
}
const cases = {
  idle: [{}, {}, {}, 100],
  branch_name: [{ name: "refactor-auth" }, { branch: "main" }, { chip: false }, 100],
  chip_wide: [{ name: "refactor-auth" }, { branch: "main" }, { chip: true }, 120],
  chip_narrow: [{ name: "refactor-auth" }, {}, { chip: true }, 40],
  usage: [{ reasoning: true, thinking: "high", usage: { input: 12345, output: 6789, cacheRead: 1000, cacheWrite: 50, cost: { total: 1.2345 } }, ctx: { contextWindow: 128000, percent: 85.25 } }, { providers: 2 }, {}, 120],
  reasoning_off: [{ reasoning: true }, { providers: 2 }, {}, 60],
  unknown_pct: [{ ctx: { contextWindow: 200000, percent: null }, cw: 200000, oauth: true }, { mode: "plan" }, { view: "full" }, 100],
  no_autocompact: [{ ctx: { contextWindow: 128000, percent: 72 } }, {}, { auto: false }, 100],
  no_model: [{ noModel: true, ctx: null }, {}, {}, 80],
  line: [{}, {}, { density: "line" }, 100],
  statuses: [{}, { statuses: [["b", "beta\tstatus\n  x"], ["a", "alpha"]] }, {}, 100],
  truncated: [{ cwd: "/very/long/" + "path/".repeat(30) }, { branch: "feature/x" }, {}, 50],
  startup: [{}, {}, { startup: true }, 100],
};
const out = {};
for (const [name, [so, dO, opts, width]] of Object.entries(cases)) {
  startupProgress.clear();
  if (opts.startup) {
    startupProgress.set({ key: "fd", kind: "download", label: "fd", receivedBytes: 3670016, totalBytes: 8388608 });
    startupProgress.set({ key: "rg", kind: "download", label: "ripgrep", receivedBytes: 1048576, totalBytes: null });
    startupProgress.set({ key: "idx", kind: "work", label: "Building semantic search index", done: 120, total: 480, unit: "files" });
    startupProgress.set({ key: "e", kind: "error", label: "Semantic search index unavailable", message: "embsearch binary not found (PATH or embsearchBinaryPath setting)" });
  }
  const f = new FooterComponent(session(so), data(dO));
  if (opts.chip !== undefined) f.setSessionChipShown(opts.chip);
  if (opts.view) f.setToolOutputView(opts.view);
  if (opts.auto === false) f.setAutoCompactEnabled(false);
  if (opts.density) f.setDensity(opts.density);
  out[name] = { so, dO, opts, width, lines: f.render(width) };
}
console.log(JSON.stringify(out, null, 1));
