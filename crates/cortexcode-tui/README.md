# cortexcode-tui

Umbrella crate for the cortex TUI namespace: depending on `cortexcode-tui`
pulls in every `cortexcode-tui-*` leaf crate (`util`, `fuzzy`, `keys`,
`terminal`, `render`, `editing`, `images`, `components`), re-exported as
`cortexcode_tui::{util, fuzzy, keys, terminal, render, editing, images,
components}`.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.
