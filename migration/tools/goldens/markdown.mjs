// marked token streams (with markdown.ts's strict-strikethrough tokenizer)
// and Markdown component renders for a corpus of inputs.
// Output: crates/cortexcode-tui-components/tests/fixtures/markdown-gold.json
import { Marked, Tokenizer } from "marked";
import { Chalk } from "chalk";
import { Markdown } from "../../tui/dist/components/markdown.js";
import { setCapabilities } from "../../tui/dist/terminal-image.js";

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

const chalk = new Chalk({ level: 3 });
const theme = {
  heading: (t) => chalk.bold.cyan(t),
  link: (t) => chalk.blue(t),
  linkUrl: (t) => chalk.dim(t),
  code: (t) => chalk.yellow(t),
  codeBlock: (t) => chalk.green(t),
  codeBlockBorder: (t) => chalk.dim(t),
  quote: (t) => chalk.italic(t),
  quoteBorder: (t) => chalk.dim(t),
  hr: (t) => chalk.dim(t),
  listBullet: (t) => chalk.cyan(t),
  bold: (t) => chalk.bold(t),
  italic: (t) => chalk.italic(t),
  strikethrough: (t) => chalk.strikethrough(t),
  underline: (t) => chalk.underline(t),
};
const styled = { color: (t) => chalk.gray(t), italic: true };
setCapabilities({ images: null, trueColor: true, hyperlinks: false });

const corpus = [
  "", "   ", "hello", "hello world\nsecond line", "para one\n\npara two", "a  \nb", "a\\\nb",
  "# H1", "## H2 ##", "### H3 #", "#### H4\ntext", "###### H6", "####### seven", "#hashtag", "# ", "#\tTab",
  "Setext\n===", "Setext 2\n---", "line\n---\nafter", "multi\nline\nsetext\n===",
  "---", "***", "___", "- - -", "* * *\ntext", "text\n\n---\n\nmore",
  "```\ncode\n```", "```js\nconst a = 1;\n```", "```\nunclosed", "~~~py\nx\n~~~", "````\n```\n````", "  ```\n  indented fence\n    more\n  ```",
  "```js title=x\nfoo\n```", "```\\*lang\nq\n```", "    indented code\n    more", "text\n    not code",
  "> quote", "> quote\ncontinued", "> a\n>\n> b", "> # heading\n> - item", "> nested\n>> deeper", "> ```\n> code\n> ```\nafter",
  "> - a\n- b", "> x\n> ---", "> lazy\nlazy2\n> back",
  "- a\n- b\n- c", "* a\n* b", "+ a\n+ b", "1. one\n2. two", "3) three\n4) four", "- a\n\n- b", "- a\n  - b\n    - c",
  "1. a\n   1. b\n   2. c\n2. d", "- [ ] todo\n- [x] done", "- [X] Done\n- [ ]", "-\n  after blank", "- a\n\n  para in item", "- a\ncontinuation",
  "- item\n```\ncode\n```", "- a\n# h", "- one\n\n\n- two", "10. ten\n11. eleven", "- a\n    code?", "- \ta", "1. x\n- y",
  "- a\n  ```\n  code\n  ```", "- a\n\n      indented code in item",
  "| a | b |\n|---|---|\n| 1 | 2 |", "a | b\n--|--\n1 | 2", "| a |\n|:-:|\n| c |", "| l | c | r |\n|:--|:-:|--:|\n| 1 | 2 | 3 |",
  "| a | b |\n|---|---|\n| only one |", "| a | b |\n|---|---|\n| 1 | 2 | 3 |", "| a \\| b | c |\n|---|---|\n| x | y |", "| h |\n|---|", "|a|b|\n|-|-|\n",
  "| **bold** | `code` |\n|---|---|\n| [l](u) | *i* |", "| a | b |\n| --- |",
  "**bold**", "*em*", "_em_", "__strong__", "***both***", "**bold *nested em* bold**", "*a **b** c*", "snake_case_word", "a*b*c", "a_b_c",
  "**unclosed", "*a", "** not bold **", "*(punct)*", "x**(y)**z", "***a** b*", "**a*", "_a __b__ a_", "* not a list*",
  "~~strike~~", "~one~", "~~ no ~~", "~~a~~b", "~~~x~~~", "~~a\\~~~",
  "`code`", "``co`de``", "` a `", "`  `", "`unclosed", "a `b` c `d`", "```inline```",
  "[link](http://x.com)", "[link](http://x.com \"title\")", "[l](<a b>)", "[l](a(b)c)", "[l](a(b)", "![img](src.png)", "![alt *x*](a.png 'T')",
  "<http://auto.link>", "<me@example.com>", "http://bare.com/path.", "www.example.com", "visit https://x.y/z) now", "mail me@x.org please",
  "[ref][r]\n\n[r]: http://ref.com", "[r]\n\n[r]: /url \"Title\"", "[R]\n\n[r]: /u", "[missing][nope]", "[x]: /a\n[y]: /b\n\n[x] [y]",
  "para\n[d]: /x", "[a][b]\n\n[b]: </x y> 'z'",
  "\\*not em\\*", "\\# not heading", "a \\\\ b", "\\`x\\`", "\\~~x~~",
  "<div>\nblock html\n</div>", "<!-- comment -->", "inline <b>bold</b> tag", "<pre>\n  pre\n</pre>", "<a href=\"x\">link</a> text", "<br/>", "<?php x ?>",
  "<script>\nalert(1)\n</script>", "a <span>b\nc</span> d",
  "émoji 😀 *em 😀*", "日本語 **太字** テキスト", "tab\there", "a\r\nb\rc", "trailing spaces   ", "&amp; &copy; &#169;",
  "**`code` in bold**", "*[link](u) in em*", "# Heading with `code` and **bold**", "## [link](http://x) heading",
  "Some text with a [link](https://example.com/very/long/path/that/goes/on) and more words to wrap the line across widths",
  "1. First item with enough text to wrap around the narrow width\n2. Second",
  "> A long quote line that should wrap across multiple lines when rendered narrow",
  "| Column A | Column B |\n|---|---|\n| a very long cell value that must wrap | short |",
  "text\n\n\n\nmore", "\n\nleading blank", "a\n\n```\nx\n```\nb", "# H\n\n## H2\ntext\n### H3",
  "- a\n\n  b\n- c", "*", "**", "_", "__init__", "2 * 3 * 4", "x ~~y~~ z ~~w", "[]()", "[a]()", "[a](#)", "!", "![", "a  b",
];

const out = [];
for (const src of corpus) {
  const tokens = parser.lexer(src.replace(/\t/g, "   "));
  const links = Object.entries(tokens.links).map(([k, v]) => [k, canon(v)]);
  const renders = {};
  for (const w of [80, 24]) {
    renders[w] = new Markdown(src, 0, 0, theme).render(w);
  }
  renders.padded = new Markdown(src, 2, 1, theme).render(40);
  renders.styled = new Markdown(src, 1, 0, theme, styled).render(40);
  out.push({ src, tokens: JSON.stringify(canon([...tokens])), links: JSON.stringify(links), renders });
}
console.log(JSON.stringify(out, null, 1));
