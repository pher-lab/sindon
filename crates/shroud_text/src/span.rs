//! Inline text spans for rich-text rendering.
//!
//! A `TextSpan` is one styled chunk of an inline rich-text run: a string plus
//! its `TextAttrs` (family / weight / style) and an optional color override.
//! `TextEngine::shape_rich` takes a slice of spans and shapes them as a single
//! paragraph — letting the shaper break across spans on word boundaries and,
//! crucially, *inside* an attributed span when no inter-span break point fits.
//!
//! This is the missing piece that `Container::row().flex_wrap()` of per-run
//! `TextWidget`s could not solve: row-wrap breaks only at run boundaries, so
//! "this is **really_long_bold_token**" cannot wrap inside the bold run. With
//! a single span list the shaper sees one logical line and can split it
//! anywhere that satisfies its wrap rules.

use crate::attrs::TextAttrs;
use shroud_core::Color;

/// Line decorations drawn over a span's glyphs.
///
/// cosmic-text shapes glyphs but does not draw underlines or strike-throughs,
/// so these are rendered separately as thin filled rectangles positioned from
/// the line's baseline (see [`TextEngine::shape_rich`](crate::TextEngine::shape_rich),
/// which emits the geometry as `DecorationLine`s). Both flags can be set at
/// once — e.g. a struck-through link — though markdown rarely combines them.
/// The decoration takes the span's resolved text color.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextDecoration {
    /// Draw a line just below the baseline (for links and `<u>`-style text).
    pub underline: bool,
    /// Draw a line through the middle of the text (GFM `~~strikethrough~~`).
    pub strikethrough: bool,
}

/// One styled chunk of inline rich text.
///
/// Build with `TextSpan::new("text")` and chain the same shortcut builders
/// available on [`TextAttrs`] (`bold`, `italic`, `monospace`, etc.) plus a
/// per-span [`color`](Self::color) override, optional line
/// [`decoration`](Self::decoration) (underline / strikethrough), and an
/// optional clickable [`link`](Self::link) target.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    pub text: String,
    pub attrs: TextAttrs,
    /// Optional per-span color. When `None`, the widget's color is used (set
    /// via `TextWidget::color`). When `Some`, this color wins for every glyph
    /// shaped from this span.
    pub color: Option<Color>,
    /// Underline / strikethrough drawn over this span's glyphs. Default
    /// (`underline: false, strikethrough: false`) draws nothing.
    pub decoration: TextDecoration,
    /// Optional opaque click target. When `Some`, the glyphs shaped from this
    /// span become a clickable region: a `TextWidget::rich` carrying an
    /// `on_link_click` handler invokes it with this string when the region is
    /// clicked. The string is application-defined (a URL, a note title, a
    /// `scheme:payload`, …) — the framework never interprets it. `None` (the
    /// default) leaves the span non-interactive.
    pub link: Option<String>,
}

impl TextSpan {
    /// New span with default attrs, no color override, no decoration, and no
    /// link.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attrs: TextAttrs::default(),
            color: None,
            decoration: TextDecoration::default(),
            link: None,
        }
    }

    /// Replace the span's attrs wholesale.
    pub fn attrs(mut self, attrs: TextAttrs) -> Self {
        self.attrs = attrs;
        self
    }

    /// Set the font family on this span only.
    pub fn family(mut self, family: crate::attrs::TextFamily) -> Self {
        self.attrs.family = family;
        self
    }

    /// Set the font weight on this span only.
    pub fn weight(mut self, weight: crate::attrs::FontWeight) -> Self {
        self.attrs.weight = weight;
        self
    }

    /// Set the font style on this span only.
    pub fn style(mut self, style: crate::attrs::FontStyle) -> Self {
        self.attrs.style = style;
        self
    }

    /// Override the color for this span's glyphs.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Replace the span's decoration wholesale.
    pub fn decoration(mut self, decoration: TextDecoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Draw an underline below this span. See [`TextDecoration`].
    pub fn underline(mut self) -> Self {
        self.decoration.underline = true;
        self
    }

    /// Draw a strike-through line over this span (GFM `~~text~~`).
    pub fn strikethrough(mut self) -> Self {
        self.decoration.strikethrough = true;
        self
    }

    /// Mark this span as a clickable link with an opaque, application-defined
    /// target string. See [`link`](Self::link). Pair with
    /// `TextWidget::on_link_click` on the rich widget to act on the click.
    pub fn link(mut self, target: impl Into<String>) -> Self {
        self.link = Some(target.into());
        self
    }

    /// Shorthand for `.weight(FontWeight::BOLD)`.
    pub fn bold(self) -> Self {
        self.weight(crate::attrs::FontWeight::BOLD)
    }

    /// Shorthand for `.style(FontStyle::Italic)`.
    pub fn italic(self) -> Self {
        self.style(crate::attrs::FontStyle::Italic)
    }

    /// Shorthand for `.family(TextFamily::Monospace)`.
    pub fn monospace(self) -> Self {
        self.family(crate::attrs::TextFamily::Monospace)
    }
}

/// Convert a shroud `Color` (linear-ish f32 RGBA) to cosmic-text's packed
/// `Color` (sRGB-ish u8 RGBA). cosmic-text's `Color::rgba` expects 0..=255 in
/// the same channel order; the linear/sRGB mismatch is the same one shroud's
/// renderer lives with for every glyph and rect, so we use the same naïve
/// component-wise scale here for consistency.
pub(crate) fn shroud_to_cosmic(c: Color) -> cosmic_text::Color {
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    cosmic_text::Color::rgba(to_u8(c.r), to_u8(c.g), to_u8(c.b), to_u8(c.a))
}

/// Reverse of [`shroud_to_cosmic`] — extract a shroud `Color` back out of a
/// cosmic-text glyph color so the renderer can use its existing per-glyph
/// color path. The conversion is round-trip-stable for the bit depths shroud
/// passes through (8-bit per channel).
pub(crate) fn cosmic_to_shroud(c: cosmic_text::Color) -> Color {
    Color::rgba(
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
        c.a() as f32 / 255.0,
    )
}
