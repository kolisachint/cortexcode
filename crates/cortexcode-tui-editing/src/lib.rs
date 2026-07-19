//! Text editing primitives for the cortex TUI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui`: `kill-ring.ts`
//! (Emacs-style kill/yank ring) and `undo-stack.ts` (clone-on-push undo
//! stack). `editor-component.ts` is a pure type interface with no runtime
//! logic of its own — it is tightly coupled to `AutocompleteProvider`
//! (`autocomplete.ts`) and the concrete `Editor` component
//! (`components/editor.ts`), so its Rust equivalent (an `EditorComponent`
//! trait extending `cortexcode_tui_render::Component`) is defined
//! alongside those in `cortexcode-tui-components` (Phase 2.7) instead of
//! here.

mod kill_ring;
mod undo_stack;

pub use kill_ring::{KillPushOptions, KillRing};
pub use undo_stack::UndoStack;
