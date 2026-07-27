//! Bundled icon font + a typed `Icon` → codepoint mapping (FW-12).
//!
//! sindon has no vector / SVG drawing primitive, but it can register a font at
//! startup and draw any glyph through the ordinary text path
//! (`App::font(..)` + `TextFamily::Named(..)`). Knot bundles a tiny subset of
//! the Material Design Icons webfont — only the glyphs it actually draws — and
//! maps each to a typed [`Icon`] so call sites say `Icon::Bold` instead of a
//! bare private-use codepoint.
//!
//! The subset font lives at `assets/knot-icons.ttf` and is redistributed under
//! the Apache 2.0 license (see `assets/ICON-FONT-LICENSE.txt`). It was produced
//! from `@mdi/font@7.4.47`'s `materialdesignicons-webfont.ttf` with
//! `pyftsubset --unicodes=<the codepoints below> --name-IDs='*'`. A few glyphs
//! beyond the current [`Icon`] variants are kept in the subset so upcoming
//! sidebar / settings wiring can add variants without re-subsetting.

use sindon::text::TextFamily;
use sindon::widgets::Button;

/// Raw bytes of the bundled icon font. Registered once at startup with
/// `App::font(icons::FONT)` (see `main`).
pub const FONT: &[u8] = include_bytes!("../assets/knot-icons.ttf");

/// The font's family name (name-table ID 1), used to target it with
/// [`TextFamily::Named`]. Matches what
/// `TextEngine::load_font_data` reports for this file.
pub const FAMILY: &str = "Material Design Icons";

/// An icon Knot draws. Each maps to one codepoint in [`FONT`]. Add a variant
/// (and its arm in [`Icon::codepoint`]) when wiring a new call site; the glyph
/// must also be present in the subset (`assets/knot-icons.ttf`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Heading,
    Bold,
    Italic,
    Code,
    Quote,
    List,
    Link,
}

impl Icon {
    /// The private-use codepoint of this icon within the bundled font.
    pub fn codepoint(self) -> char {
        match self {
            Icon::Heading => '\u{F026B}', // format-header-1
            Icon::Bold => '\u{F0264}',    // format-bold
            Icon::Italic => '\u{F0277}',  // format-italic
            Icon::Code => '\u{F0174}',    // code-tags
            Icon::Quote => '\u{F027E}',   // format-quote-close
            Icon::List => '\u{F0279}',    // format-list-bulleted
            Icon::Link => '\u{F0339}',    // link-variant
        }
    }
}

/// Build an icon button: a [`Button`] whose label is `icon`'s glyph, shaped in
/// the bundled icon family at `size` px. The caller chains `.on_click(..)`,
/// `.radius(..)`, colors, etc. as on any button.
pub fn icon_button(icon: Icon, size: f32) -> Button {
    Button::new(icon.codepoint().to_string())
        .family(TextFamily::Named(FAMILY.to_string()))
        .font_size(size)
}
