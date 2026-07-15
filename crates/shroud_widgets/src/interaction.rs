//! Shared ephemeral interaction state for the "control" widgets.
//!
//! [`Button`](crate::Button), [`Dropdown`](crate::Dropdown),
//! [`MenuItem`](crate::MenuItem) and [`Checkbox`](crate::Checkbox) all track
//! the same three pointer / keyboard flags — hovered, pressed, focused — and
//! used to hand-roll the transitions for each. That copy-paste is exactly how
//! a subtle invariant drifts between widgets; it already produced one real bug
//! (a control disabled mid-interaction stranding a stale hover / focus / press
//! that resurfaced on re-enable — see `fix(widgets): Button clears
//! hover/focus/press even while disabled`).
//!
//! This centralizes the flags and, crucially, the one rule that is easy to get
//! wrong per-widget:
//!
//! > **A control gated by `disabled` may never *enter* an active state, but
//! > must always be allowed to *leave* one.**
//!
//! So the *latching* transitions ([`enter`](InteractionState::enter),
//! [`press`](InteractionState::press),
//! [`focus_gained`](InteractionState::focus_gained)) are gated on `disabled`,
//! while the *clearing* transitions ([`leave`](InteractionState::leave),
//! [`release`](InteractionState::release),
//! [`focus_lost`](InteractionState::focus_lost)) always run. The widget keeps
//! its own activation semantics (click vs toggle vs select) and event return
//! values — this type only owns the flag bookkeeping and that invariant.
//!
//! Hover *representation* still varies (Button eases the background with an
//! [`Animated`](shroud_reactive::Animated); the others flip a bool), so a
//! widget with an animated hover drives its animator alongside these flags.

/// The three interaction flags a control widget tracks. `Copy` — it is a plain
/// bag of bools, cheap to pass by value.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InteractionState {
    /// The pointer is over the widget.
    pub hovered: bool,
    /// A primary-button press began on the widget and has not yet been
    /// released or dragged off.
    pub pressed: bool,
    /// The widget holds keyboard focus.
    pub focused: bool,
}

/// Outcome of a primary-button release, telling the caller whether to fire its
/// activation. See [`InteractionState::release`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Release {
    /// A press completed on an enabled control — fire the click / toggle /
    /// select, and treat the event as consumed.
    Fire,
    /// A press was in flight but the control is now disabled — the latch is
    /// cleared and no activation fires, but the release was still handled
    /// (consume it).
    Cancelled,
    /// No press was in flight — the release is not ours (ignore it).
    Idle,
}

impl InteractionState {
    /// Pointer entered. A *latching* transition: enters hover only while
    /// enabled, so a disabled control does not light up.
    pub fn enter(&mut self, disabled: bool) {
        if !disabled {
            self.hovered = true;
        }
    }

    /// Pointer left. A *clearing* transition: always drops hover and any press
    /// in flight — even while disabled — so nothing is stranded to resurface
    /// when the control is re-enabled.
    pub fn leave(&mut self) {
        self.hovered = false;
        self.pressed = false;
    }

    /// Primary-button press. A *latching* transition: latches only while
    /// enabled. Returns whether it latched, so the caller can consume the
    /// event (`true`) or ignore it (`false`, disabled).
    pub fn press(&mut self, disabled: bool) -> bool {
        if disabled {
            return false;
        }
        self.pressed = true;
        true
    }

    /// Primary-button release. A *clearing* transition: always releases the
    /// press latch. Reports via [`Release`] whether the caller should fire its
    /// activation (a completed press on an enabled control).
    pub fn release(&mut self, disabled: bool) -> Release {
        if !self.pressed {
            return Release::Idle;
        }
        self.pressed = false;
        if disabled {
            Release::Cancelled
        } else {
            Release::Fire
        }
    }

    /// Keyboard focus gained. A *latching* transition: latches only while
    /// enabled (a disabled control is out of the Tab order, so this guards a
    /// stray programmatic focus).
    pub fn focus_gained(&mut self, disabled: bool) {
        if !disabled {
            self.focused = true;
        }
    }

    /// Keyboard focus lost. A *clearing* transition: always drops focus, even
    /// while disabled — the tree blurs the outgoing widget when Tab moves on,
    /// and a swallowed blur would leave the ring to resurface on re-enable.
    pub fn focus_lost(&mut self) {
        self.focused = false;
    }
}

/// Move a single-selection index by `delta`, clamped to `[0, len)` — no wrap.
///
/// The shared arrow-navigation step for the single-select controls
/// ([`Segmented`](crate::Segmented), [`RadioGroup`](crate::RadioGroup)): a
/// native segmented control / radio group moves selection with the arrow keys
/// but stops at the ends rather than wrapping. Returns `current` unchanged when
/// the option list is empty.
pub(crate) fn step_selection(current: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return current;
    }
    let max = (len - 1) as i64;
    (current as i64 + delta as i64).clamp(0, max) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_selection_clamps_without_wrapping() {
        // Middle moves freely.
        assert_eq!(step_selection(1, 3, 1), 2);
        assert_eq!(step_selection(1, 3, -1), 0);
        // Ends clamp rather than wrap.
        assert_eq!(step_selection(2, 3, 1), 2, "last stays last");
        assert_eq!(step_selection(0, 3, -1), 0, "first stays first");
        // Empty list is inert.
        assert_eq!(step_selection(0, 0, 1), 0);
    }

    // The headline invariant, proven once here so every widget that composes
    // InteractionState inherits it: disabled blocks *entering* an active state
    // but never blocks *leaving* one.
    #[test]
    fn disabled_blocks_latching_not_clearing() {
        // Latch everything while enabled.
        let mut s = InteractionState::default();
        s.enter(false);
        assert!(s.press(false));
        s.focus_gained(false);
        assert_eq!(
            s,
            InteractionState {
                hovered: true,
                pressed: true,
                focused: true
            }
        );

        // Now disabled: clearing transitions must still fire.
        s.leave();
        assert!(!s.hovered, "leave clears hover even while disabled");
        assert!(!s.pressed, "leave clears press even while disabled");
        s.focus_lost();
        assert!(!s.focused, "focus_lost clears focus even while disabled");
    }

    #[test]
    fn disabled_latching_transitions_are_no_ops() {
        let mut s = InteractionState::default();
        s.enter(true);
        assert!(!s.hovered, "disabled control does not hover");
        assert!(!s.press(true), "disabled press does not latch");
        assert!(!s.pressed);
        s.focus_gained(true);
        assert!(!s.focused, "disabled control does not take focus");
    }

    #[test]
    fn release_reports_fire_cancel_idle() {
        // Fire: a press completed on an enabled control.
        let mut s = InteractionState::default();
        s.press(false);
        assert_eq!(s.release(false), Release::Fire);
        assert!(!s.pressed, "release always clears the latch");

        // Cancelled: press began enabled, control disabled before release.
        let mut s = InteractionState::default();
        s.press(false);
        assert_eq!(s.release(true), Release::Cancelled);
        assert!(!s.pressed, "release clears the latch even while disabled");

        // Idle: no press in flight.
        let mut s = InteractionState::default();
        assert_eq!(s.release(false), Release::Idle);
    }
}
