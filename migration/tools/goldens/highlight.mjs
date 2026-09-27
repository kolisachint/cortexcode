// cli-highlight's output (highlight.js 10.7.3) over a corpus: source files of
// the pin and of cortexcode, hand-written snippets, and a polyglot snippet in
// every language. Cases run in order in one process (grammars compile once and
// share modes), with cli-highlight's DEFAULT_THEME and with a marker theme for
// the classes hoocode's theme maps.
// Output: crates/cortexcode-tui-highlight/tests/fixtures/highlight-gold.json
import { createRequire } from "node:module";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
const require = createRequire(import.meta.url);
const { highlight, listLanguages } = require("cli-highlight");

const here = new URL(".", import.meta.url).pathname;
const pin = join(here, "../../.."); // dist -> packages/coding-agent -> pin root
const repo = join(pin, "../..");

const MARKED = ["keyword", "built_in", "literal", "number", "string", "comment", "function", "title", "class", "type", "attr", "variable", "params", "operator", "punctuation"];
const marker = Object.fromEntries(MARKED.map((t) => [t, (s) => `<${t}>${s}</${t}>`]));

function walk(dir, out = []) {
  for (const name of readdirSync(dir).sort()) {
    if (name === "node_modules" || name.startsWith(".") || name === "target" || name === "dist") continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

const EXT = { ts: "typescript", md: "markdown", json: "json", html: "xml", mjs: "javascript", js: "javascript", yml: "yaml", sh: "bash", css: "css", ps1: "powershell", jsx: "javascript", py: "python", c: "c", toml: "ini", rs: "rust", py3: "python" };
const PER_EXT = { ts: 14, md: 10, json: 4, html: 3, mjs: 4, yml: 3, sh: 3, css: 2, ps1: 2, jsx: 2, py: 1, c: 1, toml: 2, js: 3, rs: 14 };

function clip(text) {
  return text.split("\n").slice(0, 120).join("\n");
}

const cases = [];
function add(lang, code, theme, ignoreIllegals = true) {
  cases.push({ lang, code, theme, ignoreIllegals });
}

// Files, spread evenly over each extension's sorted list.
const files = [...walk(pin), ...walk(join(repo, "crates"))];
for (const [ext, n] of Object.entries(PER_EXT)) {
  const of = files.filter((f) => f.endsWith("." + ext));
  for (let i = 0; i < n && of.length; i++) {
    const f = of[Math.floor((i * of.length) / n)];
    const code = clip(readFileSync(f, "utf8"));
    add(EXT[ext], code, "default");
    add(EXT[ext], code, "marker");
  }
}

const SNIPPETS = {
  javascript: "const re = /ab+c/gi; // comment\nclass Foo extends Bar {\n  #x = 1n;\n  async *gen() { yield `t ${this.#x + 0x1f}`; }\n}\nconst el = <div className=\"a\">{x}</div>;\nexport default function () { return null ?? undefined; }\n",
  typescript: "interface A<T> { readonly a?: T; }\ntype U = keyof A<string> | `x${number}`;\nenum E { A = 1 }\n@dec() class C implements A<number> { constructor(private x: number) {} }\nlet f = <T,>(x: T): x is T => true;\n",
  python: "@decorator\nasync def f(a: int = 1, *args, **kw) -> str:\n    \"\"\"Doc string.\"\"\"\n    return f\"{a!r:>10}\" + r'\\d' + b'x'  # comment\nclass A(B, metaclass=M):\n    x = [i for i in range(10) if i % 2]\n",
  rust: "#[derive(Debug)]\npub struct S<'a> { x: &'a str }\nimpl<'a> S<'a> {\n    pub fn new(x: &'a str) -> Self { Self { x } } // c\n}\nfn main() { let v = vec![1u8, 0xff]; println!(\"{:?} {}\", v, r#\"raw\"#); }\n",
  go: "package main\nimport \"fmt\"\nfunc main() {\n\tch := make(chan int, 3)\n\tgo func() { ch <- 1 }()\n\tfmt.Printf(\"%d\\n\", <-ch) // c\n}\n",
  java: "@Override\npublic final class A<T extends B> implements C {\n  private static final long X = 10L;\n  public String toString() { return \"a\" + 'c'; } /* block */\n}\n",
  c: "#include <stdio.h>\n#define MAX(a,b) ((a)>(b)?(a):(b))\nint main(int argc, char **argv) {\n  unsigned long x = 0x10UL; /* c */\n  printf(\"%lu\\n\", x);\n  return 0;\n}\n",
  cpp: "template <typename T>\nclass V : public std::vector<T> {\npublic:\n  constexpr auto size() const noexcept -> std::size_t override { return 0; }\n};\nauto s = R\"(raw)\"s; // c\n",
  csharp: "using System;\nnamespace N {\n  public record R(int X);\n  class A { public async Task<int> F() => await G($\"{x}\"); }\n}\n",
  bash: "#!/bin/bash\nset -euo pipefail\nfor f in \"$@\"; do\n  echo \"${f%.txt}\" $(( 1 + 2 )) > /dev/null 2>&1 # c\ndone\nif [[ -n $X ]]; then exit 1; fi\n",
  shell: "$ ls -la\ntotal 0\n$ echo hi\nhi\n",
  json: "{\n  \"a\": [1, 2.5e3, true, null],\n  \"b\": {\"c\": \"d\\n\"}\n}\n",
  yaml: "---\nkey: value\nlist:\n  - a\n  - 'b'\nanchor: &x\n  n: 1.5\nref: *x # c\nmulti: |\n  text\n",
  ini: "[section]\nkey = \"value\"\nn = 1\narr = [1, 2]\n# comment\n",
  markdown: "# Title\n\nSome *em* and **strong** and `code` and [link](http://x).\n\n```js\nconst a = 1;\n```\n\n- item\n> quote\n",
  xml: "<?xml version=\"1.0\"?>\n<!DOCTYPE html>\n<html lang=\"en\">\n<!-- c -->\n<script>let a = 1 < 2;</script>\n<style>a { color: red; }</style>\n<p class=\"x\">t &amp; u</p>\n</html>\n",
  css: "@media (max-width: 600px) {\n  .a > #b:hover::before { content: \"x\"; color: #fff !important; margin: 0 1.5em; }\n}\n",
  scss: "$c: red;\n@mixin m($a) { color: $a; }\n.a { &:hover { @include m($c); } }\n",
  sql: "SELECT a.id, COUNT(*) AS n FROM users a LEFT JOIN t ON t.id = a.id WHERE a.name LIKE 'x%' GROUP BY a.id; -- c\n",
  ruby: "class A < B\n  attr_reader :x\n  def f(a = 1, *b, &c) = \"#{a}\"\nend\n%w[a b].each { |x| puts x } # c\n",
  php: "<?php\nnamespace A;\nfunction f(int $a): ?string { return \"x{$a}\"; }\n$x = new class { public $y = 1; };\n?>\n<p>html</p>\n",
  diff: "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n context\n",
  dockerfile: "FROM node:18 AS build\nRUN npm ci && npm run build\nCOPY --from=build /app /app\nENV A=1\nCMD [\"node\", \"x.js\"]\n",
  makefile: "all: build\n\nbuild: $(SRC)\n\t$(CC) -o $@ $^ # c\n.PHONY: all\n",
  lua: "local function f(a, ...)\n  return #a, [[long\nstring]] -- c\nend\n",
  kotlin: "data class A(val x: Int) {\n  fun f(): String = \"$x ${x + 1}\"\n}\n",
  swift: "struct A: Codable {\n  let x: Int?\n  func f() -> String { \"\\(x ?? 0)\" }\n}\n",
  scala: "case class A(x: Int)\nobject M { def f[T](t: T): T = t }\n",
  haskell: "module Main where\nmain :: IO ()\nmain = putStrLn \"hi\" -- c\n",
  r: "f <- function(x, ...) { x %>% filter(y > 1) } # c\n",
  perl: "my $x = qr/a+/;\nprint \"$x\\n\" if $y =~ m{b}x;\n",
  powershell: "function Get-X { param([string]$A) Write-Host \"$A\" } # c\n",
  handlebars: "{{#if a}}<p>{{b}}</p>{{/if}}\n",
  latex: "\\documentclass{article}\n\\begin{document}$x^2$ % c\\end{document}\n",
  mathematica: "Plot[Sin[x], {x, 0, 2 Pi}] (* c *)\n",
  plaintext: "just text\n",
};
for (const [lang, code] of Object.entries(SNIPPETS)) {
  add(lang, code, "default");
  add(lang, code, "marker");
  add(lang, code, "default", false);
}

const POLYGLOT = "# comment\n// comment\n/* block */ -- dash\nfunction f(a, b) { return a + 1.5e3 - 0x1F; }\nif (x == \"str\" && y != 'c') then print(\"a\\n\") end\n<tag attr=\"v\">text</tag>\n@decorator class A extends B:\n  let x = [1, 2, 3]; $var @at %p\n";
for (const lang of listLanguages()) {
  add(lang, POLYGLOT, "default");
}

const out = cases.map((c) => {
  let res;
  try {
    res = highlight(c.code, { language: c.lang, ignoreIllegals: c.ignoreIllegals, theme: c.theme === "marker" ? marker : {} });
  } catch (e) {
    res = null;
  }
  return { ...c, out: res };
});
process.stdout.write(JSON.stringify(out, null, 1) + "\n");
