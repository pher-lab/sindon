//! Text widget — renders a text string.

use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Color, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::Reactive;
use shroud_text::{FontStyle, FontWeight, TextAttrs, TextEngine, TextFamily};

const ELLIPSIS: &str = "\u{2026}";

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
///
/// ## Wrap (default)
///
/// By default the widget wraps on word boundaries when its laid-out width is
/// narrower than the natural shaped width. `measure()` reports the wrapped
/// height so Taffy allocates enough vertical room. Wrap is the only mode for
/// the non-truncated path — there is no "no-wrap with overflow" knob.
///
/// ## Truncate (opt-in)
///
/// Call [`TextWidget::truncate`] with `true` to force a single line and
/// append `…` (U+2026) when the text overflows the laid-out width. Use this
/// for sidebar items, file paths, or any row where height stability matters
/// more than showing the full string.
pub struct TextWidget {
    text: Reactive<String>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    color: Option<Reactive<Color>>,
    truncate: bool,
    attrs: TextAttrs,
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
            truncate: false,
            attrs: TextAttrs::default(),
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
            truncate: false,
            attrs: TextAttrs::default(),
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

    /// Force single-line rendering with a trailing `…` (U+2026) when the
    /// text overflows the laid-out width. Default `false` (wrap on).
    pub fn truncate(mut self, on: bool) -> Self {
        self.truncate = on;
        self
    }

    /// Set the font weight (e.g. `FontWeight::BOLD`, `FontWeight::MEDIUM`).
    /// Default is `FontWeight::NORMAL`.
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.attrs.weight = weight;
        self
    }

    /// Set the font slant (`FontStyle::Normal` / `Italic` / `Oblique`).
    /// Default is `FontStyle::Normal`.
    pub fn style(mut self, style: FontStyle) -> Self {
        self.attrs.style = style;
        self
    }

    /// Set the font family. Default is `TextFamily::SansSerif`.
    pub fn family(mut self, family: TextFamily) -> Self {
        self.attrs.family = family;
        self
    }

    /// Shorthand for `.weight(FontWeight::BOLD)`.
    pub fn bold(self) -> Self {
        self.weight(FontWeight::BOLD)
    }

    /// Shorthand for `.style(FontStyle::Italic)`.
    pub fn italic(self) -> Self {
        self.style(FontStyle::Italic)
    }

    /// Shorthand for `.family(TextFamily::Monospace)`.
    pub fn monospace(self) -> Self {
        self.family(TextFamily::Monospace)
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

        let natural =
            ctx.text_engine
                .shape_text_attrs(&text, font_size, line_height, None, &self.attrs);

        if self.truncate {
            // Single-line height regardless of natural — truncate's contract
            // is row-height stability. Width caps at whichever of natural /
            // available_width is smaller so the row never claims unused space.
            let w = match available_width {
                Some(aw) => natural.width.min(aw),
                None => natural.width,
            };
            return Some(Size::new(w.ceil(), line_height.ceil()));
        }

        // Compute max-content first. Taffy may call us with a narrow
        // `available_width` during its min-content / flex probing passes;
        // naively shaping with that as `max_width` would wrap every space
        // and return a tall, thin size that then gets used as the widget's
        // natural size — producing the bug where "Count: 0" becomes two
        // lines. So: only wrap when our natural width actually overflows
        // the available width.
        let shaped = if let Some(aw) = available_width {
            if natural.width > aw {
                ctx.text_engine
                    .shape_text_attrs(&text, font_size, line_height, Some(aw), &self.attrs)
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

        // Defensive: if layout collapsed the text box to zero width (e.g. an
        // inner column squeezed by a sibling in a row), bail out instead of
        // painting glyphs at their natural, unwrapped positions. cosmic-text
        // treats `Some(0.0)` as an unconstrained shape and emits an overflow
        // single-line layout that bleeds across whatever sibling boxes happen
        // to sit to the right — the symptom that first surfaced as the
        // markdown_demo blockquote text overflow. Width is the only axis that
        // can cause this; zero height just clips vertically, which is safe.
        if layout.size.width <= 0.0 {
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

        if self.truncate {
            // Single-line, may need ellipsis. Clip to layout so any sub-pixel
            // slop on the right edge can't bleed past the row.
            ctx.push_clip(layout);

            let natural = ctx.text_engine.shape_text_attrs(
                &text,
                font_size,
                line_height,
                None,
                &self.attrs,
            );
            let to_paint = if natural.width <= layout.size.width {
                natural
            } else {
                let display = ellipsize_to_fit(
                    &text,
                    &mut ctx.text_engine,
                    font_size,
                    line_height,
                    layout.size.width,
                    &self.attrs,
                );
                if display.is_empty() {
                    ctx.pop_clip();
                    return;
                }
                ctx.text_engine
                    .shape_text_attrs(&display, font_size, line_height, None, &self.attrs)
            };

            for glyph in &to_paint.glyphs {
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

            ctx.pop_clip();
            return;
        }

        let shaped = ctx.text_engine.shape_text_attrs(
            &text,
            font_size,
            line_height,
            Some(layout.size.width),
            &self.attrs,
        );

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

/// Walk char boundaries from longest to shortest prefix, returning the
/// longest `prefix + "…"` whose shaped width fits in `max_width`.
///
/// Returns the empty string when even `"…"` alone doesn't fit (caller
/// renders nothing — better than overflow). Linear in the number of chars;
/// fine for typical title-length inputs (~100 chars max).
fn ellipsize_to_fit(
    text: &str,
    engine: &mut TextEngine,
    font_size: f32,
    line_height: f32,
    max_width: f32,
    attrs: &TextAttrs,
) -> String {
    if max_width <= 0.0 {
        return String::new();
    }

    let ellipsis_only = engine.shape_text_attrs(ELLIPSIS, font_size, line_height, None, attrs);
    if ellipsis_only.width > max_width {
        return String::new();
    }

    // Collect char boundaries (byte indices where a char starts). Walk from
    // longest to shortest so the first that fits is the answer. `text.len()`
    // is the "all chars" prefix; we know the full text doesn't fit (caller
    // already shaped natural and confirmed overflow), so start one char back.
    let mut boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    boundaries.push(text.len());

    // Drop the last entry — the full string already overflows, no need to
    // re-shape it with an extra ellipsis appended.
    if boundaries.pop().is_none() {
        return String::new();
    }

    while let Some(end) = boundaries.pop() {
        let mut candidate = String::with_capacity(end + ELLIPSIS.len());
        candidate.push_str(&text[..end]);
        candidate.push_str(ELLIPSIS);
        let shaped = engine.shape_text_attrs(&candidate, font_size, line_height, None, attrs);
        if shaped.width <= max_width {
            return candidate;
        }
    }

    // Even prefix-of-zero + ellipsis was the only thing left. Return just
    // the ellipsis (we already verified it fits at the top).
    ELLIPSIS.to_string()
}
