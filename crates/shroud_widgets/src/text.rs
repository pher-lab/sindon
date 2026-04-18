//! Text widget — renders a text string.

use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Color, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::Reactive;

/// A text display widget.
///
/// Shapes and rasterizes text during `paint()` using the `TextEngine`
/// provided by `PaintContext`.
///
/// The text content and color are stored as [`Reactive<T>`], so either a
/// literal value or a signal-backed closure is accepted. A closure is
/// re-evaluated on every paint; this is how widgets observe reactive state
/// in the pull-based model.
///
/// Convenience constructors [`TextWidget::new`] and [`TextWidget::reactive`]
/// preserve the pre-Phase-14 API — they delegate to `Reactive::Static` and
/// `Reactive::derive` respectively.
pub struct TextWidget {
    text: Reactive<String>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    color: Option<Reactive<Color>>,
}

impl TextWidget {
    /// Create a text widget with static content.
    ///
    /// Accepts anything that converts into a `String` (kept for ergonomics
    /// with string literals; a reactive value can also be supplied via the
    /// blanket `Into<Reactive<String>>` conversion if callers prefer).
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: Reactive::Static(text.into()),
            font_size: None,
            line_height: None,
            color: None,
        }
    }

    /// Create a text widget whose content is produced by a closure on every
    /// paint. Equivalent to `TextWidget::new("")` then setting the text
    /// attribute to `Reactive::derive(f)`; kept as a dedicated constructor
    /// because it is how the counter example and many docs introduce
    /// reactivity.
    pub fn reactive(f: impl Fn() -> String + 'static) -> Self {
        Self {
            text: Reactive::derive(f),
            font_size: None,
            line_height: None,
            color: None,
        }
    }

    /// Set font size in pixels.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set line height in pixels.
    pub fn line_height(mut self, px: f32) -> Self {
        self.line_height = Some(px);
        self
    }

    /// Set text color.
    ///
    /// Accepts a literal `Color`, a `Signal<Color>`, a `Memo<Color>`, or a
    /// `Reactive::derive(...)` closure. Dynamic sources are re-read on every
    /// paint.
    pub fn color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Get the current text content.
    ///
    /// For reactive widgets this invokes the closure to produce a fresh
    /// value; for static widgets it clones the stored string.
    pub fn text(&self) -> String {
        self.text.get()
    }
}

impl Widget for TextWidget {
    fn style(&self) -> FlexStyle {
        let line_height = self.line_height.unwrap_or(22.0);
        FlexStyle::new().min_height(line_height)
    }

    fn measure(&self, available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let text = self.text.get();
        if text.is_empty() {
            return Some(Size::ZERO);
        }
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let line_height = self
            .line_height
            .unwrap_or(ctx.theme.typography.body.line_height);

        // Compute max-content first. Taffy may call us with a narrow
        // `available_width` during its min-content / flex probing passes;
        // naively shaping with that as `max_width` would wrap every space
        // and return a tall, thin size that then gets used as the widget's
        // natural size — producing the bug where "Count: 0" becomes two
        // lines. So: only wrap when our natural width actually overflows
        // the available width.
        let natural = ctx
            .text_engine
            .shape_text(&text, font_size, line_height, None);
        let shaped = if let Some(aw) = available_width {
            if natural.width > aw {
                ctx.text_engine
                    .shape_text(&text, font_size, line_height, Some(aw))
            } else {
                natural
            }
        } else {
            natural
        };
        // Ceil so downstream integer rounding (Taffy's pixel rounding) never
        // leaves the widget a fractional pixel short of its natural width;
        // otherwise paint's `max_width = layout.size.width` re-shaping
        // would wrap at the last whitespace. Reproducer: "Count: 0" at
        // 32px shapes to width 118.578, Taffy rounds layout to 118, paint
        // passes max_width=118 to cosmic-text, which wraps before "0".
        Some(Size::new(shaped.width.ceil(), shaped.height.ceil()))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        // Materialize the current text. For Dynamic variants this reads
        // the latest signal values each paint; for Static variants it is
        // just a clone of the stored string.
        let text = self.text.get();

        if text.is_empty() {
            return;
        }

        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let line_height = self
            .line_height
            .unwrap_or(ctx.theme.typography.body.line_height);
        let color = self
            .color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(ctx.theme.colors.on_background);

        let shaped =
            ctx.text_engine
                .shape_text(&text, font_size, line_height, Some(layout.size.width));

        for glyph in &shaped.glyphs {
            if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                ctx.draw_glyph(
                    layout.origin.x as i32 + glyph.x,
                    layout.origin.y as i32 + glyph.y,
                    image,
                    color,
                    glyph.cache_key,
                );
            }
        }
    }
}
