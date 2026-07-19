//! Ring buffer for Emacs-style kill/yank operations, ported from `kill-ring.ts`.
//!
//! Tracks killed (deleted) text entries. Consecutive kills can accumulate
//! into a single entry. Supports yank (peek the most recent) and yank-pop
//! (cycle through older entries).

#[derive(Debug, Clone, Copy, Default)]
pub struct KillPushOptions {
    /// If accumulating, prepend (backward deletion) or append (forward deletion).
    pub prepend: bool,
    /// Merge with the most recent entry instead of creating a new one.
    pub accumulate: bool,
}

#[derive(Debug, Default)]
pub struct KillRing {
    ring: Vec<String>,
}

impl KillRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add text to the kill ring.
    pub fn push(&mut self, text: &str, opts: KillPushOptions) {
        if text.is_empty() {
            return;
        }
        if opts.accumulate {
            if let Some(last) = self.ring.pop() {
                let merged = if opts.prepend {
                    format!("{text}{last}")
                } else {
                    format!("{last}{text}")
                };
                self.ring.push(merged);
                return;
            }
        }
        self.ring.push(text.to_string());
    }

    /// Get the most recent entry without modifying the ring.
    pub fn peek(&self) -> Option<&str> {
        self.ring.last().map(String::as_str)
    }

    /// Move the last entry to the front (for yank-pop cycling).
    pub fn rotate(&mut self) {
        if self.ring.len() > 1 {
            if let Some(last) = self.ring.pop() {
                self.ring.insert(0, last);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_peek() {
        let mut ring = KillRing::new();
        ring.push("hello", KillPushOptions::default());
        assert_eq!(ring.peek(), Some("hello"));
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn push_empty_text_is_ignored() {
        let mut ring = KillRing::new();
        ring.push("", KillPushOptions::default());
        assert!(ring.is_empty());
    }

    #[test]
    fn accumulate_appends_by_default() {
        let mut ring = KillRing::new();
        ring.push("foo", KillPushOptions::default());
        ring.push(
            "bar",
            KillPushOptions {
                accumulate: true,
                prepend: false,
            },
        );
        assert_eq!(ring.peek(), Some("foobar"));
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn accumulate_prepends_for_backward_deletion() {
        let mut ring = KillRing::new();
        ring.push("foo", KillPushOptions::default());
        ring.push(
            "bar",
            KillPushOptions {
                accumulate: true,
                prepend: true,
            },
        );
        assert_eq!(ring.peek(), Some("barfoo"));
    }

    #[test]
    fn accumulate_on_empty_ring_just_pushes() {
        let mut ring = KillRing::new();
        ring.push(
            "solo",
            KillPushOptions {
                accumulate: true,
                prepend: false,
            },
        );
        assert_eq!(ring.peek(), Some("solo"));
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn non_accumulating_push_creates_new_entry() {
        let mut ring = KillRing::new();
        ring.push("foo", KillPushOptions::default());
        ring.push("bar", KillPushOptions::default());
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.peek(), Some("bar"));
    }

    #[test]
    fn rotate_cycles_most_recent_to_front() {
        let mut ring = KillRing::new();
        ring.push("a", KillPushOptions::default());
        ring.push("b", KillPushOptions::default());
        ring.push("c", KillPushOptions::default());
        // ring: [a, b, c], peek = c
        ring.rotate();
        // c moves to front: [c, a, b], peek = b
        assert_eq!(ring.peek(), Some("b"));
        ring.rotate();
        // [b, c, a], peek = a
        assert_eq!(ring.peek(), Some("a"));
    }

    #[test]
    fn rotate_on_single_entry_is_noop() {
        let mut ring = KillRing::new();
        ring.push("solo", KillPushOptions::default());
        ring.rotate();
        assert_eq!(ring.peek(), Some("solo"));
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn rotate_on_empty_ring_is_noop() {
        let mut ring = KillRing::new();
        ring.rotate();
        assert!(ring.is_empty());
    }
}
