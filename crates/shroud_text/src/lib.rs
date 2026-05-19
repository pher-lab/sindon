//! shroud_text — Text shaping and rasterization via cosmic-text.
//!
//! Wraps `FontSystem` + `SwashCache` into a `TextEngine` that provides:
//! - Text shaping (string → positioned glyphs)
//! - Glyph rasterization (glyph → alpha mask)
//!
//! For secure text, `SecureTextBuffer` zeroizes internal buffers on drop.

mod attrs;
mod engine;
mod span;

pub use attrs::{FontStyle, FontWeight, TextAttrs, TextFamily};
pub use engine::{GlyphImage, ShapedGlyph, ShapedText, TextEngine};
pub use span::TextSpan;

// Re-export cosmic-text types that consumers need
pub use cosmic_text::{Attrs, CacheKey, FontSystem, Metrics, Shaping};
