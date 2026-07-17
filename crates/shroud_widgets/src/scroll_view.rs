//! ScrollView — clips its children and translates them by a scroll offset.
//!
//! The widget clips children to its own bounds and shifts them vertically
//! by `scroll_y`. Mouse wheel events whose cursor is over the viewport
//! update `scroll_y`, clamped against the user-declared `content_height`.
//!
//! Phase 11 MVP: vertical scrolling only; content height is supplied by the
//! caller via [`ScrollView::content_height`]. An optional visual scrollbar
//! is drawn on the right edge (non-interactive).

use crate::event::{EventContext, EventResult, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Rect};
use shroud_layout::FlexStyle;
use shroud_reactive::{Animated, Easing};
use std::time::Duration;

/// A scrollable viewport.
///
/// # Example (conceptual)
/// ```ignore
/// let sv = ScrollView::new()
///     .height(300.0)
///     .width_full()
///     .content_height(1200.0);
/// ```
/// Default duration of the smooth-scroll glide applied to wheel input.
/// Short enough to feel responsive, long enough to read as motion (matches
/// the hover-fade default). `scroll_transition(Duration::ZERO)` opts back out
/// to the pre-animation instant jump.
const SCROLL_TRANSITION_DEFAULT: Duration = Duration::from_millis(120);

pub struct ScrollView {
    /// Wheel scrolling retargets this animator and the displayed offset eases
    /// toward it, so a fast flick doesn't teleport the content (FW-7). Lazily
    /// created on first scroll (like `Container`'s hover fade); `None` reads as
    /// a resting offset of 0. The *target* is the logical scroll position
    /// ([`scroll_y`](Self::scroll_y)); the *current eased* value is what paint
    /// and hit-testing use ([`scroll_offset`](Widget::scroll_offset)).
    scroll_anim: Option<Animated<f32>>,
    /// Glide duration for wheel scrolling. `Duration::ZERO` = instant.
    scroll_transition: Duration,
    /// Caller-pinned content height. `None` means "auto" — the widget tree
    /// writes the laid-out children's max bottom into [`Self::auto_content_height`]
    /// after each layout pass and that value is used for scroll clamp /
    /// scrollbar.
    explicit_content_height: Option<f32>,
    /// Measured total content extent in pixels, populated by the tree after
    /// every layout pass (see `WidgetTree::sync_scroll_view_content_heights`).
    /// Includes the ScrollView's top + bottom base padding so a fully scrolled
    /// viewport shows the bottom padding flush with the last child. Ignored
    /// when [`Self::explicit_content_height`] is `Some`.
    auto_content_height: f32,
    style: FlexStyle,
    show_scrollbar: bool,
    /// User-supplied uniform padding (set via [`Self::padding`]). Stored
    /// separately so [`Widget::style`] can re-emit padding with the scrollbar
    /// gutter added to the right side without needing to read padding back
    /// out of Taffy's opaque `LengthPercentage`.
    base_padding: f32,
    // Colors (None = theme fallback)
    background: Option<Color>,
    track_color: Option<Color>,
    thumb_color: Option<Color>,
}

impl ScrollView {
    /// Create a new scroll view (column-oriented content).
    pub fn new() -> Self {
        Self {
            scroll_anim: None,
            scroll_transition: SCROLL_TRANSITION_DEFAULT,
            explicit_content_height: None,
            auto_content_height: 0.0,
            // A scroll container must declare `overflow: hidden` so flex
            // layout lets it shrink below its content. By default a flex item's
            // automatic minimum size is its content size, so a `grow(1.0)`
            // viewport nested under grow containers (no explicit height) would
            // balloon to its overflowing content instead of clamping to the
            // allocated space — leaving nothing to scroll. `overflow: hidden`
            // sets that automatic minimum to 0 *and* stops the overflow from
            // contributing to ancestors' scroll regions, so the intermediate
            // `grow` containers don't balloon either. Visual clipping is done
            // in paint; this only affects layout, and `Hidden` reserves no
            // scrollbar gutter (we draw our own).
            style: FlexStyle::new().column().overflow_hidden(),
            show_scrollbar: true,
            base_padding: 0.0,
            background: None,
            track_color: None,
            thumb_color: None,
        }
    }

