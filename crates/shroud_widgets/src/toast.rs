//! Toasts — transient, self-dismissing status messages stacked over the UI.
//!
//! A toast is the framework-owned version of the banner an app would otherwise
//! hand-build (knot's `notice`): show a short message, let it fade in, hold, and
//! fade out on its own. Multiple toasts stack; each carries a severity
//! ([`ToastKind`]) shown as a colored accent stripe.
//!
//! # Shape
//!
//! - A process-global queue (thread-local, like the animation frame vote and the
//!   caret-blink policy) holds the active toasts. [`show`] / [`info`] /
//!   [`success`] / [`warning`] / [`error`] push onto it from anywhere on the UI
//!   thread — an event handler, a frame tick, wherever — and request a repaint.
//! - A single [`ToastHost`] widget, mounted once via [`mount`] as a
//!   **click-through** overlay layer, reads that queue each paint and renders the
//!   stack. Being click-through ([`LayerOptions::tooltip`]), it never steals
//!   input: the app underneath stays fully interactive while toasts are visible.
//! - Dismissal is time-driven off the shared animation clock. While a toast is
//!   fading the host votes [`request_frame`] for smooth animation; while all are
//!   just holding it votes a single [`request_frame_at`] for the next transition,
//!   so an idle screen with a resting toast doesn't busy-pump — the same
//!   discipline as the blinking caret.
//!
//! # Not (yet) interactive
//!
//! Because the host is click-through, a toast currently has no clickable action
//! or close button — it auto-dismisses (or is dismissed programmatically via
//! [`dismiss`]). A Snackbar-style action ("Undo") would need a layer that is
//! interactive only within the toast's own rect, which the all-or-nothing layer
//! model doesn't express yet; that's a deliberate follow-up, not an oversight.
//!
//! [`request_frame`]: shroud_reactive::animation::request_frame
//! [`request_frame_at`]: shroud_reactive::animation::request_frame_at
//! [`LayerOptions::tooltip`]: crate::LayerOptions::tooltip

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use crate::layer::{HAlign, LayerAnchor, LayerOptions, VAlign};
use crate::paint::PaintContext;
use crate::tree::WidgetTree;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Color, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::animation::{self, request_frame, request_frame_at};
use shroud_text::TextAttrs;

/// How long a toast is fully opaque before it begins to fade, by default.
pub const DEFAULT_HOLD: Duration = Duration::from_millis(4000);
/// Fade-in ramp.
const FADE_IN: Duration = Duration::from_millis(180);
/// Fade-out ramp.
const FADE_OUT: Duration = Duration::from_millis(280);
/// Most toasts kept alive at once; a push past this drops the oldest so a flood
/// can't march off the top of the screen.
const MAX_TOASTS: usize = 5;

// ── Card geometry ────────────────────────────────────────────────────────────
const WIDTH: f32 = 340.0;
const PAD_X: f32 = 14.0;
const PAD_Y: f32 = 11.0;
const GAP: f32 = 10.0;
const ACCENT_W: f32 = 4.0;
/// Inset of the accent pill from the card's top/left/bottom edges — keeps it
/// clear of the card's rounded corners (shroud rounds all four corners
/// uniformly, so a flush stripe would poke out at the corners).
const ACCENT_INSET: f32 = 8.0;
/// Gap between the accent pill and the message text.
const ACCENT_GAP: f32 = 10.0;
const RADIUS: f32 = 8.0;
const SHADOW_BLUR: f32 = 18.0;

/// Severity of a toast, shown as a colored accent stripe down its leading edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastKind {
    /// Neutral information — the accent reads `theme.colors.primary`.
    #[default]
    Info,
    /// A completed action — `theme.colors.success`.
    Success,
    /// A caution — `theme.colors.warning`.
    Warning,
    /// A failure — `theme.colors.error`.
    Error,
}

