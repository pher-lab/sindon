//! sindon_text — Text shaping and rasterization via cosmic-text.
//!
//! Wraps `FontSystem` + `SwashCache` into a `TextEngine` that provides:
//! - Text shaping (string → positioned glyphs)
//! - Glyph rasterization (glyph → alpha mask)
//!
//! ## Secret residue
//!
//! Shaping runs through a vendored fork of cosmic-text (`third_party/cosmic-text`)
//! whose `BufferLine` holds its text in a `Zeroizing<String>` — the one owned
//! plaintext copy cosmic-text keeps, since shaped glyphs carry only byte offsets.
//! It is wiped when the shaped buffer is dropped, so any text shaped through the
//! engine — including `SecureText`'s transient reveal — leaves no un-zeroed
//! plaintext residue on the heap. Verified end-to-end by `tests/cosmic_residue.rs`
//! (`final_residue == 0`), which is compiled and run on the Windows CI job.

mod attrs;
mod engine;
mod span;

pub use attrs::{FontStyle, FontWeight, TextAttrs, TextFamily};
pub use engine::{
    ComposedBlock, DecorationLine, EditBuffer, GlyphImage, ShapedGlyph, ShapedText, SpanBox,
    TextEngine,
};
pub use span::{TextDecoration, TextSpan};

// Re-export cosmic-text types that consumers need
pub use cosmic_text::{Attrs, CacheKey, FontSystem, Metrics, Shaping};
