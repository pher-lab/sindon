//! Focus management — tree-global tracker for the keyboard-focused widget.
//!
//! `FocusManager` holds the index of the currently focused widget (if any)
//! and nothing else; the traversal logic that decides *which* widget gets
//! focus on Tab lives on [`WidgetTree`](crate::tree::WidgetTree), because
//! tab order is a function of the tree's shape (DFS pre-order over
//! `focusable` + `visible` nodes) and the tree is the thing that walks it.
//!
//! Owned by the tree — most callers reach it indirectly via
//! `tree.focused()` / `tree.focus(idx)` / `tree.advance_focus(dir)`.

/// Direction for Tab-style focus traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    /// Tab — move to the next focusable widget in tree order.
    Forward,
    /// Shift+Tab — move to the previous focusable widget in tree order.
    Backward,
}

/// Tree-global focus state.
///
/// A thin wrapper around `Option<usize>`. Kept as its own type so future
/// extensions (focus scopes, tab groups, "remember last focused within
/// subtree" behavior) have a place to grow without widening
/// `WidgetTree`'s surface area.
#[derive(Debug, Default)]
pub struct FocusManager {
    focused: Option<usize>,
}

impl FocusManager {
    pub fn new() -> Self {
        Self { focused: None }
    }

    /// Currently focused widget index, or `None` if no widget has focus.
    pub fn focused(&self) -> Option<usize> {
        self.focused
    }

    /// Set the focused widget. Returns the previously focused index so
    /// the caller (typically [`WidgetTree`]) can dispatch `FocusLost`
    /// to it before dispatching `FocusGained` to the new one.
    pub fn set(&mut self, idx: Option<usize>) -> Option<usize> {
        let prev = self.focused;
        self.focused = idx;
        prev
    }

    /// Clear focus if it currently points at `idx`. Called when a widget
    /// is being removed from the tree so focus does not dangle on a
    /// tombstoned slot.
    pub fn clear_if(&mut self, idx: usize) {
        if self.focused == Some(idx) {
            self.focused = None;
        }
    }
}
