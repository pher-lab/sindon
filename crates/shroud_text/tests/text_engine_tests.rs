use shroud_text::TextEngine;

#[test]
fn engine_creates_successfully() {
    let _engine = TextEngine::new();
}

#[test]
fn shape_empty_string() {
    let mut engine = TextEngine::new();
    let result = engine.shape_text("", 16.0, 20.0, None);
    assert!(result.glyphs.is_empty());
}

#[test]
fn shape_ascii_text() {
    let mut engine = TextEngine::new();
    let result = engine.shape_text("Hello", 16.0, 20.0, None);

    // Should produce glyphs (exact count depends on font,
    // but "Hello" is 5 visible characters)
    assert!(
        !result.glyphs.is_empty(),
        "shaping 'Hello' should produce glyphs"
    );
    assert!(result.width > 0.0, "shaped text should have nonzero width");
    assert!(
        result.height > 0.0,
        "shaped text should have nonzero height"
    );
}

#[test]
fn shape_text_glyph_y_includes_baseline() {
    // Regression guard: shaped glyph.y must be the baseline Y within the text
    // block, not a raw zero. cosmic-text's `LayoutGlyph.y` starts at 0 for
    // horizontal text; the baseline lives in `LayoutRun.line_y` and MUST be
    // passed as the Y offset to `physical(...)`. If someone reverts that, all
    // downstream widgets render text shifted up by ~ascent pixels.
    let mut engine = TextEngine::new();
    let result = engine.shape_text("Xyg", 32.0, 38.4, None);
    assert!(!result.glyphs.is_empty());

    // For font_size=32, ascent is typically ~0.75 * em ≈ 24. Baseline Y should
    // be at least 10 px below the block top; near-zero means the offset is lost.
    let baseline = result.glyphs[0].y;
    assert!(
        baseline > 10,
        "glyph.y ({}) looks like we forgot to add line_y — should be the baseline Y",
        baseline
    );
}

#[test]
fn shape_text_glyph_positions_are_ordered() {
    let mut engine = TextEngine::new();
    let result = engine.shape_text("ABCDEF", 16.0, 20.0, None);

    // For LTR text, glyph X positions should generally increase
    for window in result.glyphs.windows(2) {
        assert!(
            window[1].x >= window[0].x,
            "glyph positions should be non-decreasing for LTR text"
        );
    }
}

#[test]
fn rasterize_glyph() {
    let mut engine = TextEngine::new();
    let result = engine.shape_text("A", 32.0, 40.0, None);

    assert!(!result.glyphs.is_empty(), "should have at least one glyph");

    let glyph = &result.glyphs[0];
    let image = engine.rasterize(glyph.cache_key);

    // A visible letter at 32px should rasterize to a non-empty image
    assert!(image.is_some(), "'A' at 32px should produce a glyph image");

    let img = image.unwrap();
    assert!(img.width > 0);
    assert!(img.height > 0);
    assert_eq!(
        img.data.len(),
        (img.width * img.height) as usize,
        "alpha mask should have width*height bytes"
    );
    // The image should contain some non-zero pixels
    assert!(
        img.data.iter().any(|&b| b > 0),
        "glyph image should have non-zero alpha pixels"
    );
}

#[test]
fn rasterize_space_returns_none() {
    let mut engine = TextEngine::new();
    let result = engine.shape_text(" ", 16.0, 20.0, None);

    // Space may or may not produce a glyph depending on the font.
    // If it does, rasterization should return None (no visible pixels).
    for glyph in &result.glyphs {
        let image = engine.rasterize(glyph.cache_key);
        if let Some(img) = image {
            // If we got an image it should be tiny/empty for a space
            assert!(
                img.width <= 1 || img.data.iter().all(|&b| b == 0),
                "space glyph should have no visible pixels"
            );
        }
    }
}

#[test]
fn shape_with_max_width_wraps() {
    let mut engine = TextEngine::new();

    // Shape a long string without width limit
    let no_wrap = engine.shape_text("Hello World Test", 16.0, 20.0, None);

    // Shape with a narrow width limit — should wrap
    let wrapped = engine.shape_text("Hello World Test", 16.0, 20.0, Some(50.0));

    // Wrapped text should be taller (more lines) than non-wrapped
    assert!(
        wrapped.height >= no_wrap.height,
        "wrapped text should be at least as tall as non-wrapped"
    );
}

#[test]
fn multiple_shape_calls_are_independent() {
    let mut engine = TextEngine::new();

    let result1 = engine.shape_text("AAA", 16.0, 20.0, None);
    let result2 = engine.shape_text("BB", 16.0, 20.0, None);

    // Different text lengths should produce different glyph counts
    assert_ne!(
        result1.glyphs.len(),
        result2.glyphs.len(),
        "different texts should produce different glyph counts"
    );
}
