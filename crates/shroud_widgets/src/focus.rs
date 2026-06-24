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

/// Why focus moved to a widget — drives the `:focus-visible` heuristic
/// that decides whether a focus ring is painted.
///
/// Mirrors the web platform's `:focus-visible`: a ring is a navigation aid
/// for keyboard users, so it shows for keyboard and programmatic focus but
/// is suppressed when the user pointed straight at the widget (they already
/// know where focus landed, and a ring on every click reads as noise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusReason {
    /// Focus followed a pointer press (click-to-focus). Ring suppressed.
    Pointer,
    /// Focus moved via the keyboard (Tab / Shift+Tab). Ring shown.
    Keyboard,
    /// Focus was moved programmatically by the app — e.g. focusing the
    /// first field after a screen transition, or a `TreeCommand::Focus`.
    /// Treated like keyboard navigation (ring shown) since the user did
    /// not point at the widget themselves.
    Programmatic,
}

impl FocusReason {
    /// Whether a focus ring should be painted for focus acquired this way.
    pub fn shows_ring(self) -> bool {
        !matches!(self, FocusReason::Pointer)
    }
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
    /// Whether the current focus should display a ring (the
    /// `:focus-visible` heuristic). Meaningless when `focused` is `None`,
    /// and forced to `false` whenever focus clears.
    visible: bool,
}

impl FocusManager {
    pub fn new() -> Self {
        Self {
            focused: None,
            visible: false,
        }
    }

    /// Currently focused widget index, or `None` if no widget has focus.
    pub fn focused(&self) -> Option<usize> {
        self.focused
    }

    /// Set the focused widget. Returns the previously focused index so
    /// the caller (typically `WidgetTree`) can dispatch `FocusLost`
    /// to it before dispatching `FocusGained` to the new one.
    ///
    /// Clearing focus (`idx == None`) also drops the visible flag; a new
    /// focus leaves visibility untouched here — the caller sets it via
    /// [`set_visible`](Self::set_visible) from the focus reason.
    pub fn set(&mut self, idx: Option<usize>) -> Option<usize> {
        let prev = self.focused;
        self.focused = idx;
        if idx.is_none() {
            self.visible = false;
        }
        prev
    }

    /// Whether the current focus should paint a ring (`:focus-visible`).
    /// Always `false` when nothing is focused.
    pub fn visible(&self) -> bool {
        self.focused.is_some() && self.visible
    }

    /// Set the visible flag from the reason focus moved. Called by the
    /// tree's focus entrypoint right after [`set`](Self::set).
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Clear focus if it currently points at `idx`. Called when a widget
    /// is being removed from the tree so focus does not dangle on a
    /// tombstoned slot.
    pub fn clear_if(&mut self, idx: usize) {
        if self.focused == Some(idx) {
            self.focused = None;
            self.visible = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pointer_focus_hides_the_ring() {
        assert!(!FocusReason::Pointer.shows_ring());
        assert!(FocusReason::Keyboard.shows_ring());
        assert!(FocusReason::Programmatic.shows_ring());
    }

    #[test]
    fn visible_requires_a_focused_widget() {
        let mut fm = FocusManager::new();
        // Even if a stale visible flag were set, no focus means no ring.
        fm.set_visible(true);
        assert!(!fm.visible(), "nothing focused → never visible");

        fm.set(Some(3));
        fm.set_visible(true);
        assert!(fm.visible());
    }

    #[test]
    fn clearing_focus_drops_visibility() {
        let mut fm = FocusManager::new();
        fm.set(Some(1));
        fm.set_visible(true);
        assert!(fm.visible());

        // Clearing via set(None) and via clear_if both reset the flag.
        fm.set(None);
        assert!(!fm.visible());

        fm.set(Some(1));
        fm.set_visible(true);
        fm.clear_if(1);
        assert!(!fm.visible());
        assert_eq!(fm.focused(), None);
    }
}
