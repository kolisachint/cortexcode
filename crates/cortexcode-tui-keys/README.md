# cortexcode-tui-keys

Keyboard handling for the cortex TUI.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Terminal key-sequence parsing and matching against key identifiers like
`"ctrl+c"`, `"shift+ctrl+p"`, `"escape"`. Supports both legacy terminal
escape sequences and the [Kitty keyboard
protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) (CSI-u) plus
xterm's `modifyOtherKeys`, including modifier combinations, function/arrow
keys, key press/repeat/release events, and non-Latin keyboard layouts (base
layout key fallback).

Also includes a `KeybindingsManager` registry: default keybindings, user
overrides, and conflict detection.

Simplification vs. the TypeScript source: the global `getKeybindings()` /
`setKeybindings()` singleton isn't ported — callers hold their own
`KeybindingsManager` instance instead.

Ported from TypeScript `@kolisachint/hoocode-tui` → `keys.ts`, `keybindings.ts`.
