//! `TextEngine::load_font_data` registers an in-memory font whose family then
//! resolves by name in shaping (FW-12 — the mechanism behind bundled icon
//! fonts). The fixture is a ~2.5 KB subset of the Material Design Icons webfont
//! (Apache 2.0), the same family Knot bundles for its toolbar icons.

use sindon_text::{TextAttrs, TextEngine, TextFamily};

const ICON_FONT: &[u8] = include_bytes!("assets/icons-subset.ttf");

/// A private-use codepoint the fixture carries a glyph for (MDI `format-bold`).
const ICON_GLYPH: &str = "\u{F0264}";

#[test]
fn load_font_data_returns_family_and_resolves_by_name() {
    let mut engine = TextEngine::new();

    let families = engine.load_font_data(ICON_FONT);
    assert!(
        !families.is_empty(),
        "a valid font must report at least one family name"
    );

    // The reported family name is usable as `TextFamily::Named`: shaping the
    // icon codepoint in it produces a real, positioned glyph (with advance).
    let attrs = TextAttrs::default().family(TextFamily::Named(families[0].clone()));
    let shaped = engine.shape_text_attrs(ICON_GLYPH, 32.0, 38.0, None, &attrs);
    assert!(
        !shaped.glyphs.is_empty(),
        "registered icon family must shape the icon codepoint to a glyph"
    );
    assert!(
        shaped.width > 0.0,
        "the icon glyph must carry a non-zero advance"
    );
}

/// `set_default_font_family` redefines what generic / unstyled text resolves
/// to: after the swap, *default* attrs (`TextFamily::SansSerif` — what every
/// widget carries unless it calls `.family(..)`) shape the same glyph, with the
/// same advance, as an explicit `Named` request for that family. The fixture's
/// icon codepoint lives in a private-use block no system font provides, so the
/// equivalence pins the remap itself rather than any host-font coincidence.
#[test]
fn set_default_font_family_routes_unstyled_text_to_that_family() {
    let mut engine = TextEngine::new();
    let families = engine.load_font_data(ICON_FONT);
    let family = families
        .first()
        .expect("fixture must report a family")
        .clone();

    // Reference: explicitly request the fixture family by name.
    let named = TextAttrs::default().family(TextFamily::Named(family.clone()));
    let reference = engine.shape_text_attrs(ICON_GLYPH, 32.0, 38.0, None, &named);
    assert!(
        !reference.glyphs.is_empty() && reference.width > 0.0,
        "named fixture family must shape the icon glyph"
    );

    // Point the sans-serif generic at the fixture, then shape with *default*
    // attrs (no `.family(..)`). It must land on the same face → same glyphs.
    engine.set_default_font_family(&family);
    let defaulted = engine.shape_text_attrs(ICON_GLYPH, 32.0, 38.0, None, &TextAttrs::default());
    assert_eq!(
        defaulted.glyphs.len(),
        reference.glyphs.len(),
        "remapped default must resolve the same glyph run as the named family"
    );
    assert!(
        (defaulted.width - reference.width).abs() < 1e-3,
        "remapped default must carry the named family's advance (got {}, want {})",
        defaulted.width,
        reference.width
    );
}

#[test]
fn load_font_data_rejects_garbage() {
    let mut engine = TextEngine::new();
    let families = engine.load_font_data(b"this is not a font");
    assert!(
        families.is_empty(),
        "unparseable bytes must register no families"
    );
}
