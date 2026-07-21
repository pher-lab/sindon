//! Time-driven animated values.
//!
//! [`Animated<T>`] holds a value that, when retargeted via [`Animated::set`],
//! interpolates from its current value to the new one over a fixed duration
//! using an [`Easing`] curve. It reads like any other reactive source —
//! `Animated<T>: Into<Reactive<T>>` — so it drops straight into widget
//! builders that already accept `impl Into<Reactive<T>>` (e.g.
//! `Container::background`). The value is recomputed from the wall clock on
//! every `get()`, matching the framework's pull-based repaint model.
//!
//! # Driving redraws
//!
//! The system is pull-based: nothing repaints on its own. While an animation
//! is mid-flight, each `get()` registers a vote via [`request_frame`]. The
//! event loop calls [`reset_frame_request`] before a frame and
//! [`frame_requested`] after painting; if anything voted, it schedules
//! another redraw. When every animation has settled, no votes are cast and
//! the loop goes idle — no busy-looping at rest.
//!
//! ```ignore
//! use std::time::Duration;
//! use shroud_reactive::animation::{Animated, Easing};
//!
//! let bg = Animated::new(Color::BLACK, Duration::from_millis(200), Easing::EaseInOut);
//! // ... later, in a click handler:
//! bg.set(Color::WHITE); // begins a 200ms fade from the current value
//! // Container::new().background(bg.clone()) reads the eased value each paint.
//! ```
//!
//! # Testing
//!
//! Animations advance on the wall clock, so a test that asserts anything
//! about a fade *in flight* is racing a real timer. [`test_clock::freeze`]
//! pins the clock so the test steps time itself.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use shroud_core::Lerp;

use crate::reactive::Reactive;

thread_local! {
    /// Set true whenever an in-flight [`Animated::get`] is read during a
    /// frame. The event loop resets it before painting and checks it after,
    /// turning "did any animation advance this frame?" into "schedule one
    /// more redraw". Thread-local because the UI runs single-threaded on the
    /// event-loop thread, like `system_theme_signal`.
    static FRAME_REQUESTED: Cell<bool> = const { Cell::new(false) };

    /// The earliest instant a widget has asked to be repainted at, or `None`
    /// when nothing wants a *timed* wake. Unlike [`FRAME_REQUESTED`] (which
    /// asks for the very next vsync and, left set, busy-pumps at frame rate),
    /// this parks the loop until a specific deadline and fires once — the
    /// blinking text caret's toggle, say, which needs a repaint twice a second,
    /// not sixty times. The event loop folds this into its wait deadline and
    /// requests a redraw when it arrives. Reset each frame alongside
    /// `FRAME_REQUESTED`; a still-pending timed wake re-votes during paint.
    static FRAME_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };

    /// When `Some`, animations read this instead of the wall clock. Only
    /// [`test_clock`] ever sets it; in a real app it stays `None` for the
    /// process's life and [`now`] is just `Instant::now`. Thread-local for
    /// the same reason as `FRAME_REQUESTED`, and so a frozen clock in one
    /// test can't reach a test running concurrently on another thread.
    static CLOCK_OVERRIDE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// The current time as animations see it: the wall clock, unless a test has
/// frozen it via [`test_clock`]. Every read of "what time is it" in this
/// module goes through here — that's what makes a frozen clock total rather
/// than partial.
///
/// Public so widgets that drive their own time-based repaint (the caret's
/// blink phase) read the *same* clock the animation system does, which is what
/// lets [`test_clock::freeze`] pin their behaviour in a test too.
pub fn now() -> Instant {
    CLOCK_OVERRIDE
        .with(|c| c.get())
        .unwrap_or_else(Instant::now)
}

