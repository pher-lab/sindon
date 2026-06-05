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
}

/// Register that an animation still needs another frame. Called internally by
/// [`Animated::get`] while interpolating; rarely needed directly.
pub fn request_frame() {
    FRAME_REQUESTED.with(|f| f.set(true));
}

/// Clear the per-frame animation vote. The event loop calls this immediately
/// before laying out / painting a frame.
pub fn reset_frame_request() {
    FRAME_REQUESTED.with(|f| f.set(false));
}

/// Whether any animation voted for another frame since the last
/// [`reset_frame_request`]. The event loop calls this after painting and
/// schedules a redraw when it returns `true`.
pub fn frame_requested() -> bool {
    FRAME_REQUESTED.with(|f| f.get())
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
        let raw = self.start.elapsed().as_secs_f32() / self.duration.as_secs_f32();
        if raw >= 1.0 {
            return self.target.clone();
        }
        let t = self.easing.eval(raw);
        self.from.lerp(&self.target, t)
    }

    fn settled(&self) -> bool {
        self.duration.is_zero() || self.start.elapsed() >= self.duration
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
        let start = Instant::now()
            .checked_sub(duration)
            .unwrap_or_else(Instant::now);
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
        s.start = Instant::now();
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
    fn set_starts_animation_and_votes_for_a_frame() {
        reset_frame_request();
        let a = Animated::new(0.0f32, Duration::from_millis(100), Easing::Linear);
        a.set(10.0);
        // Immediately after set, we're at t≈0 → value near `from`, still
        // animating, and the read must have voted.
        let v = a.get();
        assert!((0.0..2.0).contains(&v), "value just after set was {v}");
        assert!(a.is_animating());
        assert!(frame_requested(), "an in-flight get must request a frame");
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
