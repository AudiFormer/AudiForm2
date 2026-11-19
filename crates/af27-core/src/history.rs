// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! Snapshot-based undo history. Each entry holds the state before an
//! edit plus a label shown in the History panel. States are usually
//! `Arc`-wrapped so unchanged data is shared, not copied.

#[derive(Debug, Clone)]
pub struct History<T: Clone> {
    undo: Vec<(String, T)>,
    redo: Vec<(String, T)>,
    limit: usize,
}

impl<T: Clone> Default for History<T> {
    fn default() -> Self {
        Self::new(200)
    }
}

impl<T: Clone> History<T> {
    pub fn new(limit: usize) -> Self {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Records `before` as the state to return to, under `label`.
    pub fn push(&mut self, label: impl Into<String>, before: T) {
        self.undo.push((label.into(), before));
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Steps back: takes the current state, returns the earlier one.
    pub fn undo(&mut self, current: T) -> Option<T> {
        let (label, prev) = self.undo.pop()?;
        self.redo.push((label, current));
        Some(prev)
    }

    pub fn redo(&mut self, current: T) -> Option<T> {
        let (label, next) = self.redo.pop()?;
        self.undo.push((label, current));
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|(l, _)| l.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|(l, _)| l.as_str())
    }
    /// Labels oldest first, for the History panel.
    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.undo.iter().map(|(l, _)| l.as_str())
    }
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_walk() {
        let mut h = History::new(10);
        h.push("one", 0);
        h.push("two", 1);
        let mut state = 2;
        state = h.undo(state).unwrap();
        assert_eq!(state, 1);
        state = h.undo(state).unwrap();
        assert_eq!(state, 0);
        assert!(h.undo(state).is_none());
        state = h.redo(state).unwrap();
        assert_eq!(state, 1);
        h.push("three", state);
        assert!(!h.can_redo());
    }

    #[test]
    fn limit_drops_oldest() {
        let mut h = History::new(2);
        for i in 0..5 {
            h.push(format!("{i}"), i);
        }
        assert_eq!(h.labels().collect::<Vec<_>>(), vec!["3", "4"]);
    }
}
