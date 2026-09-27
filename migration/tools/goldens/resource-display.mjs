// showLoadedResources over stub deps, rendered at a fixed width.
// Output: crates/cortexcode-code-tui-app/tests/fixtures/resource-display-gold.json
import { Container } from "@kolisachint/hoocode-tui";
import { showLoadedResources } from "./modes/interactive/resource-display.js";
import { initTheme } from "./modes/interactive/theme/theme.js";
import { setMcpServerStatus, clearMcpServerStatuses } from "./core/mcp-status.js";
initTheme("dark");
const HOME = process.env.HOME;
const si = (source, scope, baseDir) => ({ path: "", source, scope, origin: "top-level", baseDir });
const cases = {
  startup: { skills: [["a-skill", "/w/.agents/skills/a/SKILL.md", si("local", "project")], ["b-skill", `${HOME}/.agents/skills/b/SKILL.md`, si("local", "user")]], agents: [["explore", "Explores."], ["plan", "Plans."], ["general-purpose", "General."], ["reviewer", "Reviews code\nin detail."], ["debugger", "Debugs."]] },
  empty: {},
  rich_expanded: {
    expanded: true,
    skills: [["zeta", "/w/.agents/skills/zeta/SKILL.md", si("local", "project")], ["alpha", `${HOME}/.hoo/skills/alpha/SKILL.md`, si("local", "user")], ["pkg-skill", "/n/node_modules/@acme/tools/skills/x/SKILL.md", si("npm:@acme/tools", "user", "/n/node_modules/@acme/tools")]],
    templates: [["review", "/w/.hoo/prompts/review.md", si("local", "project")], ["fix", "/w/.hoo/prompts/fix.md", si("local", "project")]],
    context: [["/w/AGENTS.md", 1200, undefined], ["/w/sub/CLAUDE.md", 30000, "large"]],
    warnings: ["Could not read /w/BROKEN.md"],
    agents: [["explore", "Explores the codebase quickly and reports back with a long description that goes on and on past the width of the terminal"]],
    mcp: [{ name: "github", state: "ready", toolCount: 12, background: false, deferred: true }, { name: "auth", state: "authorizing", toolCount: 0, background: true }],
    skillDiagnostics: [{ type: "collision", message: "c", collision: { resourceType: "skill", name: "dup", winnerPath: "/w/.agents/skills/zeta/SKILL.md", loserPath: `${HOME}/.hoo/skills/dup/SKILL.md` } }, { type: "warning", message: "bad frontmatter", path: "/w/.agents/skills/zeta/SKILL.md" }, { type: "error", message: "no path error" }],
    columns: 60,
  },
  quiet_diag: { quiet: true, promptDiagnostics: [{ type: "warning", message: "dup name", path: "/w/.hoo/prompts/fix.md" }], templates: [["fix", "/w/.hoo/prompts/fix.md", si("local", "project")]] },
};
const out = {};
for (const [name, c] of Object.entries(cases)) {
  clearMcpServerStatuses();
  for (const m of c.mcp ?? []) setMcpServerStatus(m);
  const chat = new Container();
  const skills = (c.skills ?? []).map(([n, p, s]) => ({ name: n, filePath: p, sourceInfo: s }));
  const prompts = (c.templates ?? []).map(([n, p, s]) => ({ name: n, filePath: p, sourceInfo: s }));
  const loader = {
    getSkills: () => ({ skills, diagnostics: c.skillDiagnostics ?? [] }),
    getPrompts: () => ({ prompts, diagnostics: c.promptDiagnostics ?? [] }),
    getThemes: () => ({ themes: [], diagnostics: [] }),
    getExtensions: () => ({ extensions: [], errors: [] }),
    getAgentsFiles: () => ({ agentsFiles: (c.context ?? []).map(([p, tokens, size]) => ({ path: p, content: "", tokens, size })), warnings: c.warnings ?? [] }),
  };
  const agents = (c.agents ?? []).map(([n, d]) => ({ name: n, description: d }));
  showLoadedResources({
    chatContainer: chat, getCwd: () => "/w", getResourceLoader: () => loader, getPromptTemplates: () => prompts,
    getExtensionRunner: () => ({ getCommandDiagnostics: () => [], getShortcutDiagnostics: () => [] }),
    getActiveMode: () => "build", getSubagentEnabled: () => true, getAgentCount: () => agents.length, getAgents: () => agents,
    getCanvases: () => [], getColumns: () => c.columns, quietStartup: () => c.quiet ?? false, verbose: false,
    isExpanded: () => c.expanded ?? false, getBuiltInCommandConflictDiagnostics: () => [],
  }, { force: false, showDiagnosticsWhenQuiet: true });
  out[name] = { case: c, lines: chat.render(100) };
}
console.log(JSON.stringify({ home: HOME, cases: out }, null, 1));
