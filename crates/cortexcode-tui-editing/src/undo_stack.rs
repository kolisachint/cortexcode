//! Generic undo stack with clone-on-push semantics, ported from `undo-stack.ts`.
//!
//! Stores clones of state snapshots. Popped snapshots are returned
//! directly (no re-cloning) since they are already detached from the
//! stack. `S: Clone` stands in for TypeScript's `structuredClone`.

#[derive(Debug, Default)]
pub struct UndoStack<S> {
    stack: Vec<S>,
}

impl<S: Clone> UndoStack<S> {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a clone of the given state onto the stack.
    pub fn push(&mut self, state: &S) {
        self.stack.push(state.clone());
    }

    /// Pop and return the most recent snapshot, or `None` if empty.
    pub fn pop(&mut self) -> Option<S> {
        self.stack.pop()
    }

    /// Remove all snapshots.
    pub fn clear(&mut self) {
        self.stack.clear();
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
}