    /// Pin the total height of the scrollable content.
    ///
    /// By default a `ScrollView` measures its laid-out children every layout
    /// pass and uses the max bottom as `content_height` — call this only when
    /// you need to lock the value (e.g. virtualized lists where the rendered
    /// subtree is shorter than the logical content).
    pub fn content_height(mut self, h: f32) -> Self {
        self.explicit_content_height = Some(h);
        self
    }

    /// Effective content extent — explicit value if set, otherwise the value
    /// the tree wrote after the last layout pass.
    fn effective_content_height(&self) -> f32 {
        self.explicit_content_height
            .unwrap_or(self.auto_content_height)
    }

    /// Tree-side hook (same crate) for writing the measured content extent.
    /// Always updates `auto_content_height`; a caller's explicit
    /// [`Self::content_height`] still wins via [`Self::effective_content_height`].
    pub(crate) fn set_measured_content_height(&mut self, h: f32) {
        self.auto_content_height = h;
    }

    /// Padding the tree should add to the children's measured bottom to
    /// produce the total content extent (matches the top padding that Taffy
    /// has already baked into each child's relative y).
    pub(crate) fn measured_bottom_padding(&self) -> f32 {
        self.base_padding
    }

    /// Set the fixed viewport height.
    pub fn height(mut self, px: f32) -> Self {
        self.style = self.style.height(px);
        self
    }

    /// Set the fixed viewport width.
    pub fn width(mut self, px: f32) -> Self {
        self.style = self.style.width(px);
        self
    }

    /// Fill available width.
    pub fn width_full(mut self) -> Self {
        self.style = self.style.width_full();
        self
    }

    /// Fill available height.
    pub fn height_full(mut self) -> Self {
        self.style = self.style.height_full();
        self
    }

    /// Flex-grow factor — the scroll view claims this share of any leftover
    /// space along the parent's main axis. Pair with `.grow(1.0)` siblings
    /// for the common "viewport fills whatever the header leaves" layout.
    pub fn grow(mut self, factor: f32) -> Self {
        self.style = self.style.grow(factor);
        self
    }

    /// Set gap between children.
    pub fn gap(mut self, px: f32) -> Self {
        self.style = self.style.gap(px);
        self
    }

    /// Set uniform padding on the viewport. The right side may receive
    /// additional padding for the scrollbar gutter — see [`Widget::style`].
    pub fn padding(mut self, px: f32) -> Self {
        self.style = self.style.padding(px);
        self.base_padding = px;
        self
    }

    /// Toggle the visual scrollbar (drawn on the right edge).
    pub fn show_scrollbar(mut self, show: bool) -> Self {
        self.show_scrollbar = show;
        self
    }

    /// Override the viewport background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Override the scrollbar track color.
    pub fn track_color(mut self, color: Color) -> Self {
        self.track_color = Some(color);
        self
    }

    /// Override the scrollbar thumb color.
    pub fn thumb_color(mut self, color: Color) -> Self {
        self.thumb_color = Some(color);
        self
    }

    /// Set how long a wheel scroll takes to glide to its new position.
    /// Defaults to `SCROLL_TRANSITION_DEFAULT` (120 ms); pass
    /// [`Duration::ZERO`] for the pre-animation instant jump (FW-7).
    pub fn scroll_transition(mut self, duration: Duration) -> Self {
        self.scroll_transition = duration;
        self
    }

