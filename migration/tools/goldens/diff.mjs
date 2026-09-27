// jsdiff diffWords change objects for pseudo-random line pairs, and the
// pin's renderDiff output for diff texts built from them.
// Output: crates/cortexcode-code-tui-widgets/tests/fixtures/diff-gold.json
import { diffWords } from "diff";
import { renderDiff } from "./modes/interactive/components/diff.js";
import { initTheme } from "./modes/interactive/theme/theme.js";

initTheme("dark");
let seed = 0x1234abcd;
const rand = (n) => {
  seed ^= seed << 13; seed >>>= 0; seed ^= seed >>> 17; seed ^= seed << 5; seed >>>= 0;
  return seed % n;
};
const pool = ["foo", "bar", "baz", "qux", " ", "  ", "\t", ".", ",", "(", ")", "=", "==", "x1", "é", "日本", "😀", "_a", "-", "\n", "   ", "return", "let", ";", "{", "}", " ", "ˇ"];
const phrase = () => {
  let s = "";
  const n = rand(10);
  for (let i = 0; i < n; i++) s += pool[rand(pool.length)];
  return s;
};
const mutate = (s) => {
  const words = s.split(/(\s+)/);
  const out = [];
  for (const w of words) {
    const r = rand(6);
    if (r === 0) continue;
    if (r === 1) out.push(pool[rand(pool.length)]);
    out.push(w);
    if (r === 2) out.push(pool[rand(pool.length)]);
  }
  return out.join("");
};
const pairs = [
  ["foo bar baz", "foo baz"], ["foo bar baz", "foo qux baz"], ["foo\nbar baz", "foo baz"], ["foo baz", "foo\nbar baz"],
  ["foo   bar baz", "foo  baz"], ["", "abc"], ["abc", ""], ["  indented code();", "  indented code2();"],
  ["const x = foo(a, b);", "const y = foo(a, c);"], ["\tif (a) {", "\tif (b) {"],
];
for (let i = 0; i < 1200; i++) {
  const a = phrase();
  pairs.push([a, rand(4) === 0 ? phrase() : mutate(a)]);
}
const words = pairs.map(([a, b]) => ({ a, b, changes: diffWords(a, b).map((c) => ({ value: c.value, added: !!c.added, removed: !!c.removed })) }));
const diffs = [
  "-1 const x = 1;\n+1 const x = 2;\n 2 keep",
  "-10 old line\n-11 second\n+10 new line\n     ...\n 12 ctx",
  "+5 added only\n 6 context\n-7 removed only",
  "not a diff line\n-3 \tbody\ttabs\n+3 \tbody tabs",
  "-12 foo bar baz\n+12 foo qux baz",
  "-1 a\n+1 b\n+2 c",
];
for (let i = 0; i < 60; i++) {
  const a = phrase().replace(/\n/g, " "), b = mutate(a).replace(/\n/g, " ");
  diffs.push(`-${i + 1} ${a}\n+${i + 1} ${b}\n ${i + 2} ${phrase().replace(/\n/g, " ")}`);
}
const rendered = diffs.map((d) => ({ diff: d, out: renderDiff(d) }));
console.log(JSON.stringify({ words, rendered }));
