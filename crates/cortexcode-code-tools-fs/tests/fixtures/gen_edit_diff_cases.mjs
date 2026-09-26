// Regenerates edit_diff_cases.json from hoocode's own edit-diff.js (the pin),
// so the Rust port is checked against the reference implementation:
//   node crates/cortexcode-code-tools-fs/tests/fixtures/gen_edit_diff_cases.mjs \
//     target/hoocode-pin/packages/coding-agent/dist/core/tools/edit-diff.js > .../edit_diff_cases.json
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";

const mod = await import(pathToFileURL(resolve(process.argv[2])).href);
const { applyEditsToNormalizedContent, generateDiffString } = mod;

let seed = 20260926;
const rand = () => {
	seed = (seed * 1103515245 + 12345) & 0x7fffffff;
	return seed / 0x7fffffff;
};
const pick = (xs) => xs[Math.floor(rand() * xs.length)];
const int = (n) => Math.floor(rand() * n);

const words = ["alpha", "beta", "gamma", "x", "y", "return", "const", "fn", "if", "{", "}", "(", ")", ";", "=", "é", "é", "ﬁle", "😀"];
const spaces = [" ", " ", " ", "  ", "\t", " ", "   "];
const quotes = ["'", "‘", "’", '"', "“", "”", "-", "—", "–"];

function line() {
	let s = pick(["", "\t", "  ", "    ", "\t\t", " "]);
	const n = 1 + int(5);
	for (let i = 0; i < n; i++) {
		s += pick(words);
		if (rand() < 0.3) s += pick(quotes);
		if (i < n - 1) s += pick(spaces);
	}
	if (rand() < 0.15) s += pick([" ", "  ", "\t"]);
	return s;
}

function file() {
	const n = 1 + int(14);
	const lines = [];
	for (let i = 0; i < n; i++) lines.push(rand() < 0.2 && i > 0 ? lines[int(i)] : line());
	return lines.join("\n") + (rand() < 0.7 ? "\n" : "");
}

// A model's rendering of text: tabs as spaces, smart punctuation as ASCII, ...
function fuzz(text) {
	return text
		.replace(/\t/g, rand() < 0.5 ? "  " : "    ")
		.replace(/[‘’]/g, "'")
		.replace(/[“”]/g, '"')
		.replace(/[–—]/g, "-")
		.replace(/ /g, " ")
		.replace(/ +$/gm, "");
}

function edit(content) {
	const lines = content.split("\n");
	const start = int(lines.length);
	const len = 1 + int(Math.min(3, lines.length - start));
	let oldText = lines.slice(start, start + len).join("\n");
	if (rand() < 0.3 && oldText.length > 3) {
		const a = int(oldText.length - 2);
		oldText = oldText.slice(a, a + 1 + int(oldText.length - a));
	}
	const r = rand();
	if (r < 0.35) oldText = fuzz(oldText);
	else if (r < 0.45) oldText = oldText.replace(/^[ \t]+/gm, "");
	else if (r < 0.5) oldText = oldText + "\n";
	let newText;
	const k = rand();
	if (k < 0.1) newText = oldText;
	else if (k < 0.2) newText = fuzz(oldText);
	else if (k < 0.3) newText = "";
	else newText = oldText.replace(/[a-z]+/, (w) => w.toUpperCase()) + (rand() < 0.3 ? "\n" + line() : "");
	const e = { oldText, newText };
	if (rand() < 0.1) e.replaceAll = true;
	return e;
}

// JSON parsers reject lone surrogates, so no tool call can carry one.
const loneSurrogate = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/;

const applyCases = [];
while (applyCases.length < 1000) {
	const content = file();
	const n = rand() < 0.75 ? 1 : 2 + int(2);
	const edits = [];
	for (let j = 0; j < n; j++) edits.push(edit(content));
	if (rand() < 0.02) edits[0].oldText = "";
	if (edits.some((e) => loneSurrogate.test(e.oldText) || loneSurrogate.test(e.newText))) continue;
	let result;
	try {
		const { baseContent, newContent } = applyEditsToNormalizedContent(content, edits, "f.txt");
		result = { ok: newContent, diff: generateDiffString(baseContent, newContent) };
	} catch (e) {
		result = { err: e.message };
	}
	applyCases.push({ content, edits, ...result });
}

const diffCases = [];
for (let i = 0; i < 300; i++) {
	const a = file();
	const lines = a.split("\n");
	for (let j = 0; j < 1 + int(4); j++) {
		const at = int(lines.length + 1);
		const r = rand();
		if (r < 0.33) lines.splice(at, 0, line());
		else if (r < 0.66) lines.splice(at, 1);
		else lines.splice(at, 1, line(), line());
	}
	const b = lines.join("\n");
	const context = pick([4, 4, 1, 0, 2]);
	diffCases.push({ old: a, new: b, context, ...generateDiffString(a, b, context) });
}

process.stdout.write(JSON.stringify({ apply: applyCases, diff: diffCases }));
