//! Undo and redo over snapshots of some state, ported from `undo-stack.ts`.
//!
//! Stores clones on the way in; hands snapshots back directly on the way out.
//! `S: Clone` stands in for TypeScript's `structuredClone`. `push` clears the
//! redo side itself: a fresh edit invalidates every redo.

#[derive(Debug, Default)]
pub struct UndoStack<S> {
    stack: Vec<S>,
    redo_stack: Vec<S>,
}

impl<S: Clone> UndoStack<S> {
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Record a state that can be returned to, and abandon any undone future.
    pub fn push(&mut self, state: &S) {
        self.stack.push(state.clone());
        self.redo_stack.clear();
    }

    /// Step back, given the state to return to if the step is later redone.
    pub fn undo(&mut self, current: &S) -> Option<S> {
        let snapshot = self.stack.pop()?;
        self.redo_stack.push(current.clone());
        Some(snapshot)
    }

    /// Step forward again, if an undo has left somewhere to go.
    pub fn redo(&mut self, current: &S) -> Option<S> {
        let snapshot = self.redo_stack.pop()?;
        self.stack.push(current.clone());
        Some(snapshot)
    }

    /// Pop the most recent snapshot without recording a redo (the pre-redo
    /// entry point; prefer [`UndoStack::undo`]).
    pub fn pop(&mut self) -> Option<S> {
        self.stack.pop()
    }

    /// Remove all snapshots, in both directions.
    pub fn clear(&mut self) {
        self.stack.clear();
        self.redo_stack.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop_round_trips() {
        let mut stack: UndoStack<String> = UndoStack::new();
        stack.push(&"one".to_string());
        stack.push(&"two".to_string());
        assert_eq!(stack.len(), 2);
        assert_eq!(stack.pop(), Some("two".to_string()));
        assert_eq!(stack.pop(), Some("one".to_string()));
        assert_eq!(stack.pop(), None);
    }

    #[test]
    fn push_clones_so_later_mutation_does_not_affect_stack() {
        let mut stack: UndoStack<Vec<i32>> = UndoStack::new();
        let mut state = vec![1, 2, 3];
        stack.push(&state);
        state.push(4);
        assert_eq!(stack.pop(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn clear_empties_the_stack() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        stack.push(&1);
        stack.push(&2);
        stack.clear();
        assert!(stack.is_empty());
        assert_eq!(stack.pop(), None);
    }

    #[test]
    fn undo_redo_round_trip_and_push_clears_redo() {
        let mut stack: UndoStack<String> = UndoStack::new();
        stack.push(&"a".to_string());
        stack.push(&"ab".to_string());
        // live state is "abc"
        assert_eq!(stack.undo(&"abc".to_string()), Some("ab".to_string()));
        assert_eq!(stack.undo(&"ab".to_string()), Some("a".to_string()));
        assert!(stack.can_redo());
        assert_eq!(stack.redo(&"a".to_string()), Some("ab".to_string()));
        assert_eq!(stack.redo(&"ab".to_string()), Some("abc".to_string()));
        assert_eq!(stack.redo(&"abc".to_string()), None);
        stack.undo(&"abc".to_string());
        stack.push(&"abX".to_string());
        assert!(!stack.can_redo());
        stack.clear();
        assert!(!stack.can_undo());
    }
}