    /// The logical (target) vertical scroll offset in pixels — where the
    /// content is heading. Equals the displayed offset once any glide settles.
    /// Wheel input and clamping operate on this; the smoothly-eased value the
    /// user sees is [`scroll_offset`](Widget::scroll_offset).
    pub fn scroll_y(&self) -> f32 {
        self.scroll_anim.as_ref().map_or(0.0, |a| a.target())
    }

    /// The current eased offset the content is drawn at — lags the target
    /// while a wheel glide is in flight, then matches it. Reads also vote for
    /// another frame while animating (see [`Animated`]).
    fn displayed_scroll(&self) -> f32 {
        self.scroll_anim.as_ref().map_or(0.0, |a| a.get())
    }

    /// Retarget the scroll glide, lazily creating the animator with the
    /// configured [`scroll_transition`](Self::scroll_transition) duration.
    fn drive_scroll(&mut self, to: f32) {
        self.scroll_anim
            .get_or_insert_with(|| Animated::new(0.0, self.scroll_transition, Easing::EaseOut))
            .set(to);
    }

    /// Maximum allowed scroll offset given the current viewport height.
    pub fn max_scroll_y(&self, viewport_height: f32) -> f32 {
        (self.effective_content_height() - viewport_height).max(0.0)
    }

    /// Scroll the minimum distance that brings the vertical span
    /// `[top, top + height]` inside the viewport, and return the resulting
    /// scroll target.
    ///
    /// `top` is measured in *content* space — the frame the children's layout
    /// rects live in, before [`Widget::scroll_offset`] shifts them — so a
    /// caller converts a descendant's rect with `descendant.y - viewport.y`.
    ///
    /// A span that is already fully visible does not move: the current target
    /// is only clamped into the range of offsets that satisfy both edges. When
    /// the span is taller than the viewport that range inverts (every offset
    /// between the bounds leaves it covering the viewport entirely), which the
    /// same clamp handles once the bounds are ordered — a covering span stays
    /// put, and one that is off-screen is pulled in by its nearest edge.
    ///
    /// Retargets the same glide wheel scrolling uses: this is a response to a
    /// user gesture (Tab), so the content slides rather than teleports.
    pub(crate) fn reveal_span(&mut self, top: f32, height: f32, viewport_height: f32) -> f32 {
        let cur = self.scroll_y();
        // A zero-height viewport means the tree has not laid this view out
        // yet; every bound below would be nonsense, so keep the offset.
        if viewport_height <= 0.0 {
            return cur;
        }
        // The offsets that put the span's top edge at the viewport top, and
        // its bottom edge at the viewport bottom, respectively.
        let at_top = top;
        let at_bottom = top + height - viewport_height;
        let (lo, hi) = if at_bottom <= at_top {
            (at_bottom, at_top)
        } else {
            (at_top, at_bottom)
        };
        let new = cur
            .clamp(lo, hi)
            .clamp(0.0, self.max_scroll_y(viewport_height));
        if new != cur {
            self.drive_scroll(new);
        }
        new
    }

    /// Re-clamp the scroll offset to the current content/viewport. The tree
    /// calls this after each layout pass (see
    /// `WidgetTree::sync_scroll_view_content_heights`) so that when the content
    /// shrinks — e.g. switching to a shorter note, or deleting text — a stale
    /// offset doesn't leave the top scrolled out of view. The offset only ever
    /// moves *down* to the new maximum (0 when nothing overflows); growing
    /// content leaves it untouched, so an active scroll position is preserved.
    pub(crate) fn clamp_scroll(&mut self, viewport_height: f32) {
        let max = self.max_scroll_y(viewport_height);
        if let Some(a) = &self.scroll_anim {
            if a.target() > max {
                // Re-clamp is a system correction, not a user gesture — snap
                // instantly so switching to a shorter note doesn't visibly
                // slide the (reused) viewport back up.
                a.snap(max);
            }
        }
    }
}

impl Default for ScrollView {
    fn default() -> Self {
        Self::new()
    }
}

