//! Text attributes — font family, weight, and slant.
//!
//! Wraps the subset of `cosmic_text::Attrs` that shroud widgets expose,
//! converting cosmic-text's borrowed `Family<'a>` into an owned `TextFamily`
//! so widgets can hold attrs in struct fields without lifetime gymnastics.

pub use cosmic_text::{Style as FontStyle, Weight as FontWeight};

/// Font family selector.
///
/// Generic variants (`Serif`, `SansSerif`, `Monospace`, `Cursive`, `Fantasy`)
/// map to cosmic-text's generic families and resolve against whatever the
/// platform considers a font of that class. `Named` picks a specific family by
/// name (e.g. `"Noto Sans JP"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TextFamily {
    Serif,
    SansSerif,
    Monospace,
    Cursive,
    Fantasy,
    /// A specific font family by name. Falls back to the platform default if
    /// the named family is not installed.
    Named(String),
}

impl Default for TextFamily {
    fn default() -> Self {
        Self::SansSerif
    }
}

impl TextFamily {
    /// Borrow as cosmic-text's family enum. `Named` borrows the inner string.
    pub(crate) fn as_cosmic(&self) -> cosmic_text::Family<'_> {
        match self {
            Self::Serif => cosmic_text::Family::Serif,
            Self::SansSerif => cosmic_text::Family::SansSerif,
            Self::Monospace => cosmic_text::Family::Monospace,
            Self::Cursive => cosmic_text::Family::Cursive,
            Self::Fantasy => cosmic_text::Family::Fantasy,
            Self::Named(name) => cosmic_text::Family::Name(name.as_str()),
        }
    }
}

/// Owned attributes applied during text shaping.
///
/// All shaping calls funnel through this struct so widgets can carry a single
/// `TextAttrs` field instead of juggling weight/style/family separately. The
/// default (`SansSerif`, `NORMAL`, `Normal`) matches cosmic-text's `Attrs::new()`
/// — i.e. what shroud used everywhere before Phase 33.
#[derive(Debug, Clone, PartialEq)]
pub struct TextAttrs {
    pub family: TextFamily,
    pub weight: FontWeight,
    pub style: FontStyle,
}

impl Default for TextAttrs {
    fn default() -> Self {
        Self {
            family: TextFamily::default(),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }
}

impl TextAttrs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn family(mut self, family: TextFamily) -> Self {
        self.family = family;
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = weight;
        self
    }

    pub fn style(mut self, style: FontStyle) -> Self {
        self.style = style;
        self
    }

    /// Convert into the cosmic-text `Attrs` value used by the shaper. Borrows
    /// from `self` for the family name, so the returned `Attrs` is bound by
    /// `self`'s lifetime.
    pub(crate) fn as_cosmic(&self) -> cosmic_text::Attrs<'_> {
        cosmic_text::Attrs::new()
            .family(self.family.as_cosmic())
            .weight(self.weight)
            .style(self.style)
    }
}
