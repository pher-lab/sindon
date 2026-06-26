//! `TextEngine::load_font_data` registers an in-memory font whose family then
//! resolves by name in shaping (FW-12 — the mechanism behind bundled icon
//! fonts). The fixture is a ~2.5 KB subset of the Material Design Icons webfont
//! (Apache 2.0), the same family Knot bundles for its toolbar icons.

use shroud_text::{TextAttrs, TextEngine, TextFamily};

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

#[test]
fn load_font_data_rejects_garbage() {
    let mut engine = TextEngine::new();
    let families = engine.load_font_data(b"this is not a font");
    assert!(
        families.is_empty(),
        "unparseable bytes must register no families"
    );
}
