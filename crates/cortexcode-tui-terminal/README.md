# cortexcode-tui-terminal

Terminal abstraction for the cortex TUI: raw mode, dimensions, cursor
control, bracketed paste, Kitty keyboard protocol negotiation, and OSC
progress/title sequences.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Ported from `@kolisachint/hoocode-tui`'s `terminal.ts` and `stdin-buffer.ts`.
Raw mode and terminal size are handled via `crossterm`; other escape
sequences are written directly to match hoocode's byte-for-byte behavior.

Deferred: the Windows `ENABLE_VIRTUAL_TERMINAL_INPUT` console-mode tweak
(`koffi`-based in the original) is not ported — Shift+Tab may not be
distinguishable from Tab on Windows consoles without it.
