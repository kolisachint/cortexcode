# cortexcode-tui-components

UI widgets for the cortex TUI: `Spacer`, `Text`, `TruncatedText`,
`BoxComponent`, `Image`, `Loader`, `CancellableLoader`, `SelectList`, and
more as they're ported.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Ported from `@kolisachint/hoocode-tui`'s `components/*.ts` (plus
`autocomplete.ts` and `editor-component.ts`, which live at the package
root in the original but are tightly coupled to the `Editor` component).

`Loader`'s animation timing and `Tui`'s render coalescing share the same
adaptation: TypeScript calls back into shared mutable state from a
`setInterval`/timer, which doesn't translate to Rust's `Rc<RefCell<...>>`
component tree across threads. `Loader::tick()` is a plain method the
owning event loop calls periodically instead.
