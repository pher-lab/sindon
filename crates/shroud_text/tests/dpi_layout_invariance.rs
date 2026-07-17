//! DPI: rasterization-only proof.
//!
//! Text is made crisp on a HiDPI display by rasterizing each glyph at the
//! device's resolution — *not* by shaping the paragraph differently. shroud
//! shapes in logical units always, and hands the scale factor only to
//! `LayoutGlyph::physical`, which snaps placement to the physical pixel grid and
//! stamps `font_size * scale` into the cache key the rasterizer reads.
//!
//! That split is load-bearing rather than tidy. `Input` derives its caret,
//! selection and click-target geometry from the same shaped output the painter
//! consumes (the premise `highlight_layout_spike` pins from the other side), so
//! if the scale factor could reach the *layout* numbers, every caret in the app
//! would drift the moment the user dragged the window to a monitor with
//! different scaling — and it would drift only there, which is the kind of bug
//! that gets reported as "sometimes the cursor is in the wrong place".
//!
//! The invariant, then: at any scale — same width, same height, same glyph
//! count, different bitmaps.

use shroud_text::TextEngine;

/// Mixed scripts on purpose: CJK resolves a different fallback font, so a
/// scale-dependent shaping bug would show up here and not in ASCII-only text.
const TEXT: &str = "The quick brown fox — 素早い茶色の狐が跳ぶ";

#[test]
fn scale_leaves_layout_identical() {
    let mut engine = TextEngine::new();
    let at_100 = engine.shape_text(TEXT, 16.0, 20.0, Some(400.0));

    engine.set_scale(2.0);
    let at_200 = engine.shape_text(TEXT, 16.0, 20.0, Some(400.0));

    assert_eq!(
        at_200.width, at_100.width,
        "shaping must not depend on the display scale — if it did, a DPI change \
         would re-wrap the paragraph under the caret"
    );
    assert_eq!(
        at_200.height, at_100.height,
        "line count must not move either"
    );
    assert_eq!(
        at_200.glyphs.len(),
        at_100.glyphs.len(),
        "the same text must produce the same glyphs at any scale"
    );
}

#[test]
fn placement_stays_logical_across_scales() {
    let mut engine = TextEngine::new();
    let at_100 = engine.shape_text(TEXT, 16.0, 20.0, None);
    let logical_xs: Vec<f32> = at_100.glyphs.iter().map(|g| g.x).collect();

    engine.set_scale(2.0);
    let at_200 = engine.shape_text(TEXT, 16.0, 20.0, None);

    for (glyph, logical_x) in at_200.glyphs.iter().zip(&logical_xs) {
        // Same logical spot: the scale buys a finer grid to snap onto, not a
        // different position. Callers add a *logical* origin to this number, so
        // a scale-dependent value here would drag every glyph away from its
        // widget — the whole run slides toward the origin, which is precisely
        // the bug an earlier cut of this slice shipped.
        //
        // Tolerance is one physical pixel expressed logically (½ at 2x): the two
        // runs snap to grids of different fineness, so they may land on
        // neighbouring subdivisions of the same spot.
        assert!(
            (glyph.x - logical_x).abs() <= 0.5,
            "glyph sat at logical x={logical_x} at 100% but x={} at 200%; \
             the display scale must not move text in logical space",
            glyph.x
        );
    }
}

#[test]
fn the_baseline_stays_put_across_scales() {
    // The y twin of the test above, and not redundant with it: glyph placement
    // is `glyph_pos * scale + offset`, and only the *offset* carries the
    // baseline. At x the offset is zero, so an unscaled offset is invisible
    // there — this axis is the only one that can catch it. It escaped once: the
    // baseline was passed logical while the glyph was scaled, so dividing back
    // out halved the baseline and slid every line ~7px up its own box. Anything
    // more than a pixel of drift here is that bug returning.
    let mut engine = TextEngine::new();
    let at_100 = engine.shape_text(TEXT, 16.0, 20.0, None);
    let logical_ys: Vec<f32> = at_100.glyphs.iter().map(|g| g.y).collect();

    engine.set_scale(2.0);
    let at_200 = engine.shape_text(TEXT, 16.0, 20.0, None);

    for (glyph, logical_y) in at_200.glyphs.iter().zip(&logical_ys) {
        assert!(
            (glyph.y - logical_y).abs() <= 0.5,
            "baseline sat at logical y={logical_y} at 100% but y={} at 200%; \
             the offset passed to `physical()` must be pre-scaled or text \
             drifts vertically with the display scale",
            glyph.y
        );
    }
}

#[test]
fn scale_selects_a_different_rasterization() {
    let mut engine = TextEngine::new();
    let at_100 = engine.shape_text(TEXT, 16.0, 20.0, None);
    let key_at_100 = at_100.glyphs[0].cache_key;

    engine.set_scale(2.0);
    let at_200 = engine.shape_text(TEXT, 16.0, 20.0, None);

    // Two failures at once, which is why this assert is worth its own test:
    // the cache key must carry the scaled size (or the rasterizer reuses the
    // 100% bitmap and text stays soft), *and* the shape cache must be keyed by
    // scale (or this second call is served the first call's entry and never
    // reaches the rasterizer at all).
    assert_ne!(
        at_200.glyphs[0].cache_key, key_at_100,
        "glyph cache keys must differ across scales, or HiDPI text is a \
         stretched 100% bitmap"
    );
}
