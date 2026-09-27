import { Lexer } from "marked";
const r = Lexer.rules;
const out = {};
for (const [k, v] of Object.entries(r.block.gfm)) out["block."+k] = v.source ? [v.source, v.flags] : null;
for (const [k, v] of Object.entries(r.inline.gfm)) out["inline."+k] = v.source ? [v.source, v.flags] : null;
console.log(JSON.stringify(out, null, 1));