impl ToastKind {
    /// The accent color for this kind, read from the active theme.
    fn accent(self, theme: &shroud_core::Theme) -> Color {
        match self {
            ToastKind::Info => theme.colors.primary,
            ToastKind::Success => theme.colors.success,
            ToastKind::Warning => theme.colors.warning,
            ToastKind::Error => theme.colors.error,
        }
    }
}

/// A handle to a live toast, returned by [`show`] and friends so it can be
/// [`dismiss`]ed early.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToastId(u64);

struct ActiveToast {
    id: ToastId,
    message: String,
    kind: ToastKind,
    created: Instant,
    hold: Duration,
    /// Set by [`dismiss`] to bring the fade-out forward to that instant.
    dismissed: Option<Instant>,
}

thread_local! {
    static TOASTS: RefCell<Vec<ActiveToast>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

fn next_id() -> ToastId {
    NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id.wrapping_add(1));
        ToastId(id)
    })
}

/// Show `message` as a default [`Info`](ToastKind::Info) toast held for
/// [`DEFAULT_HOLD`]. Returns its [`ToastId`].
pub fn show(message: impl Into<String>) -> ToastId {
    push(message.into(), ToastKind::Info, DEFAULT_HOLD)
}

/// Show an [`Info`](ToastKind::Info) toast.
pub fn info(message: impl Into<String>) -> ToastId {
    push(message.into(), ToastKind::Info, DEFAULT_HOLD)
}

/// Show a [`Success`](ToastKind::Success) toast.
pub fn success(message: impl Into<String>) -> ToastId {
    push(message.into(), ToastKind::Success, DEFAULT_HOLD)
}

/// Show a [`Warning`](ToastKind::Warning) toast.
pub fn warning(message: impl Into<String>) -> ToastId {
    push(message.into(), ToastKind::Warning, DEFAULT_HOLD)
}

/// Show an [`Error`](ToastKind::Error) toast.
pub fn error(message: impl Into<String>) -> ToastId {
    push(message.into(), ToastKind::Error, DEFAULT_HOLD)
}

/// Show a toast with an explicit kind and hold duration. A zero `hold` still
/// fades in and out — it just doesn't linger.
pub fn show_for(message: impl Into<String>, kind: ToastKind, hold: Duration) -> ToastId {
    push(message.into(), kind, hold)
}

fn push(message: String, kind: ToastKind, hold: Duration) -> ToastId {
    let id = next_id();
    TOASTS.with(|t| {
        let mut list = t.borrow_mut();
        list.push(ActiveToast {
            id,
            message,
            kind,
            created: animation::now(),
            hold,
            dismissed: None,
        });
        // Cap the backlog: drop the oldest still-holding toast(s) so a burst
        // can't overflow the screen. Removing outright (rather than fading) is
        // fine — the ones dropped are the least recent and already on their way
        // out perceptually.
        while list.len() > MAX_TOASTS {
            list.remove(0);
        }
    });
    // Wake the loop so the new toast paints even if nothing else is animating.
    request_frame();
    id
}

/// Begin dismissing the toast with `id` now — it fades out from wherever it is
/// rather than waiting out its hold. No-op if it's already gone.
pub fn dismiss(id: ToastId) {
    TOASTS.with(|t| {
        if let Some(toast) = t.borrow_mut().iter_mut().find(|x| x.id == id) {
            if toast.dismissed.is_none() {
                toast.dismissed = Some(animation::now());
            }
        }
    });
    request_frame();
}

/// Dismiss every active toast (each fades out).
pub fn clear() {
    let now = animation::now();
    TOASTS.with(|t| {
        for toast in t.borrow_mut().iter_mut() {
            toast.dismissed.get_or_insert(now);
        }
    });
    request_frame();
}

/// The number of toasts currently alive (including fading ones). Mostly for
/// tests and diagnostics.
pub fn active_count() -> usize {
    TOASTS.with(|t| t.borrow().len())
}

/// Which part of its lifetime a toast is in — drives the frame-vote choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    FadingIn,
    Holding,
    FadingOut,
}