/// Width of the visual scrollbar in pixels.
const SCROLLBAR_WIDTH: f32 = 6.0;
/// Inset from the right edge.
const SCROLLBAR_INSET: f32 = 2.0;
/// Minimum height of the scrollbar thumb.
const SCROLLBAR_THUMB_MIN: f32 = 16.0;
/// Total horizontal space reserved on the right for the scrollbar.
///
/// Equals scrollbar width + edge inset + a small breathing margin so children
/// do not visually crowd the track. The reservation is unconditional whenever
/// `show_scrollbar` is true (cf. CSS `scrollbar-gutter: stable`) — it avoids
/// horizontal reflow at the moment content starts overflowing and is the fix
/// for the bug where unbreakable strings (e.g. long `AAAAAA…`) drew under the
/// scrollbar track.
const SCROLLBAR_GUTTER: f32 = SCROLLBAR_WIDTH + SCROLLBAR_INSET + 4.0;

impl Widget for ScrollView {
    /// Returns the layout style with the scrollbar gutter folded into the
    /// right padding when [`Self::show_scrollbar`] is on. Children laid out
    /// inside this padded box never overlap the scrollbar track, even when
    /// they declare `width_full` and the content overflows. Cleared by
    /// `show_scrollbar(false)`.
    fn style(&self) -> FlexStyle {
        if self.show_scrollbar {
            self.style.clone().padding_trbl(
                self.base_padding,
                self.base_padding + SCROLLBAR_GUTTER,
                self.base_padding,
                self.base_padding,
            )
        } else {
            self.style.clone()
        }
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        // Viewport background (drawn without any scroll offset).
        let bg = self.background.unwrap_or(ctx.theme.colors.surface);
        ctx.fill_rect(layout, bg);
    }

    fn paint_pre_children(&self, layout: Rect, ctx: &mut PaintContext) {
        ctx.push_clip(layout);
        ctx.push_offset(0.0, -self.displayed_scroll());
    }

