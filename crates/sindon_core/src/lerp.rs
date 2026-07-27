//! Linear interpolation for animatable values.
//!
//! [`Lerp`] is the "what can be tweened" half of the animation system; the
//! time-driven [`Animated`](../../sindon_reactive/animation/struct.Animated.html)
//! value lives in `sindon_reactive` and calls `lerp` each frame with an
//! eased progress `t`. Kept here in `sindon_core` because the concrete
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
        // Interpolate in *premultiplied*-alpha space, then un-premultiply.
        //
        // A straight per-channel RGBA lerp goes wrong whenever the two
        // endpoints differ in alpha — most visibly fading a transparent color
        // (`Color::TRANSPARENT` is `rgba(0,0,0,0)`, i.e. *black* at alpha 0)
        // toward an opaque one, as the hover transition does. Straight lerp
        // drags RGB up from black while alpha is still low; the renderer's
        // straight-alpha "over" blend (`src.rgb*src.a + dst*(1-src.a)`) then
        // loses energy to black around the midpoint, so the pixel darkens
        // before it brightens — a visible black flash. Premultiplied
        // interpolation keeps the color's hue steady and just fades its
        // coverage, which is what compositing actually wants.
        //
        // When both endpoints are opaque (every theme color), the alpha factor
        // is 1 throughout and this reduces to the old straight lerp, so theme
        // cross-fades are unchanged.
        let a = self.a.lerp(&to.a, t);
        let pr = (self.r * self.a).lerp(&(to.r * to.a), t);
        let pg = (self.g * self.a).lerp(&(to.g * to.a), t);
        let pb = (self.b * self.a).lerp(&(to.b * to.a), t);
        if a <= f32::EPSILON {
            // Fully transparent result — its RGB is invisible, and dividing by
            // ~0 alpha would blow up, so return clean transparent black.
            Color::rgba(0.0, 0.0, 0.0, 0.0)
        } else {
            Color::rgba(pr / a, pg / a, pb / a, a)
        }
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
    fn opaque_colors_interpolate_per_channel() {
        // With both endpoints opaque the premultiplied path collapses to a
        // plain per-channel lerp (alpha factor is 1 throughout) — this is the
        // theme cross-fade case, which must be unchanged.
        let a = Color::rgba(0.0, 0.0, 0.0, 1.0);
        let b = Color::rgba(1.0, 0.5, 0.25, 1.0);
        let mid = a.lerp(&b, 0.5);
        assert_eq!(mid, Color::rgba(0.5, 0.25, 0.125, 1.0));
    }

    #[test]
    fn fade_from_transparent_keeps_hue_and_only_lowers_alpha() {
        // The hover-transition case: TRANSPARENT (black at alpha 0) → an opaque
        // color. Premultiplied interpolation must hold the destination hue and
        // only ramp coverage — at the midpoint the RGB is the full hover color
        // at half alpha, NOT a half-bright (darkened) color. This is the fix
        // for the "black flash before it brightens" sidebar-hover bug.
        let transparent = Color::rgba(0.0, 0.0, 0.0, 0.0);
        let hover = Color::rgba(1.0, 0.5, 0.25, 1.0);
        let mid = transparent.lerp(&hover, 0.5);
        assert_eq!(mid, Color::rgba(1.0, 0.5, 0.25, 0.5));
    }

    #[test]
    fn color_endpoints_are_exact() {
        // Opaque endpoints (the common case: theme colors) round-trip exactly —
        // the un-premultiply divides by alpha 1, which is lossless.
        let a = Color::rgb(0.2, 0.4, 0.6);
        let b = Color::rgb(0.8, 0.6, 0.4);
        assert_eq!(a.lerp(&b, 0.0), a);
        assert_eq!(a.lerp(&b, 1.0), b);
    }

    #[test]
    fn fully_transparent_both_ends_stays_transparent() {
        // Two different "colors" at alpha 0 are both invisible; the result is
        // clean transparent black (no divide-by-zero, no NaN leaking through).
        let a = Color::rgba(1.0, 0.0, 0.0, 0.0);
        let b = Color::rgba(0.0, 0.0, 1.0, 0.0);
        assert_eq!(a.lerp(&b, 0.5), Color::rgba(0.0, 0.0, 0.0, 0.0));
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
