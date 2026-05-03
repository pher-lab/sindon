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

/// A scrollable viewport.
///
/// # Example (conceptual)
/// ```ignore
/// let sv = ScrollView::new()
///     .height(300.0)
///     .width_full()
///     .content_height(1200.0);
/// ```
pub struct ScrollView {
    scroll_y: f32,
    content_height: f32,
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
            scroll_y: 0.0,
            content_height: 0.0,
            style: FlexStyle::new().column(),
            show_scrollbar: true,
            base_padding: 0.0,
            background: None,
            track_color: None,
            thumb_color: None,
        }
    }

    /// Declare the total height of the scrollable content.
    ///
    /// This is used to clamp scrolling. In Phase 11 the caller supplies this
    /// value explicitly; future iterations may derive it from the laid-out
    /// children.
    pub fn content_height(mut self, h: f32) -> Self {
        self.content_height = h;
        self
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

    /// Current vertical scroll offset in pixels.
    pub fn scroll_y(&self) -> f32 {
        self.scroll_y
    }

    /// Maximum allowed scroll offset given the current viewport height.
    pub fn max_scroll_y(&self, viewport_height: f32) -> f32 {
        (self.content_height - viewport_height).max(0.0)
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
        ctx.push_offset(0.0, -self.scroll_y);
    }

    fn paint_post_children(&self, layout: Rect, ctx: &mut PaintContext) {
        ctx.pop_offset();
        ctx.pop_clip();

        // Scrollbar (drawn on top, unclipped, at viewport coords).
        if !self.show_scrollbar {
            return;
        }
        let viewport_h = layout.size.height;
        if self.content_height <= viewport_h || viewport_h <= 0.0 {
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
        let thumb_h = ((viewport_h / self.content_height) * viewport_h).max(SCROLLBAR_THUMB_MIN);
        let thumb_h = thumb_h.min(viewport_h);
        let max_scroll = (self.content_height - viewport_h).max(0.0);
        let progress = if max_scroll > 0.0 {
            (self.scroll_y / max_scroll).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb_y = layout.origin.y + progress * (viewport_h - thumb_h);
        let thumb_rect = Rect::new(track_x, thumb_y, SCROLLBAR_WIDTH, thumb_h);
        ctx.fill_rect(thumb_rect, thumb_color);
    }

    fn scroll_offset(&self) -> (f32, f32) {
        (0.0, self.scroll_y)
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
            let new_y = (self.scroll_y - delta_y).clamp(0.0, max);
            if new_y != self.scroll_y {
                self.scroll_y = new_y;
            }
            return EventResult::Consumed;
        }
        EventResult::Ignored
    }
}
