//! Linear interpolation for animatable values.
//!
//! [`Lerp`] is the "what can be tweened" half of the animation system; the
//! time-driven [`Animated`](../../shroud_reactive/animation/struct.Animated.html)
//! value lives in `shroud_reactive` and calls `lerp` each frame with an
//! eased progress `t`. Kept here in `shroud_core` because the concrete
//! impls (`Color`, `Point`) are core geometry types and the trait should
//! sit below the reactive layer that consumes it.

use crate::geometry::{Color, Point};

/// A value that can be linearly interpolated toward another of the same
/// type. `t` is the (already-eased) progress in `[0.0, 1.0]`: `t == 0`
/// returns `self`, `t == 1` returns `to`.
///
/// Implementors should interpolate component-wise and must not clamp `t`
/// themselves — the caller owns easing and clamping, so an implementor that
/// re-clamped would break overshoot easings added later.
pub trait Lerp {
    /// Interpolate from `self` toward `to` by progress `t`.
    fn lerp(&self, to: &Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        self + (to - self) * t
    }
}

impl Lerp for Color {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Color::rgba(
            self.r.lerp(&to.r, t),
            self.g.lerp(&to.g, t),
            self.b.lerp(&to.b, t),
            self.a.lerp(&to.a, t),
        )
    }
}

impl Lerp for Point {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Point::new(self.x.lerp(&to.x, t), self.y.lerp(&to.y, t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_endpoints_and_midpoint() {
        assert_eq!(0.0f32.lerp(&10.0, 0.0), 0.0);
        assert_eq!(0.0f32.lerp(&10.0, 1.0), 10.0);
        assert_eq!(0.0f32.lerp(&10.0, 0.5), 5.0);
    }

    #[test]
    fn color_interpolates_each_channel_including_alpha() {
        let a = Color::rgba(0.0, 0.0, 0.0, 0.0);
        let b = Color::rgba(1.0, 0.5, 0.25, 1.0);
        let mid = a.lerp(&b, 0.5);
        assert_eq!(mid, Color::rgba(0.5, 0.25, 0.125, 0.5));
    }

    #[test]
    fn color_endpoints_are_exact() {
        let a = Color::rgb(0.2, 0.4, 0.6);
        let b = Color::rgb(0.8, 0.6, 0.4);
        assert_eq!(a.lerp(&b, 0.0), a);
        assert_eq!(a.lerp(&b, 1.0), b);
    }

    #[test]
    fn point_interpolates_both_axes() {
        let a = Point::new(0.0, 10.0);
        let b = Point::new(10.0, 0.0);
        assert_eq!(a.lerp(&b, 0.5), Point::new(5.0, 5.0));
    }

    #[test]
    fn t_is_not_reclamped_by_impls() {
        // Easing owns clamping; an impl must extrapolate honestly so future
        // overshoot easings (t > 1) work. 0->10 at t=1.5 must reach 15.
        assert_eq!(0.0f32.lerp(&10.0, 1.5), 15.0);
    }
}
