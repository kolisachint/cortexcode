// marked token streams (strict-strikethrough tokenizer) for deterministic
// pseudo-random documents stitched from markdown fragments.
// Output: crates/cortexcode-tui-components/tests/fixtures/markdown-fuzz-gold.json
import { Marked, Tokenizer } from "marked";

const STRICT = /^(~~)(?=[^\s~])((?:\\.|[^\\])*?(?:\\.|[^\s~\\]))\1(?=[^~]|$)/;
class Strict extends Tokenizer {
  del(src) {
    const m = STRICT.exec(src);
    if (!m) return undefined;
    return { type: "del", raw: m[0], text: m[2], tokens: this.lexer.inlineTokens(m[2]) };
  }
}
const parser = new Marked();
parser.setOptions({ tokenizer: new Strict() });
function canon(v) {
  if (Array.isArray(v)) return v.map(canon);
  if (v && typeof v === "object") {
    const o = {};
    for (const k of Object.keys(v).sort()) if (v[k] !== undefined) o[k] = canon(v[k]);
    return o;
  }
  return v;
}
let seed = 0x2545f491;
const rand = (n) => {
  seed ^= seed << 13; seed >>>= 0; seed ^= seed >>> 17; seed ^= seed << 5; seed >>>= 0;
  return seed % n;
};
const frags = [
  "word", " ", "  ", "\n", "\n\n", "*", "**", "_", "__", "~~", "~", "`", "``", "```", "\\", "[", "]", "(", ")", "![",
  "<", ">", "#", "## ", "- ", "* ", "1. ", "2) ", "> ", "|", "---", "===", "\n    ", "\n  ", "http://a.b/c", "www.x.io",
  "a@b.co", "<b>", "</b>", "<div>", "&amp;", "[x]", "[x]: /u", "\"t\"", "'q'", "é", "😀", "日本", ".", ",", "!", "?", ":",
  "\t", "- [ ] ", "- [x] ", "| a | b |\n|---|---|\n", "```js\n", "~~~\n", "<!-- c -->", "\\*", "\\_", "x_y", "a*b",
];
const out = [];
for (let i = 0; i < 1500; i++) {
  let src = "";
  const n = 1 + rand(24);
  for (let j = 0; j < n; j++) src += frags[rand(frags.length)];
  let tokens;
  try { tokens = parser.lexer(src.replace(/\t/g, "   ")); } catch { continue; }
  const links = Object.entries(tokens.links).map(([k, v]) => [k, canon(v)]);
  out.push({ src, tokens: JSON.stringify(canon([...tokens])), links: JSON.stringify(links) });
}
console.log(JSON.stringify(out));
