//! SecureText widget — renders text from a SecureString.
//!
//! Text content is accessed only via `expose()` during paint.
//! The shaped glyph data lives only for the duration of the paint call.

use crate::paint::PaintContext;
use crate::widget::Widget;
use sindon_core::{AccessNode, AccessRole, Color, Rect, SecurityLevel};
use sindon_layout::FlexStyle;
use sindon_security::SecureString;

/// A text widget for sensitive data.
///
/// Unlike `TextWidget`, this widget:
/// - Reports `SecurityLevel::Sensitive`
/// - Accesses text via closure (`expose`) — content never escapes
/// - Text is stored in a `SecureString` that zeroizes on drop
///
/// The widget stores a function that provides the text on demand,
/// enabling both static `SecureString` references and reactive
/// `SecureSignal<SecureString>` usage.
type TextFn = Box<dyn Fn(&mut dyn FnMut(&str))>;

pub struct SecureText {
    /// Closure that provides the text content for shaping.
    text_fn: TextFn,
    font_size: Option<f32>,
    line_height: Option<f32>,
    color: Option<Color>,
}

impl SecureText {
    /// Create from a `SecureString` (cloned into a closure).
    ///
    /// Note: the SecureString is moved into this widget and accessed
    /// via closure during paint only.
    pub fn new(text: SecureString) -> Self {
        Self {
            text_fn: Box::new(move |f| text.expose(|s| f(s))),
            font_size: None,
            line_height: None,
            color: None,
        }
    }

    /// Create from a reactive `SecureSignal<SecureString>`.
    ///
    /// The signal is read during each paint call, providing
    /// automatic reactivity.
    pub fn from_signal(signal: sindon_reactive::SecureSignal<SecureString>) -> Self {
        Self {
            text_fn: Box::new(move |f| signal.expose(|s| s.expose(|text| f(text)))),
            font_size: None,
            line_height: None,
            color: None,
        }
    }

    /// Create from any closure that provides text access.
    pub fn from_fn(text_fn: impl Fn(&mut dyn FnMut(&str)) + 'static) -> Self {
        Self {
            text_fn: Box::new(text_fn),
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
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

impl Widget for SecureText {
    fn security_level(&self) -> SecurityLevel {
        SecurityLevel::Sensitive
    }

    fn accessibility(&self) -> Option<AccessNode> {
        // Displays a secret (a decrypted note, a revealed field). Exposed as a
        // protected, value-less label: a screen reader learns *that* protected
        // content is here, never *what* it says. The buffer is never read.
        Some(
            AccessNode::new(AccessRole::Label)
                .name("Protected content")
                .protected(),
        )
    }

    fn style(&self) -> FlexStyle {
        let line_height = self.line_height.unwrap_or(22.0);
        FlexStyle::new().min_height(line_height)
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let line_height = self
            .line_height
            .unwrap_or(ctx.theme.typography.body.line_height);
        let color = self.color.unwrap_or(ctx.theme.colors.on_background);
        let layout_width = layout.size.width;
        let origin_x = layout.origin.x;
        let origin_y = layout.origin.y;

        (self.text_fn)(&mut |text: &str| {
            if text.is_empty() {
                return;
            }

            // Uncached: the shaped glyphs encode the revealed secret, so they
            // must never be retained in the shape cache (unlike masked /
            // note-body text, which is cacheable).
            let shaped = ctx.text_engine.shape_text_uncached(
                text,
                font_size,
                line_height,
                Some(layout_width),
            );

            for glyph in &shaped.glyphs {
                if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                    ctx.draw_secure_glyph(
                        origin_x + glyph.x,
                        origin_y + glyph.y,
                        image,
                        color,
                        glyph.cache_key,
                    );
                }
            }
        });
    }
}
