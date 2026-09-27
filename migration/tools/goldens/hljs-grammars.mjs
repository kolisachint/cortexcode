// Dump highlight.js 10.7.3's registered languages (as cli-highlight loads
// them) as a JSON object graph: object identity, frozen flags, regexes and the
// few callbacks, by name. Output: crates/cortexcode-tui-highlight/data/hljs-grammars.json
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
const require = createRequire(import.meta.url);
const hljs = require("highlight.js");

const probe = hljs.END_SAME_AS_BEGIN({});
const known = new Map([
  [probe["on:begin"].toString(), "endSameAsBegin.begin"],
  [probe["on:end"].toString(), "endSameAsBegin.end"],
]);
function fnName(fn) {
  const src = fn.toString();
  if (known.has(src)) return known.get(src);
  if (src.includes("m.index !== 0")) return "shebang.begin";
  if (src.includes("beforeMatch cannot be used with starts")) return "ext.beforeMatch";
  if (src.includes("hasClosingTag")) return "jsx.isTrulyOpeningTag";
  if (src.includes("SYSTEM_SYMBOLS_SET")) return "mathematica.systemSymbol";
  throw new Error("unknown callback: " + src);
}

const ids = new Map();
const nodes = [];
function ref(value) {
  if (value === null || value === undefined) return null;
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") return value;
  if (value instanceof RegExp) return { $re: value.source, flags: value.flags };
  if (typeof value === "function") return { $fn: fnName(value) };
  if (Array.isArray(value)) return { $arr: value.map(ref) };
  if (ids.has(value)) return { $ref: ids.get(value) };
  const id = nodes.length;
  ids.set(value, id);
  const node = { frozen: Object.isFrozen(value), props: {} };
  nodes.push(node);
  for (const key of Object.keys(value)) {
    if (key === "rawDefinition") continue;
    node.props[key] = ref(value[key]);
  }
  return { $ref: id };
}

const languages = {};
for (const name of hljs.listLanguages()) {
  languages[name] = ref(hljs.getLanguage(name));
}
const aliases = {};
for (const name of hljs.listLanguages()) {
  for (const alias of hljs.getLanguage(name).aliases || []) aliases[alias] = name;
}

// mathematica's system-symbol set lives in a closure.
const src = readFileSync(require.resolve("highlight.js/lib/languages/mathematica.js"), "utf8");
const start = src.indexOf("const SYSTEM_SYMBOLS = [");
const end = src.indexOf("];", start);
const systemSymbols = JSON.parse(src.slice(start + "const SYSTEM_SYMBOLS = ".length, end + 1).replace(/'/g, '"').replace(/,\s*\]/, "]"));

console.log(JSON.stringify({ version: hljs.versionString, languages, aliases, nodes, systemSymbols }));
