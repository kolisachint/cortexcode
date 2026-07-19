# cortexcode-tui-render

Differential rendering for the cortex TUI: the component tree
(`Component`/`Container`), overlay positioning, and the `Tui` differential
terminal writer that only rewrites changed lines.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Ported from `@kolisachint/hoocode-tui`'s `tui.ts`. Two performance-only
optimizations from the original were deliberately dropped (documented in
`src/tui.rs`): reference-identity flatten memoization (this port diffs by
line content instead, so the *output* is identical) and the 16ms render
coalescing timer (every `request_render()` renders immediately).

Threading model: `Tui`'s component tree uses `Rc<RefCell<dyn Component>>`
and is not `Send`, while `Terminal::start` reads stdin on a background
thread. `Tui::start` bridges the two via an `mpsc` channel of `TuiEvent`s
that the owning thread drains into `Tui::process_event`.
