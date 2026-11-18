//! Undo and redo: a bounded list of earlier states and a list of undone ones.
//!
//! The history holds whole states (`T`), not edits: the app records the state as it
//! was before each change, and undoing swaps the current state for the newest recorded
//! one. Keeping states makes every kind of change undoable the same way; the app keeps
//! them cheap by sharing the large parts (meshes) between states.

use std::collections::VecDeque;

/// Earlier states to go back to, and undone states to go forward to again.
#[derive(Clone, Debug)]
pub struct History<T> {
    undo: VecDeque<T>,
    redo: Vec<T>,
    /// The most undo steps kept; the oldest are dropped beyond it.
    limit: usize,
}

impl<T> History<T> {
    /// An empty history keeping at most `limit` undo steps.
    pub fn new(limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Notes `before`, the state as it was before a change. Anything undone is dropped:
    /// after a new change it can't be redone.
    pub fn record(&mut self, before: T) {
        self.redo.clear();
        self.undo.push_back(before);
        while self.undo.len() > self.limit {
            self.undo.pop_front();
        }
    }

    /// The state to go back to, given the `current` one (kept for redo); `None` when
    /// there is nothing to undo.
    pub fn undo(&mut self, current: T) -> Option<T> {
        let back = self.undo.pop_back()?;
        self.redo.push(current);
        Some(back)
    }

    /// The undone state to go forward to again, given the `current` one (kept for
    /// undo); `None` when nothing was undone.
    pub fn redo(&mut self, current: T) -> Option<T> {
        let forward = self.redo.pop()?;
        self.undo.push_back(current);
        Some(forward)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Forgets every step (a new or opened project starts a new history).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_undo_and_redo_step_through_states() {
        let mut h = History::new(100);
        assert!(!h.can_undo() && !h.can_redo());
        // States 1 -> 2 -> 3, each recorded as the state before a change.
        h.record(1);
        h.record(2);
        assert_eq!(h.undo(3), Some(2));
        assert_eq!(h.undo(2), Some(1));
        assert_eq!(h.undo(1), None, "nothing left to undo");
        assert_eq!(h.redo(1), Some(2));
        assert_eq!(h.redo(2), Some(3));
        assert_eq!(h.redo(3), None, "nothing left to redo");
    }

    #[test]
    fn test_history_new_change_clears_redo() {
        let mut h = History::new(100);
        h.record(1);
        assert_eq!(h.undo(2), Some(1));
        assert!(h.can_redo());
        h.record(1);
        assert!(!h.can_redo(), "a new change drops what was undone");
        assert_eq!(h.undo(5), Some(1));
    }

    #[test]
    fn test_history_keeps_only_the_newest_steps() {
        let mut h = History::new(3);
        for i in 0..10 {
            h.record(i);
        }
        assert_eq!(h.undo(10), Some(9));
        assert_eq!(h.undo(9), Some(8));
        assert_eq!(h.undo(8), Some(7));
        assert_eq!(h.undo(7), None, "older steps were dropped");
    }

    #[test]
    fn test_history_clear() {
        let mut h = History::new(10);
        h.record(1);
        h.clear();
        assert!(!h.can_undo() && !h.can_redo());
    }
}