struct Render {
    alpha: f32,
    phase: Phase,
    /// The instant the hold ends and fade-out begins (a timed-wake target while
    /// holding).
    fade_out_start: Instant,
}

/// Pure lifetime math: given when a toast was created, its hold, any early
/// dismissal, and the current instant, return its opacity and phase — or `None`
/// once it has fully faded out and should be removed.
fn render_at(
    created: Instant,
    hold: Duration,
    dismissed: Option<Instant>,
    now: Instant,
) -> Option<Render> {
    let fade_in_end = created + FADE_IN;
    let fade_out_start = dismissed
        .map(|d| d.max(created))
        .unwrap_or(fade_in_end + hold);
    let gone = fade_out_start + FADE_OUT;

    if now >= gone {
        return None;
    }
    let render = if now >= fade_out_start {
        let t =
            now.saturating_duration_since(fade_out_start).as_secs_f32() / FADE_OUT.as_secs_f32();
        Render {
            alpha: (1.0 - t).clamp(0.0, 1.0),
            phase: Phase::FadingOut,
            fade_out_start,
        }
    } else if now < fade_in_end {
        let t = now.saturating_duration_since(created).as_secs_f32() / FADE_IN.as_secs_f32();
        Render {
            alpha: t.clamp(0.0, 1.0),
            phase: Phase::FadingIn,
            fade_out_start,
        }
    } else {
        Render {
            alpha: 1.0,
            phase: Phase::Holding,
            fade_out_start,
        }
    };
    Some(render)
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color::rgba(c.r, c.g, c.b, c.a * a)
}

/// The overlay widget that stacks and paints the active toasts. Mount one per
/// window with [`mount`]; app code never builds this directly beyond that.
///
/// Reads the process-global queue each paint, so it needs no children and no
/// per-frame wiring — pushing a toast anywhere makes it appear here.
pub struct ToastHost {
    width: f32,
}

impl ToastHost {
    /// A host at the default card width.
    pub fn new() -> Self {
        Self { width: WIDTH }
    }

    /// The card width in pixels (messages wrap within it). Non-positive values
    /// are ignored.
    pub fn width(mut self, px: f32) -> Self {
        if px > 0.0 {
            self.width = px;
        }
        self
    }

    /// Left inset of the message text: past the accent pill and its gap.
    fn text_left(&self) -> f32 {
        ACCENT_INSET + ACCENT_W + ACCENT_GAP
    }

    /// Content width available to a message, inside the accent on the left and
    /// the padding on the right.
    fn text_width(&self) -> f32 {
        (self.width - self.text_left() - PAD_X).max(1.0)
    }
}

impl Default for ToastHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for ToastHost {
    // Deliberately no `accessibility` node: a click-through status overlay adds
    // no operable semantics, and its transient text is announced by the app's
    // own live-region story where one exists. Keeping it out of the a11y tree
    // avoids a permanent empty group under the window root.

    fn style(&self) -> FlexStyle {
        FlexStyle::new()
    }

