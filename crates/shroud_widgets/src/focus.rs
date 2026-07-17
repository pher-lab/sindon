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
    /// Focus is being handed back to the *same logical widget* after a
    /// rebuild replaced its node — see
    /// [`WidgetTree::refocus_initially`](crate::tree::WidgetTree::refocus_initially).
    ///
    /// Focus did not really move here: the widget the user was on is still
    /// the widget they are on, it just lives at a new index. So the ring
    /// state is not re-derived from a reason at all — it carries the flag
    /// focus already had, which is what keeps a rebuild invisible to the
    /// `:focus-visible` heuristic. Deriving it instead would light a ring
    /// on a widget the user reached by pointing at it.
    Restored {
        /// The `:focus-visible` flag in effect before the rebuild.
        visible: bool,
    },
    /// Focus is being handed back to the widget that *opened* a layer, after
    /// that layer was dismissed with focus still inside it — see
    /// [`WidgetTree::pop_layer`](crate::tree::WidgetTree::pop_layer).
    ///
    /// Distinct from [`Restored`](Self::Restored): focus really is moving, to a
    /// different widget than the one that had it. What carries over is not the
    /// widget but the *mode* the user was in — the ring flag of the focus the
    /// layer took away. Someone who tabbed through a dialog and pressed Escape
    /// is still navigating by keyboard when they land back on the trigger, and
    /// someone who clicked their way through it is still not.
    Returned {
        /// The `:focus-visible` flag of the focus the dismissed layer held.
        visible: bool,
    },
}

impl FocusReason {
    /// Whether a focus ring should be painted for focus acquired this way.
    pub fn shows_ring(self) -> bool {
        match self {
            FocusReason::Pointer => false,
            FocusReason::Keyboard | FocusReason::Programmatic => true,
            FocusReason::Restored { visible } | FocusReason::Returned { visible } => visible,
        }
    }

    /// Whether focus acquired this way should scroll the widget into view —
    /// see [`WidgetTree::reveal_pending_focus`](crate::tree::WidgetTree).
    ///
    /// A near-twin of [`shows_ring`](Self::shows_ring), but a different
    /// question, so the two are kept apart: a ring says "focus is here", a
    /// reveal says "focus went somewhere you can't see". `Restored` is where
    /// they part company.
    pub fn scrolls_into_view(self) -> bool {
        match self {
            // The user pointed at the widget, so it is on screen already.
            FocusReason::Pointer => false,
            // These can land anywhere in the tree, including well outside a
            // scrolled viewport — the reason the reveal exists at all.
            FocusReason::Keyboard | FocusReason::Programmatic => true,
            // Focus did not really move: the widget is where the user left it.
            // Scrolling would turn an invisible rebuild into a visible jump.
            FocusReason::Restored { .. } => false,
            // Focus does move here, to a trigger that is usually still on
            // screen — but "usually" is not a guarantee (a layer can be opened
            // by a shortcut, from anywhere), and landing focus somewhere unseen
            // is the very thing the reveal exists to prevent.
            FocusReason::Returned { .. } => true,
        }
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
    fn restored_focus_shows_its_old_ring_but_never_scrolls() {
        // The one reason where "paint a ring" and "scroll into view" disagree:
        // a rebuild that restores a keyboard focus keeps the ring, yet must not
        // move the viewport under the user.
        assert!(FocusReason::Restored { visible: true }.shows_ring());
        assert!(!FocusReason::Restored { visible: true }.scrolls_into_view());
        assert!(!FocusReason::Restored { visible: false }.scrolls_into_view());
    }

    #[test]
    fn only_pointer_focus_skips_the_reveal() {
        assert!(!FocusReason::Pointer.scrolls_into_view());
        assert!(FocusReason::Keyboard.scrolls_into_view());
        assert!(FocusReason::Programmatic.scrolls_into_view());
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
