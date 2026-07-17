//! Text widget — renders a text string.

use std::cell::RefCell;

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{AccessNode, AccessRole, Color, Point, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::Reactive;
use shroud_text::{FontStyle, FontWeight, TextAttrs, TextEngine, TextFamily, TextSpan};

const ELLIPSIS: &str = "\u{2026}";

/// Handler invoked when a clickable inline link (a [`TextSpan`] with a
/// [`link`](TextSpan::link) target) is clicked. Receives the span's opaque
/// target string and the dispatch context (so the handler can queue tree
/// mutations — e.g. navigating to another note).
type LinkClickHandler = Box<dyn FnMut(&str, &mut EventContext)>;

/// A cached clickable region, recomputed each paint from the shaped span
/// boxes. `rect` is block-relative (origin at the widget's layout origin),
/// matching the space a click is translated into during event dispatch.
struct LinkHit {
    rect: Rect,
    link: String,
}

/// Internal text content variant. Either plain reactive text (the original
/// path, by far the more common case) or an inline rich-text span list.
enum TextContent {
    Plain(Reactive<String>),
    Rich(Vec<TextSpan>),
}

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
    content: TextContent,
    font_size: Option<f32>,
    line_height: Option<f32>,
    color: Option<Reactive<Color>>,
    truncate: bool,
    attrs: TextAttrs,
    /// Click handler for inline links. `None` (the default) makes the widget
    /// inert to clicks — `event` returns `Ignored` immediately. Only the
    /// rich path produces clickable links (a plain `TextWidget` has no spans).
    on_link_click: Option<LinkClickHandler>,
    /// Clickable regions cached during `paint` (which has the `TextEngine`)
    /// for `event` (which does not) to hit-test against. Recomputed every
    /// paint, so it tracks the current layout/wrap. `RefCell` because `paint`
    /// takes `&self`.
    link_hits: RefCell<Vec<LinkHit>>,
    /// Link target that received the most recent `MouseDown`, so a `MouseUp`
    /// only fires the handler when press and release land on the same link
    /// (mirrors `Button`'s pressed-state click semantics).
    pressed_link: Option<String>,
    /// Optional rotation in **degrees**, clockwise-positive on screen. Read on
    /// every paint (so a `Signal<f32>` or `Animated<f32>` drives a spinning
    /// disclosure chevron with zero per-frame wiring). Glyphs turn rigidly
    /// about the widget's layout center. `None` / `0.0` paints upright.
    /// Honored on the plain-text path (the icon-glyph use case); rich-span
    /// content paints unrotated.
    rotation: Option<Reactive<f32>>,
}

