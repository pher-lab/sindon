//! Regression: the measure-only path must report exactly what shaping does.
//!
//! `Widget::measure` runs several times per widget per frame (Taffy probes
//! min-content and max-content before the final pass) and only ever needs the
//! box, but it used to get it from `shape_text_attrs`, which returns
//! `ShapedText` **by value** — so every one of those calls cloned the whole
//! glyph vector. On a text-heavy tree that was ~90% of the layout pass: a
//! 256-paragraph preview cost 2.34 ms per layout, against 234 µs once measure
//! stopped cloning.
//!
//! `measure_text_attrs` / `measure_rich` are the non-cloning twins. They must
//! stay bit-identical to the shaping path, because layout sizes the box from
//! one and paint wraps the glyphs with the other: any disagreement shows up as
//! text wrapping differently than the space reserved for it (the class of bug
//! the `ceil()` comments in `TextWidget::measure` were written for).

use sindon_core::Color;
use sindon_text::{TextAttrs, TextEngine, TextSpan};

const FS: f32 = 16.0;
const LH: f32 = 19.2;
const PROSE: &str = "The quick brown fox jumps over the lazy dog and keeps \
going until this line has to wrap somewhere sensible.";

#[test]
fn measure_matches_shape_for_every_wrap_width() {
    let attrs = TextAttrs::default();
    // Unconstrained, wide enough not to wrap, and narrow enough to wrap
    // several times — the three regimes `measure` is asked about.
    for wrap in [None, Some(2_000.0), Some(300.0), Some(90.0)] {
        let mut engine = TextEngine::new();
        let shaped = engine.shape_text_attrs(PROSE, FS, LH, wrap, &attrs);
        let measured = engine.measure_text_attrs(PROSE, FS, LH, wrap, &attrs);
        assert_eq!(
            (shaped.width, shaped.height),
            measured,
            "wrap = {wrap:?} disagreed"
        );
    }
}

#[test]
fn measure_agrees_whether_it_hits_or_misses_the_cache() {
    let attrs = TextAttrs::default();
    let wrap = Some(300.0);

    // Cold engine: this call misses and has to shape.
    let mut cold = TextEngine::new();
    let on_miss = cold.measure_text_attrs(PROSE, FS, LH, wrap, &attrs);

    // Warm engine: the same query, already cached by a shaping call.
    let mut warm = TextEngine::new();
    warm.shape_text_attrs(PROSE, FS, LH, wrap, &attrs);
    let on_hit = warm.measure_text_attrs(PROSE, FS, LH, wrap, &attrs);

    assert_eq!(on_miss, on_hit);
}

#[test]
fn a_measure_miss_leaves_the_glyphs_cached_for_the_paint_that_follows() {
    // The frame order is measure-then-paint, so a measure that had to shape
    // must populate the cache — otherwise sizing a widget would double the
    // shaping work instead of front-loading it.
    let attrs = TextAttrs::default();
    let mut engine = TextEngine::new();

    engine.measure_text_attrs(PROSE, FS, LH, Some(300.0), &attrs);
    let (shapes_after_measure, _) = engine.take_shape_stats();
    assert_eq!(shapes_after_measure, 1, "measure had to shape once");

    let shaped = engine.shape_text_attrs(PROSE, FS, LH, Some(300.0), &attrs);
    let (shapes_after_paint, _) = engine.take_shape_stats();
    assert_eq!(shapes_after_paint, 0, "paint reused the measure's shaping");
    assert!(!shaped.glyphs.is_empty());
}

#[test]
fn repeated_measures_never_reshape() {
    // The whole point: Taffy's probing is free after the first shape.
    let attrs = TextAttrs::default();
    let mut engine = TextEngine::new();
    engine.measure_text_attrs(PROSE, FS, LH, Some(300.0), &attrs);
    let _ = engine.take_shape_stats();

    for _ in 0..10 {
        engine.measure_text_attrs(PROSE, FS, LH, Some(300.0), &attrs);
    }
    let (shapes, _) = engine.take_shape_stats();
    assert_eq!(shapes, 0);
}

#[test]
fn measure_rich_matches_shape_rich() {
    let spans = vec![
        TextSpan::new("A bold lead-in ").color(Color::WHITE),
        TextSpan::new(PROSE),
    ];
    for wrap in [None, Some(400.0), Some(120.0)] {
        let mut engine = TextEngine::new();
        let shaped = engine.shape_rich(&spans, FS, LH, wrap);
        let measured = engine.measure_rich(&spans, FS, LH, wrap);
        assert_eq!(
            (shaped.width, shaped.height),
            measured,
            "rich wrap = {wrap:?} disagreed"
        );
    }
}

#[test]
fn measure_text_and_measure_text_attrs_agree_on_default_attrs() {
    let mut engine = TextEngine::new();
    let plain = engine.measure_text(PROSE, FS, LH, Some(300.0));
    let with_attrs = engine.measure_text_attrs(PROSE, FS, LH, Some(300.0), &TextAttrs::default());
    assert_eq!(plain, with_attrs);
}