    fn measure(&self, _available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let now = animation::now();
        let font = ctx.theme.typography.body.font_size;
        let line = ctx.theme.typography.body.line_height;
        let attrs = TextAttrs::default();
        let tw = self.text_width();

        let mut total = 0.0f32;
        let mut count = 0usize;
        TOASTS.with(|t| {
            for toast in t.borrow().iter() {
                if render_at(toast.created, toast.hold, toast.dismissed, now).is_none() {
                    continue;
                }
                let shaped =
                    ctx.text_engine
                        .shape_text_attrs(&toast.message, font, line, Some(tw), &attrs);
                total += (shaped.height + PAD_Y * 2.0).max(line + PAD_Y * 2.0);
                count += 1;
            }
        });
        if count == 0 {
            return Some(Size::ZERO);
        }
        total += GAP * (count as f32 - 1.0);
        Some(Size::new(self.width, total))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let now = animation::now();
        let font = ctx.theme.typography.body.font_size;
        let line = ctx.theme.typography.body.line_height;
        let attrs = TextAttrs::default();
        let tw = self.text_width();

        let surface = ctx.theme.colors.surface;
        let outline = ctx.theme.colors.outline;
        let text_color = ctx.theme.colors.on_surface;

        // Snapshot the render state (without mutating), draw top→bottom, then
        // prune the fully-gone entries and cast exactly one frame vote.
        let mut any_fading = false;
        let mut next_wake: Option<Instant> = None;
        let mut y = layout.origin.y;

        TOASTS.with(|t| {
            for toast in t.borrow().iter() {
                let Some(r) = render_at(toast.created, toast.hold, toast.dismissed, now) else {
                    continue;
                };
                match r.phase {
                    Phase::FadingIn | Phase::FadingOut => any_fading = true,
                    Phase::Holding => {
                        next_wake = Some(match next_wake {
                            Some(w) => w.min(r.fade_out_start),
                            None => r.fade_out_start,
                        });
                    }
                }

                let shaped =
                    ctx.text_engine
                        .shape_text_attrs(&toast.message, font, line, Some(tw), &attrs);
                let card_h = (shaped.height + PAD_Y * 2.0).max(line + PAD_Y * 2.0);
                let card = Rect::new(layout.origin.x, y, self.width, card_h);
                let a = r.alpha;

                // Elevation: a soft shadow nudged down, so the card reads as
                // floating regardless of the color behind it.
                ctx.fill_shadow(
                    Rect::new(card.origin.x, card.origin.y + 3.0, card.size.width, card_h),
                    with_alpha(Color::BLACK, 0.35 * a),
                    RADIUS,
                    SHADOW_BLUR,
                );
                // Card body + a hairline border for definition on low-contrast
                // backgrounds.
                ctx.fill_rect_rounded(card, with_alpha(surface, a), RADIUS);
                ctx.stroke_rect_rounded(card, with_alpha(outline, a), RADIUS, 1.0);
                // Severity accent — an inset pill down the leading edge.
                ctx.fill_rect_rounded(
                    Rect::new(
                        card.origin.x + ACCENT_INSET,
                        card.origin.y + ACCENT_INSET,
                        ACCENT_W,
                        (card_h - ACCENT_INSET * 2.0).max(ACCENT_W),
                    ),
                    with_alpha(toast.kind.accent(&ctx.theme), a),
                    ACCENT_W / 2.0,
                );

                // Message, vertically centered in the card.
                let text_x = card.origin.x + self.text_left();
                let text_y = card.origin.y + (card_h - shaped.height) / 2.0;
                let color = with_alpha(text_color, a);
                ctx.push_clip(card);
                for glyph in &shaped.glyphs {
                    if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                        ctx.draw_glyph(
                            text_x + glyph.x,
                            text_y + glyph.y,
                            image,
                            color,
                            glyph.cache_key,
                        );
                    }
                }
                ctx.pop_clip();

                y += card_h + GAP;
            }
        });

        // Remove the fully-faded toasts once, after drawing this frame.
        TOASTS.with(|t| {
            t.borrow_mut().retain(|toast| {
                render_at(toast.created, toast.hold, toast.dismissed, now).is_some()
            });
        });

        // Vote: continuous while anything animates, otherwise a single sparse
        // wake at the next hold→fade transition. Nothing left ⇒ no vote ⇒ idle.
        if any_fading {
            request_frame();
        } else if let Some(w) = next_wake {
            request_frame_at(w);
        }
    }
}