impl TextWidget {
    /// Create a text widget with static content.
    ///
    /// Accepts anything that converts into a `String` (kept for ergonomics
    /// with string literals; a reactive value can also be supplied via the
    /// blanket `Into<Reactive<String>>` conversion if callers prefer).
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            content: TextContent::Plain(Reactive::Static(text.into())),
            font_size: None,
            line_height: None,
            color: None,
            truncate: false,
            attrs: TextAttrs::default(),
            on_link_click: None,
            link_hits: RefCell::new(Vec::new()),
            pressed_link: None,
            rotation: None,
        }
    }

    /// Create a text widget whose content is produced by a closure on every
    /// paint. Equivalent to `TextWidget::new("")` then setting the text
    /// attribute to `Reactive::derive(f)`; kept as a dedicated constructor
    /// because it is how the counter example and many docs introduce
    /// reactivity.
    pub fn reactive(f: impl Fn() -> String + 'static) -> Self {
        Self {
            content: TextContent::Plain(Reactive::derive(f)),
            font_size: None,
            line_height: None,
            color: None,
            truncate: false,
            attrs: TextAttrs::default(),
            on_link_click: None,
            link_hits: RefCell::new(Vec::new()),
            pressed_link: None,
            rotation: None,
        }
    }

    /// Create a text widget from an inline rich-text span list.
    ///
    /// Each [`TextSpan`] carries its own font family / weight / style and an
    /// optional color override. The shaper lays them out as a single line of
    /// text and wraps either at run boundaries *or* inside an individual
    /// span if no inter-span break fits — closing the limitation that
    /// `Container::row().flex_wrap()` of per-run text widgets could not.
    ///
    /// Widget-level `.bold()` / `.italic()` / `.family()` / `.weight()` /
    /// `.style()` / `.monospace()` builders are ignored in rich mode (each
    /// span owns its own attrs). `.color(...)` still applies as the fallback
    /// color for spans whose own `.color(...)` is unset. `.truncate(true)`
    /// is **not supported** in rich mode (markdown_demo does not use it; if
    /// you need a truncated rich line, file a follow-up).
    pub fn rich(spans: Vec<TextSpan>) -> Self {
        Self {
            content: TextContent::Rich(spans),
            font_size: None,
            line_height: None,
            color: None,
            truncate: false,
            attrs: TextAttrs::default(),
            on_link_click: None,
            link_hits: RefCell::new(Vec::new()),
            pressed_link: None,
            rotation: None,
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

    /// Rotate the rendered glyphs by `degrees` clockwise about the widget's
    /// layout center. Accepts a literal `f32`, a `Signal<f32>`, or an
    /// `Animated<f32>` (via `Into<Reactive<f32>>`), re-read every paint — so a
    /// disclosure chevron animates 0° → 90° just by handing this an
    /// `Animated<f32>`, no per-frame plumbing.
    ///
    /// Intended for single icon glyphs (a `▸`/`▾` chevron, a `+`/`×` toggle).
    /// Only the plain-text path honors it; the background, focus ring, and any
    /// decoration lines stay axis-aligned, and rich-span content paints
    /// unrotated. The widget's measured box is unchanged — rotation is purely
    /// visual, so reserve square-ish space for a glyph that will spin.
    pub fn rotation(mut self, degrees: impl Into<Reactive<f32>>) -> Self {
        self.rotation = Some(degrees.into());
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

    /// Make inline links clickable: the handler fires when a [`TextSpan`] that
    /// carries a [`link`](TextSpan::link) target is clicked, receiving that
    /// opaque target string plus the dispatch [`EventContext`].
    ///
    /// Only meaningful on the [`rich`](Self::rich) path — a plain or reactive
    /// `TextWidget` has no spans and so no links. A click that misses every
    /// link region is left unhandled (returns `Ignored`), so it doesn't
    /// swallow scrolls or selection on the surrounding text.
    ///
    /// The clickable geometry is captured during `paint`, so a link only
    /// becomes hittable after the widget's first paint — which always happens
    /// before any click in the normal event/redraw cycle. Hit-testing is by
    /// mouse position only; there is no keyboard activation for inline links
    /// yet (the widget is not focusable).
    pub fn on_link_click(mut self, f: impl FnMut(&str, &mut EventContext) + 'static) -> Self {
        self.on_link_click = Some(Box::new(f));
        self
    }

    /// The link target at block-relative point `rel`, if any region contains
    /// it. Used by `event` to map a click to its span's link.
    fn link_at(&self, pos: Point, layout: Rect) -> Option<String> {
        let rel = Point::new(pos.x - layout.origin.x, pos.y - layout.origin.y);
        self.link_hits
            .borrow()
            .iter()
            .find(|h| h.rect.contains(rel))
            .map(|h| h.link.clone())
    }

    /// Get the current text content.
    ///
    /// For reactive widgets this invokes the closure to produce a fresh
    /// value; for static widgets it clones the stored string. For rich-text
    /// widgets the span texts are concatenated (no styling preserved) so
    /// callers that probe the displayed text see something reasonable.
    pub fn text(&self) -> String {
        match &self.content {
            TextContent::Plain(r) => r.get(),
            TextContent::Rich(spans) => spans.iter().map(|s| s.text.as_str()).collect(),
        }
    }
}

impl Widget for TextWidget {
    fn accessibility(&self) -> Option<AccessNode> {
        let text = self.text();
        if text.is_empty() {
            return None;
        }
        Some(AccessNode::new(AccessRole::Label).name(text))
    }

    fn style(&self) -> FlexStyle {
        // No `min_height` here: this is a measured leaf, and its height comes
        // from `measure` (the shaped line height). Declaring a style `min_size`
        // *and* a measure makes Taffy over-count the content height of a
        // content-hugging flex ancestor whenever the two diverge — which they
        // do as soon as the font scale isn't 1.0, because a style `min_height`
        // can't see the theme's scaled line height (style() has no theme) and
        // would be a stale constant. The symptom was a centered card growing
        // ~one line per text taller than its content at "Large" font size. See
        // `Button::style` for the same invariant.
        FlexStyle::new()
    }

    fn measure(&self, available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let line_height = self
            .line_height
            .unwrap_or(ctx.theme.typography.body.line_height);

        match &self.content {
            TextContent::Plain(r) => {
                let text = r.get();
                if text.is_empty() {
                    return Some(Size::ZERO);
                }
                let natural = ctx.text_engine.shape_text_attrs(
                    &text,
                    font_size,
                    line_height,
                    None,
                    &self.attrs,
                );

                if self.truncate {
                    // Single-line height regardless of natural — truncate's
                    // contract is row-height stability. Width caps at
                    // whichever of natural / available_width is smaller so
                    // the row never claims unused space.
                    let w = match available_width {
                        Some(aw) => natural.width.min(aw),
                        None => natural.width,
                    };
                    return Some(Size::new(w.ceil(), line_height.ceil()));
                }

                // Compute max-content first. Taffy may call us with a narrow
                // `available_width` during its min-content / flex probing
                // passes; naively shaping with that as `max_width` would wrap
                // every space and return a tall, thin size that then gets
                // used as the widget's natural size — producing the bug
                // where "Count: 0" becomes two lines. So: only wrap when our
                // natural width actually overflows the available width.
                let shaped = if let Some(aw) = available_width {
                    if natural.width > aw {
                        ctx.text_engine.shape_text_attrs(
                            &text,
                            font_size,
                            line_height,
                            Some(aw),
                            &self.attrs,
                        )
                    } else {
                        natural
                    }
                } else {
                    natural
                };
                // Ceil so downstream integer rounding (Taffy's pixel rounding)
                // never leaves the widget a fractional pixel short of its
                // natural width; otherwise paint's `max_width = layout.size.width`
                // re-shaping would wrap at the last whitespace. Reproducer:
                // "Count: 0" at 32px shapes to width 118.578, Taffy rounds
                // layout to 118, paint passes max_width=118 to cosmic-text,
                // which wraps before "0".
                Some(Size::new(shaped.width.ceil(), shaped.height.ceil()))
            }
            TextContent::Rich(spans) => {
                if spans.iter().all(|s| s.text.is_empty()) {
                    return Some(Size::ZERO);
                }
                let natural = ctx
                    .text_engine
                    .shape_rich(spans, font_size, line_height, None);
                let shaped = if let Some(aw) = available_width {
                    if natural.width > aw {
                        ctx.text_engine
                            .shape_rich(spans, font_size, line_height, Some(aw))
                    } else {
                        natural
                    }
                } else {
                    natural
                };
                Some(Size::new(shaped.width.ceil(), shaped.height.ceil()))
            }
        }
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
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

        let spans = match &self.content {
            TextContent::Rich(spans) => spans,
            TextContent::Plain(_) => {
                paint_plain(self, layout, ctx, font_size, line_height, color);
                return;
            }
        };

        // Rich path: shape all spans as one inline run; cosmic-text propagates
        // each span's color into the per-glyph `color_opt`, which we lift back
        // out into `ShapedGlyph::color` and apply per-draw. Glyphs without a
        // span-level color fall back to the widget-level `color`.
        if spans.iter().all(|s| s.text.is_empty()) {
            return;
        }
        let shaped =
            ctx.text_engine
                .shape_rich(spans, font_size, line_height, Some(layout.size.width));

        // Underlines / strike-throughs first: they go to the rect batch, which
        // the renderer draws beneath the glyph batch. Since each line takes its
        // span's color (the same color its glyphs use), over/under ordering is
        // visually identical — drawing as rects just reuses the existing fill
        // pipeline instead of needing decoration support in the glyph shader.
        for line in &shaped.decoration_lines {
            ctx.fill_rect(
                Rect::new(
                    layout.origin.x + line.rect.origin.x,
                    layout.origin.y + line.rect.origin.y,
                    line.rect.size.width,
                    line.rect.size.height,
                ),
                line.color.unwrap_or(color),
            );
        }

        for glyph in &shaped.glyphs {
            if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                ctx.draw_glyph(
                    layout.origin.x + glyph.x,
                    layout.origin.y + glyph.y,
                    image,
                    glyph.color.unwrap_or(color),
                    glyph.cache_key,
                );
            }
        }

        // Refresh the clickable hit regions from this paint's shaped geometry
        // (block-relative; `event` adds the layout origin back). Only worth
        // doing when a handler is installed and at least one span is a link.
        if self.on_link_click.is_some() {
            let mut hits = self.link_hits.borrow_mut();
            hits.clear();
            for b in &shaped.span_boxes {
                if let Some(link) = spans.get(b.span).and_then(|s| s.link.as_ref()) {
                    hits.push(LinkHit {
                        rect: b.rect,
                        link: link.clone(),
                    });
                }
            }
        }
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        // Fast out for the overwhelmingly common case: no link handler means
        // the widget is inert to pointer input (it stays non-focusable too).
        if self.on_link_click.is_none() {
            return EventResult::Ignored;
        }
        match event {
            WidgetEvent::MouseDown {
                position,
                button: MouseButton::Left,
            } => match self.link_at(*position, layout) {
                Some(link) => {
                    self.pressed_link = Some(link);
                    EventResult::Consumed
                }
                None => {
                    self.pressed_link = None;
                    EventResult::Ignored
                }
            },
            WidgetEvent::MouseUp {
                position,
                button: MouseButton::Left,
            } => {
                // Fire only when press and release landed on the same link,
                // so a press-then-drag-away doesn't trigger navigation.
                let released_on = self.link_at(*position, layout);
                let fire = match (self.pressed_link.take(), released_on) {
                    (Some(p), Some(r)) if p == r => Some(r),
                    _ => None,
                };
                if let Some(link) = fire {
                    if let Some(handler) = &mut self.on_link_click {
                        handler(&link, ctx);
                    }
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            // Pointer left the widget mid-press: cancel the pending click.
            WidgetEvent::MouseLeave => {
                self.pressed_link = None;
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }
}

/// Original plain-text paint path, extracted so the rich path doesn't have to
/// thread an extra branch through every step. Behavior is verbatim what the
/// inline path was before Phase 34 (truncate + wrap support, ellipsis, etc.).
fn paint_plain(
    widget: &TextWidget,
    layout: Rect,
    ctx: &mut PaintContext,
    font_size: f32,
    line_height: f32,
    color: Color,
) {
    let TextContent::Plain(text_src) = &widget.content else {
        return;
    };
    let text = text_src.get();
    if text.is_empty() {
        return;
    }

    let truncate = widget.truncate;
    let attrs = &widget.attrs;

    // Resolve the (radians, pivot) rotation once. `0.0` collapses to `None`
    // so the upright fast path skips the push/pop entirely. The pivot is the
    // layout center, so a single icon glyph spins in place.
    let rotation = widget
        .rotation
        .as_ref()
        .map(|r| r.get())
        .filter(|deg| *deg != 0.0)
        .map(|deg| {
            (
                deg.to_radians(),
                Point::new(
                    layout.origin.x + layout.size.width / 2.0,
                    layout.origin.y + layout.size.height / 2.0,
                ),
            )
        });

    if truncate {
        // Single-line, may need ellipsis. Clip to layout so any sub-pixel
        // slop on the right edge can't bleed past the row.
        ctx.push_clip(layout);

        let natural = ctx
            .text_engine
            .shape_text_attrs(&text, font_size, line_height, None, attrs);
        let to_paint = if natural.width <= layout.size.width {
            natural
        } else {
            let display = ellipsize_to_fit(
                &text,
                &mut ctx.text_engine,
                font_size,
                line_height,
                layout.size.width,
                attrs,
            );
            if display.is_empty() {
                ctx.pop_clip();
                return;
            }
            ctx.text_engine
                .shape_text_attrs(&display, font_size, line_height, None, attrs)
        };

        if let Some((angle, pivot)) = rotation {
            ctx.push_rotation(angle, pivot);
        }
        for glyph in &to_paint.glyphs {
            if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                ctx.draw_glyph(
                    layout.origin.x + glyph.x,
                    layout.origin.y + glyph.y,
                    image,
                    color,
                    glyph.cache_key,
                );
            }
        }
        if rotation.is_some() {
            ctx.pop_rotation();
        }

        ctx.pop_clip();
        return;
    }

    let shaped = ctx.text_engine.shape_text_attrs(
        &text,
        font_size,
        line_height,
        Some(layout.size.width),
        attrs,
    );

    if let Some((angle, pivot)) = rotation {
        ctx.push_rotation(angle, pivot);
    }
    for glyph in &shaped.glyphs {
        if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
            ctx.draw_glyph(
                layout.origin.x + glyph.x,
                layout.origin.y + glyph.y,
                image,
                color,
                glyph.cache_key,
            );
        }
    }
    if rotation.is_some() {
        ctx.pop_rotation();
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