/// Freeze the animation clock so tests can observe a fade at a chosen point
/// instead of racing a real timer.
///
/// A transition lasting a fixed wall-clock duration (e.g. `Container`'s 120 ms
/// hover fade) makes "assert the fade is still in flight" a race: if the test
/// thread is descheduled past the duration between starting the fade and
/// painting, the animation has already settled and the assertion fails. That
/// passes in isolation and fails under load. Freezing the clock removes real
/// elapsed time from the equation entirely.
///
/// This is public because app authors testing their own widgets hit exactly
/// the same race — nothing in the framework's own runtime touches it.
///
/// ```
/// use std::time::Duration;
/// use shroud_reactive::animation::{Animated, Easing, test_clock};
///
/// let clock = test_clock::freeze();
/// let a = Animated::new(0.0f32, Duration::from_millis(100), Easing::Linear);
/// a.set(10.0);
/// assert!(a.is_animating(), "no time has passed, so it can't have settled");
///
/// clock.advance(Duration::from_millis(50));
/// assert_eq!(a.get(), 5.0, "exactly halfway, every run");
///
/// clock.advance(Duration::from_millis(50));
/// assert!(!a.is_animating());
/// // Dropping `clock` restores the wall clock.
/// ```
pub mod test_clock {
    use super::{CLOCK_OVERRIDE, Duration, Instant};

    /// Holds the animation clock still until dropped, when the previous clock
    /// (normally the wall clock) is restored. Restoring on drop — rather than
    /// leaving the override set — matters because libtest runs tests on the
    /// main thread when `--test-threads=1`, so a leaked freeze would be
    /// inherited by every later test on that thread. Unwinding on a failed
    /// assertion still drops the guard.
    #[must_use = "the clock unfreezes as soon as the guard is dropped"]
    pub struct ClockGuard {
        prev: Option<Instant>,
    }

    impl ClockGuard {
        /// Move the frozen clock forward by `by`. Animations advance exactly
        /// as far as asked — no more, no less, however loaded the machine is.
        pub fn advance(&self, by: Duration) {
            CLOCK_OVERRIDE.with(|c| {
                let frozen = c
                    .get()
                    .expect("clock override cleared while a ClockGuard was alive");
                c.set(Some(frozen + by));
            });
        }

        /// The instant the clock currently reads.
        pub fn now(&self) -> Instant {
            CLOCK_OVERRIDE
                .with(|c| c.get())
                .expect("clock override cleared while a ClockGuard was alive")
        }
    }

    impl Drop for ClockGuard {
        fn drop(&mut self) {
            CLOCK_OVERRIDE.with(|c| c.set(self.prev));
        }
    }

    /// Freeze the clock at the current instant. See the [module
    /// docs](self) for why.
    pub fn freeze() -> ClockGuard {
        freeze_at(Instant::now())
    }

    /// Freeze the clock at a specific instant. Prefer [`freeze`] unless the
    /// test needs to place an animation relative to an `Instant` it already
    /// holds.
    pub fn freeze_at(at: Instant) -> ClockGuard {
        let prev = CLOCK_OVERRIDE.with(|c| c.replace(Some(at)));
        ClockGuard { prev }
    }
}

/// Register that an animation still needs another frame. Called internally by
/// [`Animated::get`] while interpolating; rarely needed directly.
pub fn request_frame() {
    FRAME_REQUESTED.with(|f| f.set(true));
}

/// Ask to be repainted at (no later than) `at`, without pinning the loop at
/// frame rate in the meantime. Only the earliest outstanding deadline is kept,
/// so many voters collapse to one wake. Used by widgets whose repaint is
/// time-driven but sparse — the caret's blink toggle. Prefer this over
/// [`request_frame`] whenever the next repaint is at a *known* future instant
/// rather than "as soon as possible": `request_frame` left set re-votes every
/// frame and never lets the loop sleep.
pub fn request_frame_at(at: Instant) {
    FRAME_DEADLINE.with(|d| {
        let next = match d.get() {
            Some(existing) => existing.min(at),
            None => at,
        };
        d.set(Some(next));
    });
}

/// Clear the per-frame animation votes (both the next-frame flag and any timed
/// wake deadline). The event loop calls this immediately before laying out /
/// painting a frame; anything still in flight re-votes during that paint.
pub fn reset_frame_request() {
    FRAME_REQUESTED.with(|f| f.set(false));
    FRAME_DEADLINE.with(|d| d.set(None));
}

/// Whether any animation voted for another frame since the last
/// [`reset_frame_request`]. The event loop calls this after painting and
/// schedules a redraw when it returns `true`.
pub fn frame_requested() -> bool {
    FRAME_REQUESTED.with(|f| f.get())
}

