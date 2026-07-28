//! Progress indicators — a [`ProgressBar`] and a circular [`Spinner`].
//!
//! Two shapes of the same idea: tell the user work is happening. A
//! [`ProgressBar`] is *determinate* when it knows how far along it is (a
//! download's byte count, an import's row count) and *indeterminate* when it
//! only knows work is ongoing; a [`Spinner`] is always indeterminate — the
//! compact "busy" glyph for a button or a small inline slot.
//!
//! # Driving the animation
//!
//! The indeterminate variants animate off the shared animation clock: each
//! paint reads a monotonic phase and votes for the next frame via
//! [`request_frame`](sindon_reactive::animation::request_frame), exactly as
//! [`Animated`](sindon_reactive::animation::Animated) does while in flight. That
//! means an indeterminate indicator repaints continuously *only while it is on
//! screen* — hide it (`display: none`) or remove it from the tree and the votes
//! stop with it, so the event loop returns to idle rather than busy-pumping
//! forever. A determinate bar casts no vote of its own; it repaints when its
//! bound value changes, like any other reactive-driven widget.
//!
//! # Accessibility
//!
//! Both map to [`AccessRole::ProgressIndicator`]. A determinate bar reports a
//! numeric range (`0..1`, `now = fraction`) so a screen reader can announce the
//! percentage; an indeterminate bar or spinner reports no value (announced as
//! "busy"). Neither is operable — a progress indicator is the read-only
//! counterpart of a [`Slider`](crate::Slider), so it advertises none of the
//! value-setting actions.

use std::cell::Cell;
use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use sindon_core::{AccessNode, AccessRange, AccessRole, Color, Rect, Size};
use sindon_layout::FlexStyle;
use sindon_reactive::Reactive;
use sindon_reactive::animation::{self, request_frame};

