# cortexcode-tui-components

UI widgets for the cortex TUI: `Spacer`, `Text`, `TruncatedText`,
`BoxComponent`, `Image`, `Loader`, `CancellableLoader`, `SelectList`,
`SettingsList`, `Input`, `Markdown`, and the `AutocompleteProvider`.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Ported from `@kolisachint/hoocode-tui`'s `components/*.ts` (plus
`autocomplete.ts` and `editor-component.ts`, which live at the package
root in the original but are tightly coupled to the `Editor` component).

`Loader`'s animation timing and `Tui`'s render coalescing share the same
adaptation: TypeScript calls back into shared mutable state from a
`setInterval`/timer, which doesn't translate to Rust's `Rc<RefCell<...>>`
component tree across threads. `Loader::tick()` is a plain method the
owning event loop calls periodically instead.

`Markdown` parses with `pulldown-cmark` instead of `marked` (a different
parser model — flat events vs. a token tree), via a small intermediate
AST; see `src/markdown/mod.rs` for the resulting fidelity trade-offs
(table column sizing, nested-style ANSI-reset recovery, no footnotes/
definition lists/inline images).