/// Mount the toast overlay into `tree`, returning the layer's root index.
///
/// Call once at app boot, after building the main tree. The overlay is a
/// bottom-center, click-through layer; from then on any [`show`] / [`info`] /
/// … call anywhere on the UI thread makes a toast appear over the UI without
/// disturbing input focus.
pub fn mount(tree: &mut WidgetTree) -> usize {
    let options = LayerOptions::tooltip().anchor(LayerAnchor::Viewport {
        h: HAlign::Center,
        v: VAlign::End,
        offset: (0.0, -24.0),
    });
    tree.push_layer(options, ToastHost::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reset the process-global queue so tests don't leak into one another (they
    /// share the thread-local when run single-threaded).
    fn reset() {
        TOASTS.with(|t| t.borrow_mut().clear());
    }

    #[test]
    fn show_enqueues_and_returns_distinct_ids() {
        reset();
        let a = info("one");
        let b = error("two");
        assert_ne!(a, b, "each toast gets a fresh id");
        assert_eq!(active_count(), 2);
    }

    #[test]
    fn backlog_is_capped_to_the_most_recent() {
        reset();
        for i in 0..(MAX_TOASTS + 3) {
            show(format!("msg {i}"));
        }
        assert_eq!(active_count(), MAX_TOASTS, "a flood drops the oldest");
    }

    #[test]
    fn fade_in_hold_fade_out_alpha_curve() {
        let created = Instant::now();
        let hold = Duration::from_millis(1000);

        // Start of life: invisible, fading in.
        let r = render_at(created, hold, None, created).unwrap();
        assert_eq!(r.alpha, 0.0);
        assert_eq!(r.phase, Phase::FadingIn);

        // Half through the fade-in ramp.
        let mid_in = render_at(created, hold, None, created + FADE_IN / 2).unwrap();
        assert!((mid_in.alpha - 0.5).abs() < 0.02, "≈half faded in");

        // Deep in the hold: fully opaque.
        let holding = render_at(created, hold, None, created + FADE_IN + hold / 2).unwrap();
        assert_eq!(holding.alpha, 1.0);
        assert_eq!(holding.phase, Phase::Holding);

        // Midway through fade-out.
        let out_start = created + FADE_IN + hold;
        let mid_out = render_at(created, hold, None, out_start + FADE_OUT / 2).unwrap();
        assert!((mid_out.alpha - 0.5).abs() < 0.02, "≈half faded out");
        assert_eq!(mid_out.phase, Phase::FadingOut);

        // Fully past the end: gone.
        assert!(render_at(created, hold, None, out_start + FADE_OUT).is_none());
    }

    #[test]
    fn dismiss_brings_the_fade_out_forward() {
        let created = Instant::now();
        let hold = Duration::from_secs(10);
        // Without dismissal it would still be holding a second in.
        let at = created + FADE_IN + Duration::from_secs(1);
        assert_eq!(
            render_at(created, hold, None, at).unwrap().phase,
            Phase::Holding
        );
        // Dismissed at that instant, the same time now reads as fade-out.
        let r = render_at(created, hold, Some(at), at).unwrap();
        assert_eq!(r.phase, Phase::FadingOut);
        assert_eq!(r.alpha, 1.0, "fade-out starts from full opacity");
        // And it's gone FADE_OUT later.
        assert!(render_at(created, hold, Some(at), at + FADE_OUT).is_none());
    }

    #[test]
    fn dismiss_marks_the_matching_toast() {
        reset();
        let id = success("saved");
        dismiss(id);
        TOASTS.with(|t| {
            let list = t.borrow();
            assert!(list[0].dismissed.is_some(), "dismiss stamps the toast");
        });
    }

    #[test]
    fn clock_before_creation_saturates_to_faded_in_zero() {
        // A frozen clock placed before `created` must not panic on the
        // Instant subtraction; it reads as the very start of the fade-in.
        let created = Instant::now();
        let earlier = created - Duration::from_millis(50);
        let r = render_at(created, DEFAULT_HOLD, None, earlier).unwrap();
        assert_eq!(r.alpha, 0.0);
        assert_eq!(r.phase, Phase::FadingIn);
    }

    #[test]
    fn accent_differs_by_kind() {
        let theme = shroud_core::Theme::default();
        assert_eq!(ToastKind::Error.accent(&theme), theme.colors.error);
        assert_eq!(ToastKind::Success.accent(&theme), theme.colors.success);
        assert_ne!(
            ToastKind::Info.accent(&theme),
            ToastKind::Warning.accent(&theme),
            "info and warning read different accents"
        );
    }
}