thread_local! {
    /// The instant the process's indeterminate animations are measured from,
    /// captured lazily on first use off the animation clock. Shared by every
    /// indeterminate indicator so they stay phase-locked to one another, and
    /// stable across tree rebuilds (a rebuilt spinner doesn't jump back to phase
    /// zero). Thread-local because the UI runs single-threaded on the event-loop
    /// thread, like the animation frame vote itself.
    static ANIM_EPOCH: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn anim_epoch() -> Instant {
    ANIM_EPOCH.with(|c| {
        c.get().unwrap_or_else(|| {
            let t = animation::now();
            c.set(Some(t));
            t
        })
    })
}

/// A monotonic phase in `[0.0, 1.0)` for one `period`, read off the animation
/// clock, plus a vote for the next frame so the loop keeps repainting while an
/// indeterminate indicator is visible. A zero (or negative) period is a fixed
/// phase of `0.0`, never a divide-by-zero.
fn running_phase(period: Duration) -> f32 {
    request_frame();
    let p = period.as_secs_f32();
    if p <= 0.0 {
        return 0.0;
    }
    let elapsed = animation::now().saturating_duration_since(anim_epoch());
    (elapsed.as_secs_f32() / p).fract()
}

/// Default bar thickness (also its intrinsic height, so a stretch parent only
/// widens it).
const BAR_THICKNESS: f32 = 6.0;
/// Intrinsic min width so a bar in a hug (non-stretch) context is still legible;
/// a stretch parent (a card column) overrides it to full width.
const BAR_MIN_WIDTH: f32 = 140.0;
/// The sweeping segment's width, as a fraction of the track, for the
/// indeterminate bar.
const BAR_SEGMENT: f32 = 0.35;
/// One full left-to-right sweep of the indeterminate segment.
const BAR_PERIOD: Duration = Duration::from_millis(1150);

/// A horizontal progress bar — determinate ([`new`](ProgressBar::new)) or
/// indeterminate ([`indeterminate`](ProgressBar::indeterminate)).
///
/// # Example (conceptual)
/// ```
/// # use sindon_reactive::Signal;
/// # use sindon_widgets::ProgressBar;
/// // Determinate, driven by a signal in `[0, 1]`:
/// let done = Signal::new(0.0);
/// let bar = ProgressBar::new(done).label("Importing");
///
/// // Indeterminate — work of unknown duration:
/// let busy = ProgressBar::indeterminate();
/// ```
pub struct ProgressBar {
    /// `Some` → determinate: a reactive fraction in `[0, 1]` (clamped on read).
    /// `None` → indeterminate: a segment sweeps the track, no value reported.
    value: Option<Reactive<f32>>,
    thickness: f32,
    track_color: Option<Color>,
    fill_color: Option<Color>,
    /// Accessible name announced by a screen reader (e.g. "Uploading"). `None`
    /// leaves the node named only by its role.
    label: Option<String>,
}

impl ProgressBar {
    /// A determinate bar showing `value`, a fraction in `[0, 1]` (values outside
    /// the range are clamped). Accepts a constant, a [`Signal<f32>`], or any
    /// other [`Reactive<f32>`] source, read fresh each paint.
    ///
    /// [`Signal<f32>`]: sindon_reactive::Signal
    pub fn new(value: impl Into<Reactive<f32>>) -> Self {
        Self {
            value: Some(value.into()),
            thickness: BAR_THICKNESS,
            track_color: None,
            fill_color: None,
            label: None,
        }
    }

    /// An indeterminate bar — a segment sweeps the track to signal ongoing work
    /// of unknown duration. Animates continuously while visible (see the module
    /// docs); reports no numeric value to assistive technology.
    pub fn indeterminate() -> Self {
        Self {
            value: None,
            thickness: BAR_THICKNESS,
            track_color: None,
            fill_color: None,
            label: None,
        }
    }

    /// Override the bar thickness (and thus its intrinsic height). Non-positive
    /// values are ignored.
    pub fn thickness(mut self, px: f32) -> Self {
        if px > 0.0 {
            self.thickness = px;
        }
        self
    }

    /// Override the track (groove) colour. `None` reads
    /// `theme.colors.surface_variant`.
    pub fn track_color(mut self, color: Color) -> Self {
        self.track_color = Some(color);
        self
    }

    /// Override the filled / sweeping colour. `None` reads
    /// `theme.colors.primary`.
    pub fn fill_color(mut self, color: Color) -> Self {
        self.fill_color = Some(color);
        self
    }

    /// Set the accessible name a screen reader announces alongside the role /
    /// percentage (e.g. "Uploading photos").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The current fraction in `[0, 1]` when determinate, or `None` when
    /// indeterminate.
    fn fraction(&self) -> Option<f32> {
        self.value.as_ref().map(|v| v.get().clamp(0.0, 1.0))
    }
}

impl Widget for ProgressBar {
    fn accessibility(&self) -> Option<AccessNode> {
        let mut node = AccessNode::new(AccessRole::ProgressIndicator);
        if let Some(label) = &self.label {
            node = node.name(label.clone());
        }
        // A determinate bar reports its position; an indeterminate one reports
        // no value at all (screen readers announce it as "busy").
        if let Some(f) = self.fraction() {
            node = node.numeric(AccessRange {
                min: 0.0,
                max: 1.0,
                now: f as f64,
            });
        }
        Some(node)
    }

    fn style(&self) -> FlexStyle {
        // Measured leaf — no `min_size` here (see `Slider` / the `Button` note).
        FlexStyle::new()
    }

    fn measure(&self, _available_width: Option<f32>, _ctx: &mut MeasureContext) -> Option<Size> {
        Some(Size::new(BAR_MIN_WIDTH, self.thickness))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let track = self.track_color.unwrap_or(ctx.theme.colors.surface_variant);
        let fill = self.fill_color.unwrap_or(ctx.theme.colors.primary);

        let width = layout.size.width.max(0.0);
        let radius = self.thickness / 2.0;
        let track_y = layout.origin.y + (layout.size.height - self.thickness) / 2.0;
        let track_rect = Rect::new(layout.origin.x, track_y, width, self.thickness);

        // Groove first, then the filled / sweeping portion on top.
        ctx.fill_rect_rounded(track_rect, track, radius);

        match self.fraction() {
            // Determinate: fill from the left edge up to the fraction.
            Some(f) => {
                let fill_w = width * f;
                if fill_w > 0.0 {
                    ctx.fill_rect_rounded(
                        Rect::new(layout.origin.x, track_y, fill_w, self.thickness),
                        fill,
                        radius,
                    );
                }
            }
            // Indeterminate: a segment sweeps left→right, clipped to the track so
            // its ends stay inside the groove.
            None => {
                let phase = running_phase(BAR_PERIOD);
                let seg_w = width * BAR_SEGMENT;
                let travel = width + seg_w;
                let x = layout.origin.x - seg_w + phase * travel;
                ctx.push_clip(track_rect);
                ctx.fill_rect_rounded(Rect::new(x, track_y, seg_w, self.thickness), fill, radius);
                ctx.pop_clip();
            }
        }
    }
}

/// Default spinner diameter in pixels.
const SPINNER_SIZE: f32 = 24.0;
/// Number of dots around the ring.
const SPINNER_DOTS: usize = 8;
/// One full revolution of the bright "head".
const SPINNER_PERIOD: Duration = Duration::from_millis(900);
/// The faintest a trailing dot fades to (the head is full opacity).
const SPINNER_MIN_ALPHA: f32 = 0.15;
/// Dot radius as a fraction of the spinner diameter.
const SPINNER_DOT_FRAC: f32 = 0.11;
/// Radius of the ring the dot *centres* sit on, as a fraction of the diameter —
/// under half, to leave room for the dots themselves inside the box.
const SPINNER_RING_FRAC: f32 = 0.36;

/// A circular indeterminate spinner — eight dots around a ring with a bright
/// head that rotates, trailing a comet tail of fading dots.
///
/// Built entirely from filled discs (no arc / path primitive needed), so it
/// rides on the same rounded-rect fill every other widget uses. Animates
/// continuously while visible (see the module docs).
///
/// # Example (conceptual)
/// ```
/// # use sindon_widgets::Spinner;
/// let inline = Spinner::new().size(18.0);             // inline, in a button
/// let named = Spinner::new().label("Loading vault");  // named for a screen reader
/// ```
pub struct Spinner {
    size: f32,
    color: Option<Color>,
    /// Accessible name announced by a screen reader. `None` leaves the node
    /// named only by its role ("busy").
    label: Option<String>,
}

impl Spinner {
    /// A spinner at the default diameter (`SPINNER_SIZE`).
    pub fn new() -> Self {
        Self {
            size: SPINNER_SIZE,
            color: None,
            label: None,
        }
    }

    /// Set the diameter in pixels (its intrinsic square size). Non-positive
    /// values are ignored.
    pub fn size(mut self, px: f32) -> Self {
        if px > 0.0 {
            self.size = px;
        }
        self
    }

    /// Override the dot colour. `None` reads `theme.colors.primary`.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Set the accessible name a screen reader announces (e.g. "Loading").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Spinner {
    fn accessibility(&self) -> Option<AccessNode> {
        // Indeterminate: a role, an optional name, and no numeric value — a
        // screen reader announces it as busy.
        let mut node = AccessNode::new(AccessRole::ProgressIndicator);
        if let Some(label) = &self.label {
            node = node.name(label.clone());
        }
        Some(node)
    }

    fn style(&self) -> FlexStyle {
        FlexStyle::new()
    }

    fn measure(&self, _available_width: Option<f32>, _ctx: &mut MeasureContext) -> Option<Size> {
        Some(Size::new(self.size, self.size))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let base = self.color.unwrap_or(ctx.theme.colors.primary);
        let phase = running_phase(SPINNER_PERIOD);

        // Derive the geometry from the actual laid-out box (min side), so a
        // stretched or squashed slot stays a centred circle rather than an
        // ellipse.
        let diameter = layout.size.width.min(layout.size.height).max(0.0);
        let cx = layout.origin.x + layout.size.width / 2.0;
        let cy = layout.origin.y + layout.size.height / 2.0;
        let ring_r = diameter * SPINNER_RING_FRAC;
        let dot_r = diameter * SPINNER_DOT_FRAC;

        for i in 0..SPINNER_DOTS {
            let a = i as f32 / SPINNER_DOTS as f32;
            let angle = a * TAU;
            let dot_cx = cx + angle.cos() * ring_r;
            let dot_cy = cy + angle.sin() * ring_r;

            // How far *behind* the rotating head this dot is, as a fraction of a
            // revolution: 0 at the head (brightest), approaching 1 at the tail
            // (faintest). The head advances with `phase`, so the bright dot
            // appears to sweep around the ring.
            let behind = (phase - a).rem_euclid(1.0);
            let alpha = SPINNER_MIN_ALPHA + (1.0 - SPINNER_MIN_ALPHA) * (1.0 - behind);
            let color = Color::rgba(base.r, base.g, base.b, base.a * alpha);

            ctx.fill_rect_rounded(
                Rect::new(dot_cx - dot_r, dot_cy - dot_r, dot_r * 2.0, dot_r * 2.0),
                color,
                dot_r,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sindon_reactive::Signal;

    fn measure_ctx_size(w: &dyn Widget) -> Size {
        let mut engine = sindon_text::TextEngine::new();
        let theme = sindon_core::Theme::default();
        let mut ctx = MeasureContext::new(&mut engine, &theme);
        w.measure(None, &mut ctx)
            .expect("progress widgets are sized")
    }

    #[test]
    fn determinate_bar_reports_clamped_fraction_to_a11y() {
        let bar = ProgressBar::new(0.6);
        let node = bar.accessibility().expect("a progress bar has a11y");
        let range = node
            .numeric
            .expect("a determinate bar reports a numeric range");
        assert_eq!((range.min, range.max), (0.0, 1.0));
        assert!((range.now - 0.6).abs() < 1e-6);
    }

    #[test]
    fn determinate_fraction_is_clamped_both_ways() {
        assert_eq!(
            ProgressBar::new(-0.5).fraction(),
            Some(0.0),
            "below 0 clamps"
        );
        assert_eq!(
            ProgressBar::new(1.5).fraction(),
            Some(1.0),
            "above 1 clamps"
        );
    }

    #[test]
    fn bound_signal_is_read_fresh() {
        let sig = Signal::new(0.2f32);
        let bar = ProgressBar::new(sig);
        assert_eq!(bar.fraction(), Some(0.2));
        sig.set(0.9);
        assert_eq!(
            bar.fraction(),
            Some(0.9),
            "a later signal write is reflected"
        );
    }

    #[test]
    fn indeterminate_bar_reports_no_value() {
        let bar = ProgressBar::indeterminate();
        assert_eq!(bar.fraction(), None);
        let node = bar.accessibility().expect("still exposes a busy node");
        assert!(
            node.numeric.is_none(),
            "an indeterminate bar reports no numeric value"
        );
    }

    #[test]
    fn spinner_and_bar_are_progress_indicators() {
        assert_eq!(
            Spinner::new().accessibility().unwrap().role,
            AccessRole::ProgressIndicator
        );
        assert_eq!(
            ProgressBar::indeterminate().accessibility().unwrap().role,
            AccessRole::ProgressIndicator
        );
    }

    #[test]
    fn label_maps_to_the_accessible_name() {
        let node = Spinner::new().label("Loading").accessibility().unwrap();
        assert_eq!(node.name.as_deref(), Some("Loading"));
    }

    #[test]
    fn intrinsic_sizes_track_the_builders() {
        assert_eq!(
            measure_ctx_size(&Spinner::new().size(40.0)),
            Size::new(40.0, 40.0)
        );
        assert_eq!(
            measure_ctx_size(&ProgressBar::indeterminate().thickness(10.0)),
            Size::new(BAR_MIN_WIDTH, 10.0),
        );
    }

    #[test]
    fn non_positive_size_and_thickness_are_ignored() {
        // A degenerate builder value must not poison the intrinsic size.
        assert_eq!(
            measure_ctx_size(&Spinner::new().size(0.0)).width,
            SPINNER_SIZE
        );
        assert_eq!(
            measure_ctx_size(&ProgressBar::indeterminate().thickness(-4.0)).height,
            BAR_THICKNESS,
        );
    }

    #[test]
    fn running_phase_wraps_in_unit_interval() {
        // Pure timing helper: whatever the elapsed time, the phase stays in
        // [0, 1), and a zero period is a fixed 0 rather than a divide-by-zero.
        let _clock = animation::test_clock::freeze();
        for _ in 0..4 {
            let p = running_phase(BAR_PERIOD);
            assert!((0.0..1.0).contains(&p), "phase {p} out of [0,1)");
        }
        assert_eq!(running_phase(Duration::ZERO), 0.0, "zero period is fixed");
    }

    #[test]
    fn determinate_paint_emits_track_then_fill() {
        // A determinate bar draws two rects: the groove and the fill. A
        // half-full bar's fill is about half the track width.
        let mut ctx = PaintContext::default();
        let bar = ProgressBar::new(0.5);
        bar.paint(Rect::new(0.0, 0.0, 200.0, 6.0), &mut ctx);
        assert_eq!(ctx.rects.len(), 2, "track + fill");
        let track_w = ctx.rects[0].width;
        let fill_w = ctx.rects[1].width;
        assert!((track_w - 200.0).abs() < 1e-3);
        assert!((fill_w - 100.0).abs() < 1e-3, "half full ≈ half the track");
    }

    #[test]
    fn zero_progress_paints_only_the_track() {
        // A 0% bar has no fill rect at all — nothing to draw with zero width.
        let mut ctx = PaintContext::default();
        ProgressBar::new(0.0).paint(Rect::new(0.0, 0.0, 200.0, 6.0), &mut ctx);
        assert_eq!(ctx.rects.len(), 1, "only the track");
    }

    #[test]
    fn spinner_paints_one_disc_per_dot() {
        let mut ctx = PaintContext::default();
        Spinner::new().paint(Rect::new(0.0, 0.0, 24.0, 24.0), &mut ctx);
        assert_eq!(ctx.rects.len(), SPINNER_DOTS, "one disc per dot");
        // Every disc is a full circle (radius = half its side).
        for r in &ctx.rects {
            assert!((r.radius - r.width / 2.0).abs() < 1e-3, "dots are discs");
        }
    }
}