    fn paint_post_children(&self, layout: Rect, ctx: &mut PaintContext) {
        ctx.pop_offset();
        ctx.pop_clip();

        // Scrollbar (drawn on top, unclipped, at viewport coords).
        if !self.show_scrollbar {
            return;
        }
        let viewport_h = layout.size.height;
        let content_h = self.effective_content_height();
        if content_h <= viewport_h || viewport_h <= 0.0 {
            return;
        }

        let track_color = self.track_color.unwrap_or(ctx.theme.colors.surface_variant);
        let thumb_color = self
            .thumb_color
            .unwrap_or(ctx.theme.colors.on_surface_variant);

        let track_x = layout.origin.x + layout.size.width - SCROLLBAR_WIDTH - SCROLLBAR_INSET;
        let track_rect = Rect::new(track_x, layout.origin.y, SCROLLBAR_WIDTH, viewport_h);
        ctx.fill_rect(track_rect, track_color);

        // Thumb size proportional to viewport/content ratio.
        let thumb_h = ((viewport_h / content_h) * viewport_h).max(SCROLLBAR_THUMB_MIN);
        let thumb_h = thumb_h.min(viewport_h);
        let max_scroll = (content_h - viewport_h).max(0.0);
        // Thumb tracks the displayed (eased) offset so it glides with the
        // content rather than jumping ahead to the target.
        let progress = if max_scroll > 0.0 {
            (self.displayed_scroll() / max_scroll).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb_y = layout.origin.y + progress * (viewport_h - thumb_h);
        let thumb_rect = Rect::new(track_x, thumb_y, SCROLLBAR_WIDTH, thumb_h);
        ctx.fill_rect(thumb_rect, thumb_color);
    }

    fn scroll_offset(&self) -> (f32, f32) {
        // Hit-testing must use the displayed (eased) offset so a click lands on
        // the glyph the user currently sees mid-glide, matching paint.
        (0.0, self.displayed_scroll())
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, _ctx: &mut EventContext) -> EventResult {
        if let WidgetEvent::Scroll {
            position, delta_y, ..
        } = event
        {
            if !layout.contains(*position) {
                return EventResult::Ignored;
            }
            let max = self.max_scroll_y(layout.size.height);
            // Accumulate against the *target*, not the in-flight displayed
            // value, so consecutive wheel ticks add up instead of fighting the
            // glide. `drive_scroll` eases the displayed offset toward it.
            let new_y = (self.scroll_y() - delta_y).clamp(0.0, max);
            if new_y != self.scroll_y() {
                self.drive_scroll(new_y);
            }
            return EventResult::Consumed;
        }
        EventResult::Ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1000px of content in a 200px viewport, so there is room to scroll in
    /// both directions from any offset the tests park at.
    fn view(scroll: f32) -> ScrollView {
        let mut sv = ScrollView::new().content_height(1000.0);
        sv.drive_scroll(scroll);
        sv
    }

    #[test]
    fn a_visible_span_is_left_alone() {
        // Sitting at 300, the viewport shows content 300..500. A span inside
        // that is already visible: revealing it must not move anything.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(350.0, 40.0, 200.0), 300.0);
        assert_eq!(sv.scroll_y(), 300.0);
    }

    #[test]
    fn a_span_below_the_viewport_scrolls_its_bottom_edge_into_view() {
        // Span at 520..560 is past the bottom (viewport ends at 500). The
        // minimum move puts its bottom edge on the viewport bottom, which
        // leaves it *just* visible rather than jumping it to the top.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(520.0, 40.0, 200.0), 360.0);
        assert_eq!(sv.scroll_y(), 360.0);
    }

    #[test]
    fn a_span_above_the_viewport_scrolls_its_top_edge_into_view() {
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(100.0, 40.0, 200.0), 100.0);
        assert_eq!(sv.scroll_y(), 100.0);
    }

    #[test]
    fn revealing_the_first_span_reaches_the_very_top() {
        // The top edge of the content must be reachable — the case where Tab
        // wraps back around to the first widget.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(0.0, 40.0, 200.0), 0.0);
    }

    #[test]
    fn the_reveal_never_scrolls_past_the_content_bounds() {
        // A span at the very end must not scroll past max (content - viewport
        // = 800), and one that would demand a negative offset clamps at 0.
        let mut sv = view(0.0);
        assert_eq!(sv.reveal_span(960.0, 40.0, 200.0), 800.0);

        let mut sv = view(500.0);
        // A span taller than the whole content can't be satisfied at the
        // bottom; the clamp keeps the offset legal.
        assert_eq!(sv.reveal_span(0.0, 1000.0, 200.0), 500.0);
    }

    #[test]
    fn a_span_taller_than_the_viewport_stays_put_while_it_covers() {
        // 400px span at 200..600 with the viewport at 300..500: the span
        // already fills the viewport, so there is no "more visible" to get to
        // and the offset must not move.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(200.0, 400.0, 200.0), 300.0);
    }

    #[test]
    fn a_span_taller_than_the_viewport_is_pulled_in_by_its_nearest_edge() {
        // Same 400px span, but the viewport is above it (0..200). Aligning its
        // top is the nearest way in.
        let mut sv = view(0.0);
        assert_eq!(sv.reveal_span(200.0, 400.0, 200.0), 200.0);

        // ...and from below (viewport 700..900), its bottom.
        let mut sv = view(700.0);
        assert_eq!(sv.reveal_span(200.0, 400.0, 200.0), 400.0);
    }

    #[test]
    fn an_unlaid_out_viewport_is_not_scrolled() {
        // Focus can be applied before the first layout pass; a zero-height
        // viewport must be treated as "no information", not as a reason to
        // reset the offset.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(500.0, 40.0, 0.0), 300.0);
        assert_eq!(sv.scroll_y(), 300.0);
    }
}
