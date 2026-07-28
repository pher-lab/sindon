//! Text-caret blink policy and phase.
//!
//! A focused text field's insertion caret blinks: solid for one interval, gone
//! for the next, and so on. Two things make a blink pleasant rather than
//! distracting, and both live here:
//!
//! - **It's driven by a *timed* wake, not a frame-rate pump.** The widget asks
//!   [`sindon_reactive::animation::request_frame_at`] for the next toggle
//!   instant; the event loop sleeps until then and repaints once. A focused,
//!   idle field costs two frames a second, not sixty.
//! - **It respects the OS setting.** The interval comes from the platform
//!   (Windows' `GetCaretBlinkTime`), and a user who disabled blinking entirely
//!   (an accessibility choice) gets [`CaretBlink::Off`] — a solid caret, no
//!   wakes. `sindon_app` publishes the platform value at startup via
//!   [`set_caret_blink_from_system`]; an app can override either with
//!   [`set_caret_blink`].
//!
//! The blink *reference* — when the current solid phase began — is owned by
//! each widget, which resets it to "now" on any caret activity (a keystroke, an
//! arrow, a click) so the caret is solid *while you're typing* and only resumes
//! blinking after a pause. The pure [`blink_phase`] here turns a
//! `(reference, now, interval)` into "draw it?" plus "when's the next toggle?".

use std::cell::Cell;
use std::time::{Duration, Instant};

/// The Windows default caret blink interval, and our fallback when the platform
/// can't be queried. This is the half-period: the caret is solid for one
/// `Interval`, hidden for the next.
pub const DEFAULT_CARET_BLINK: Duration = Duration::from_millis(530);

/// How (and whether) the text caret blinks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretBlink {
    /// Blink with this half-period: solid for one `Interval`, hidden the next.
    Interval(Duration),
    /// Don't blink — draw a solid caret. This is what a user who turned caret
    /// blinking off (Windows exposes this as an accessibility setting) gets,
    /// and it schedules no timed wakes.
    Off,
}

impl Default for CaretBlink {
    fn default() -> Self {
        CaretBlink::Interval(DEFAULT_CARET_BLINK)
    }
}

thread_local! {
    /// The active blink policy. Read by focused text widgets at paint. Starts
    /// at the platform default and is replaced by the OS value at startup
    /// (unless an app opted to drive it itself). Thread-local because the UI
    /// runs single-threaded on the event-loop thread, like the animation frame
    /// vote and `system_theme_signal`.
    static CARET_BLINK: Cell<CaretBlink> = const { Cell::new(CaretBlink::Interval(DEFAULT_CARET_BLINK)) };

    /// Set once an app calls [`set_caret_blink`]. It makes the startup platform
    /// publish a no-op: an app that deliberately chose a policy (a privacy
    /// "focus mode" that disables the blink, say) must win over the OS value
    /// the event loop reads on the way up.
    static CARET_BLINK_USER_SET: Cell<bool> = const { Cell::new(false) };
}

/// The active caret-blink policy. Focused [`Input`](crate::Input) /
/// [`SecureInput`](crate::SecureInput) read this each paint.
pub fn caret_blink() -> CaretBlink {
    CARET_BLINK.with(Cell::get)
}

/// Override the caret-blink policy for the process. Marks the policy as
/// app-chosen, so the OS value published at startup won't clobber it. Call
/// [`CaretBlink::Off`] to pin a solid caret (a focus/privacy mode), or an
/// `Interval` to force a rate.
pub fn set_caret_blink(blink: CaretBlink) {
    CARET_BLINK.with(|c| c.set(blink));
    CARET_BLINK_USER_SET.with(|u| u.set(true));
}

/// Publish the OS-provided policy. No-op once [`set_caret_blink`] has run, so
/// an app override survives the event loop reading the system value on startup.
/// Called by `sindon_app`; apps use [`set_caret_blink`] instead.
#[doc(hidden)]
pub fn set_caret_blink_from_system(blink: CaretBlink) {
    if CARET_BLINK_USER_SET.with(Cell::get) {
        return;
    }
    CARET_BLINK.with(|c| c.set(blink));
}

/// Given when the current solid phase began (`reference`), the current time,
/// and the blink half-period, return whether the caret is visible now and the
/// instant of the next toggle (what the widget votes as its timed wake).
///
/// Even half-cycles are visible, odd ones hidden, so a freshly reset caret
/// (`now == reference`) is solid — which is what keeps it solid the moment you
/// type. Time before `reference` saturates to zero rather than panicking, and
/// an idle so long the toggle index overflows a `u32` clamps instead of
/// wrapping (that's ~72 years at the default rate — purely defensive).
pub fn blink_phase(reference: Instant, now: Instant, interval: Duration) -> (bool, Instant) {
    let ivl = interval.as_nanos().max(1);
    let halves = (now.saturating_duration_since(reference).as_nanos() / ivl) as u64;
    let visible = halves.is_multiple_of(2);

    let next_index = u32::try_from(halves.saturating_add(1)).unwrap_or(u32::MAX);
    let next = interval
        .checked_mul(next_index)
        .and_then(|offset| reference.checked_add(offset))
        .unwrap_or_else(|| now + interval);
    (visible, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_at_reference_and_across_the_first_interval() {
        let reference = Instant::now();
        let ivl = Duration::from_millis(500);

        // The moment the phase resets, and anywhere inside the first interval,
        // the caret is solid — this is the "solid while typing" guarantee.
        let (vis, next) = blink_phase(reference, reference, ivl);
        assert!(vis, "solid the instant the phase resets");
        assert_eq!(next, reference + ivl, "first toggle is one interval out");

        let (vis, next) = blink_phase(reference, reference + Duration::from_millis(250), ivl);
        assert!(vis, "still solid a quarter of the way in");
        assert_eq!(
            next,
            reference + ivl,
            "toggle instant doesn't drift mid-phase"
        );
    }

    #[test]
    fn toggles_off_then_on_on_interval_boundaries() {
        let reference = Instant::now();
        let ivl = Duration::from_millis(500);

        // Exactly on the first boundary: hidden, next toggle one interval later.
        let (vis, next) = blink_phase(reference, reference + ivl, ivl);
        assert!(!vis, "hidden once the first interval elapses");
        assert_eq!(next, reference + 2 * ivl);

        // Second boundary: visible again.
        let (vis, next) = blink_phase(reference, reference + 2 * ivl, ivl);
        assert!(vis, "visible again after two intervals");
        assert_eq!(next, reference + 3 * ivl);

        // Deep into an odd half-cycle stays hidden.
        let (vis, _) = blink_phase(reference, reference + ivl + Duration::from_millis(1), ivl);
        assert!(!vis);
    }

    #[test]
    fn time_before_reference_saturates_to_solid() {
        // A frozen clock placed before `reference` (or a reference set in a
        // later paint than `now` was read) must not panic on Instant subtraction.
        let reference = Instant::now();
        let earlier = reference - Duration::from_millis(100);
        let (vis, _) = blink_phase(reference, earlier, Duration::from_millis(500));
        assert!(vis, "elapsed saturates to zero → solid, no panic");
    }

    #[test]
    fn system_value_does_not_override_an_app_choice() {
        // Isolated to this test's thread via the thread-local. Order matters:
        // an explicit app choice must survive the startup platform publish.
        set_caret_blink(CaretBlink::Off);
        set_caret_blink_from_system(CaretBlink::Interval(Duration::from_millis(200)));
        assert_eq!(
            caret_blink(),
            CaretBlink::Off,
            "app override wins over the OS-published value"
        );
    }
}