/// The earliest timed-wake deadline voted via [`request_frame_at`] since the
/// last [`reset_frame_request`], or `None`. The event loop reads this after
/// painting and parks until (at most) this instant, then repaints — giving the
/// caret its blink toggle without a frame-rate busy loop.
pub fn frame_deadline() -> Option<Instant> {
    FRAME_DEADLINE.with(|d| d.get())
}

/// An easing curve mapping linear progress `t ∈ [0, 1]` to eased progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Easing {
    /// Constant rate.
    Linear,
    /// Quadratic ease-in: slow start, fast end.
    EaseIn,
    /// Quadratic ease-out: fast start, slow end.
    EaseOut,
    /// Quadratic ease-in-out: slow at both ends. The default — it reads as
    /// the most natural for UI color/opacity transitions.
    #[default]
    EaseInOut,
}

impl Easing {
    /// Map linear progress to eased progress. Input is clamped to `[0, 1]`;
    /// output endpoints are exactly `0.0` and `1.0`.
    pub fn eval(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => t * (2.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    let u = 2.0 * t - 1.0;
                    0.5 + 0.5 * (u * (2.0 - u))
                }
            }
        }
    }
}

struct AnimState<T> {
    from: T,
    target: T,
    start: Instant,
    duration: Duration,
    easing: Easing,
}

impl<T: Lerp + Clone> AnimState<T> {
    /// Current interpolated value without casting a frame vote. The shared
    /// core of `get` (which votes) and `set` (which needs the live value as
    /// the new `from`).
    fn value(&self) -> T {
        if self.duration.is_zero() {
            return self.target.clone();
        }
        let raw = self.elapsed().as_secs_f32() / self.duration.as_secs_f32();
        if raw >= 1.0 {
            return self.target.clone();
        }
        let t = self.easing.eval(raw);
        self.from.lerp(&self.target, t)
    }

    fn settled(&self) -> bool {
        self.duration.is_zero() || self.elapsed() >= self.duration
    }

    /// Time since `start` on the animation clock. Saturating rather than
    /// `Instant::elapsed` because a test may freeze the clock at an instant
    /// *before* an already-running animation started, which would otherwise
    /// underflow; reading that as "zero elapsed" is the sensible answer.
    fn elapsed(&self) -> Duration {
        now().saturating_duration_since(self.start)
    }
}

/// A value that animates toward a target over time.
///
/// Cheap to clone (`Rc`-backed): clone it into event handlers to retarget it
/// and into widget builders to read it. Construct with [`Animated::new`],
/// retarget with [`Animated::set`], read the eased value with
/// [`Animated::get`] (or via its [`Reactive`] conversion).
pub struct Animated<T> {
    state: Rc<RefCell<AnimState<T>>>,
}

impl<T> Clone for Animated<T> {
    fn clone(&self) -> Self {
        Self {
            state: Rc::clone(&self.state),
        }
    }
}

impl<T: Lerp + Clone> Animated<T> {
    /// Create an animated value resting at `initial`. It starts fully
    /// settled — `get()` returns `initial` and casts no frame vote until the
    /// first [`set`](Self::set).
    ///
    /// `duration` is how long each transition takes; `easing` shapes it. A
    /// zero `duration` makes every `set` instantaneous (still useful to keep
    /// a uniform API where the duration is configuration-driven).
    pub fn new(initial: T, duration: Duration, easing: Easing) -> Self {
        // Backdate `start` by `duration` so the initial state reads as
        // already complete (settled), with no spurious startup vote.
        let start = now().checked_sub(duration).unwrap_or_else(now);
        Self {
            state: Rc::new(RefCell::new(AnimState {
                from: initial.clone(),
                target: initial,
                start,
                duration,
                easing,
            })),
        }
    }

    /// Retarget the animation. Interpolation restarts from the *current*
    /// eased value (so retargeting mid-flight is smooth, never a jump back to
    /// the old `from`) toward `target` over the configured duration.
    pub fn set(&self, target: T) {
        let mut s = self.state.borrow_mut();
        s.from = s.value();
        s.target = target;
        s.start = now();
    }

