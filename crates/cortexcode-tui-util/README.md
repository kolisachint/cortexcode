# cortexcode-tui-util

Shared utilities for the cortex TUI.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

ANSI-aware terminal string handling: grapheme-cluster-based visible-width
calculation (CJK/emoji-aware), word wrapping and truncation that preserve
SGR styling and OSC-8 hyperlinks across line breaks, and column-range
slicing for overlay compositing.

Simplification vs. the TypeScript source: emoji-width classification uses
the same fast heuristic pre-filter the TS code uses before its exact
`\p{RGI_Emoji}` regex check (an ECMAScript-only Unicode-set alias with no
Rust equivalent) — the heuristic *is* the classifier here. See the module
docs in `src/width.rs` for details.

Ported from TypeScript `@kolisachint/hoocode-tui` → `utils.ts`.