    /// Jump immediately to `value` with no interpolation — the animation is
    /// left fully settled. Use for resets that must not visibly slide (e.g.
    /// re-clamping a scroll offset after its content shrank, or snapping to a
    /// fresh document), in contrast to [`set`](Self::set), which eases. Casts
    /// no frame vote.
    pub fn snap(&self, value: T) {
        let mut s = self.state.borrow_mut();
        s.from = value.clone();
        s.target = value;
        // Backdate `start` past `duration` so `settled()` holds and `value()`
        // short-circuits to `target`: no interpolation, no vote.
        s.start = now().checked_sub(s.duration).unwrap_or_else(now);
    }

    /// Read the current eased value. While the animation is still in flight
    /// this also votes for another frame (see module docs).
    pub fn get(&self) -> T {
        let s = self.state.borrow();
        if !s.settled() {
            request_frame();
        }
        s.value()
    }

    /// The resting target value, ignoring any in-flight interpolation.
    pub fn target(&self) -> T {
        self.state.borrow().target.clone()
    }

    /// Whether the animation is still interpolating (has not reached its
    /// target yet).
    pub fn is_animating(&self) -> bool {
        !self.state.borrow().settled()
    }
}

/// An `Animated<T>` reads as a `Dynamic` reactive source: each paint pulls
/// the freshest eased value, and the read votes for another frame while
/// in flight. This is what lets `Container::background(animated)` work.
impl<T: Lerp + Clone + 'static> From<Animated<T>> for Reactive<T> {
    fn from(a: Animated<T>) -> Self {
        Reactive::Dynamic(Rc::new(move || a.get()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shroud_core::Color;

    #[test]
    fn easing_endpoints_are_exact() {
        for e in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            assert_eq!(e.eval(0.0), 0.0, "{e:?} at 0");
            assert_eq!(e.eval(1.0), 1.0, "{e:?} at 1");
        }
    }

    #[test]
    fn easing_clamps_out_of_range_input() {
        assert_eq!(Easing::Linear.eval(-1.0), 0.0);
        assert_eq!(Easing::Linear.eval(2.0), 1.0);
    }

    #[test]
    fn easing_inout_is_symmetric_about_midpoint() {
        // f(0.5) == 0.5 and f(t) + f(1-t) == 1 for a symmetric ease.
        let e = Easing::EaseInOut;
        assert!((e.eval(0.5) - 0.5).abs() < 1e-6);
        assert!((e.eval(0.25) + e.eval(0.75) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn new_value_rests_at_initial_and_does_not_vote() {
        reset_frame_request();
        let a = Animated::new(5.0f32, Duration::from_millis(100), Easing::Linear);
        assert_eq!(a.get(), 5.0);
        assert!(!a.is_animating());
        assert!(
            !frame_requested(),
            "a settled animation must not request a frame"
        );
    }

    #[test]
    fn timed_wake_keeps_only_the_earliest_deadline() {
        reset_frame_request();
        assert_eq!(frame_deadline(), None, "reset clears any pending deadline");

        let base = Instant::now();
        let later = base + Duration::from_millis(500);
        let sooner = base + Duration::from_millis(200);

        request_frame_at(later);
        assert_eq!(frame_deadline(), Some(later));

        // A sooner vote wins; a later one does not push the wake out.
        request_frame_at(sooner);
        assert_eq!(frame_deadline(), Some(sooner));
        request_frame_at(later);
        assert_eq!(
            frame_deadline(),
            Some(sooner),
            "a later vote must not delay an already-earlier wake"
        );
    }

    #[test]
    fn timed_wake_is_independent_of_the_next_frame_flag() {
        reset_frame_request();
        // Asking for a timed wake must not also assert "repaint immediately" —
        // that's the whole point of not busy-pumping between blinks.
        request_frame_at(Instant::now() + Duration::from_millis(500));
        assert!(!frame_requested(), "a timed wake is not a next-frame vote");
        assert!(frame_deadline().is_some());

        reset_frame_request();
        assert_eq!(frame_deadline(), None);
    }

    #[test]
    fn set_starts_animation_and_votes_for_a_frame() {
        // Frozen clock: "just after set" means exactly t=0, not "t≈0 if this
        // thread isn't descheduled" — otherwise a stall past the 100ms
        // duration settles the animation and the vote never happens.
        let _clock = test_clock::freeze();
        reset_frame_request();
        let a = Animated::new(0.0f32, Duration::from_millis(100), Easing::Linear);
        a.set(10.0);
        assert_eq!(a.get(), 0.0, "at t=0 the value is still `from`");
        assert!(a.is_animating());
        assert!(frame_requested(), "an in-flight get must request a frame");
    }

    #[test]
    fn frozen_clock_holds_an_animation_mid_flight() {
        let clock = test_clock::freeze();
        let a = Animated::new(0.0f32, Duration::from_millis(100), Easing::Linear);
        a.set(100.0);

        clock.advance(Duration::from_millis(25));
        assert_eq!(a.get(), 25.0, "linear easing, a quarter of the way");
        assert!(a.is_animating());

        clock.advance(Duration::from_millis(75));
        assert_eq!(a.get(), 100.0, "arrived exactly on the target");
        assert!(!a.is_animating(), "the duration has fully elapsed");
    }

    #[test]
    fn frozen_clock_stops_real_time_from_advancing_an_animation() {
        let _clock = test_clock::freeze();
        let a = Animated::new(0.0f32, Duration::from_millis(1), Easing::Linear);
        a.set(1.0);
        // A 1ms animation would settle instantly on the wall clock. Sleeping
        // well past it must change nothing: only `advance` moves the clock.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(a.get(), 0.0);
        assert!(
            a.is_animating(),
            "real time must not advance a frozen clock"
        );
    }

    #[test]
    fn dropping_the_guard_restores_the_wall_clock() {
        let a = {
            let clock = test_clock::freeze();
            let a = Animated::new(0.0f32, Duration::from_millis(1), Easing::Linear);
            a.set(1.0);
            clock.advance(Duration::from_micros(1));
            assert!(a.is_animating(), "frozen mid-flight");
            a
        };
        // The guard is gone, so `elapsed` is measured against the real clock
        // again — and the 1ms duration is long past.
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            !a.is_animating(),
            "unfreezing must let real time settle the animation"
        );
    }

    #[test]
    fn freezing_before_a_running_animation_starts_reads_as_zero_elapsed() {
        // `freeze_at` an instant that predates the animation's `start` makes
        // `now - start` negative; that must saturate to zero rather than
        // panic on `Instant` subtraction overflow.
        let before = Instant::now();
        let a = Animated::new(0.0f32, Duration::from_millis(100), Easing::Linear);
        a.set(10.0);
        let _clock = test_clock::freeze_at(before);
        assert_eq!(a.get(), 0.0);
        assert!(a.is_animating());
    }

    #[test]
    fn snap_jumps_without_animating_or_voting() {
        reset_frame_request();
        let a = Animated::new(0.0f32, Duration::from_secs(10), Easing::Linear);
        a.set(100.0); // begin a long animation
        assert!(a.is_animating());
        a.snap(42.0);
        assert_eq!(a.get(), 42.0, "snap lands exactly on the value");
        assert!(!a.is_animating(), "snap leaves the animation settled");
        assert!(!frame_requested(), "a snapped animation casts no vote");
    }

    #[test]
    fn zero_duration_is_instant() {
        reset_frame_request();
        let a = Animated::new(0.0f32, Duration::ZERO, Easing::Linear);
        a.set(42.0);
        assert_eq!(a.get(), 42.0);
        assert!(!a.is_animating());
        assert!(!frame_requested());
    }

    #[test]
    fn target_reports_destination_mid_flight() {
        let a = Animated::new(0.0f32, Duration::from_secs(10), Easing::Linear);
        a.set(100.0);
        assert_eq!(
            a.target(),
            100.0,
            "target is the destination, not the eased value"
        );
        assert!(a.get() < 100.0, "eased value hasn't arrived yet");
    }

    #[test]
    fn settled_animation_returns_exact_target() {
        // A finished animation reads its target exactly (no float drift from
        // interpolation), because `value()` short-circuits once raw >= 1.
        let a = Animated::new(Color::BLACK, Duration::ZERO, Easing::EaseInOut);
        a.set(Color::WHITE);
        assert_eq!(a.get(), Color::WHITE);
    }

    #[test]
    fn converts_into_reactive_and_reads_through_it() {
        let a = Animated::new(3.0f32, Duration::ZERO, Easing::Linear);
        let r: Reactive<f32> = a.clone().into();
        a.set(7.0);
        assert_eq!(r.get(), 7.0, "reactive view reflects the animated value");
    }
}
