use shroud_core::Color;
use shroud_text::{FontWeight, TextAttrs, TextEngine, TextFamily, TextSpan};

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
fn baseline_is_independent_of_script_mix() {
    // Regression for the cosmic-text per-line centering jitter: cosmic derives
    // the baseline from each line's real fonts' ascent/descent, so an
    // ASCII-only line and a CJK-containing line landed on different baselines —
    // the brackets in `[a]` sat lower than in `[あ]`. We override with a fixed
    // baseline, so the shared leading `[` glyph must land at the same y in both.
    //
    // On a runner with no CJK font, `あ` renders as `.notdef` from the *same*
    // default font, so the inputs coincide and this still passes (it just can't
    // catch a regression there) — it never fails spuriously.
    let mut engine = TextEngine::new();
    let ascii = engine.shape_text("[a]", 24.0, 30.0, None);
    let cjk = engine.shape_text("[\u{3042}]", 24.0, 30.0, None); // [あ]
    assert!(!ascii.glyphs.is_empty() && !cjk.glyphs.is_empty());
    assert_eq!(
        ascii.glyphs[0].y, cjk.glyphs[0].y,
        "the leading '[' must share one baseline regardless of script mix \
         (ascii {} vs cjk {})",
        ascii.glyphs[0].y, cjk.glyphs[0].y
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
    assert!(!img.is_color, "a Latin letter is a monochrome alpha mask");
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
fn rasterize_color_emoji_keeps_rgba() {
    // A color emoji must come back as RGBA (4 bytes/pixel) tagged `is_color`,
    // not an alpha mask — that's what stops it painting as a solid white
    // silhouette. Color-emoji fonts aren't guaranteed in every environment, so
    // this is a best-effort guard: if *any* shaped glyph rasterizes as color,
    // its data must be tightly-packed RGBA. (On Windows, Segoe UI Emoji makes
    // this fire for real.)
    let mut engine = TextEngine::new();
    let result = engine.shape_text("😀", 32.0, 40.0, None);

    for glyph in &result.glyphs {
        if let Some(img) = engine.rasterize(glyph.cache_key) {
            if img.is_color {
                assert_eq!(
                    img.data.len(),
                    (img.width * img.height * 4) as usize,
                    "a color glyph must carry width*height*4 RGBA bytes"
                );
                return;
            }
        }
    }
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
fn cursor_position_empty_prefix_is_origin() {
    let mut engine = TextEngine::new();
    let (x, y) = engine.cursor_position("", 16.0, 20.0, None);
    assert_eq!(x, 0.0);
    assert_eq!(y, 0.0);
}

#[test]
fn cursor_position_grows_along_x_for_single_line() {
    let mut engine = TextEngine::new();
    let (x_a, y_a) = engine.cursor_position("A", 16.0, 20.0, None);
    let (x_abc, y_abc) = engine.cursor_position("ABC", 16.0, 20.0, None);
    assert!(x_abc > x_a, "longer prefix should sit further right");
    assert_eq!(y_a, y_abc, "single-line cursor stays on the same line");
}

#[test]
fn cursor_position_after_single_newline_does_not_double_count_line_height() {
    // Regression: an earlier version added `line_height` itself on top of
    // the trailing empty BufferLine's `line_top`, landing the cursor on
    // line 3 instead of line 2 for "abc\n". Visible in textarea_demo as
    // "press Enter, caret jumps an extra line".
    let mut engine = TextEngine::new();
    let (x, y) = engine.cursor_position("abc\n", 16.0, 19.2, None);
    assert_eq!(x, 0.0, "cursor x should sit at left margin after a newline");
    assert!(
        (y - 19.2).abs() < 0.5,
        "cursor y must be exactly one line_height after one '\\n', got {y}"
    );
}

#[test]
fn cursor_position_after_hard_break_lands_on_next_line() {
    let mut engine = TextEngine::new();
    let (x_pre, _y_pre) = engine.cursor_position("foo", 16.0, 20.0, None);
    let (x_after, y_after) = engine.cursor_position("foo\n", 16.0, 20.0, None);

    assert_eq!(
        x_after, 0.0,
        "cursor after a hard break starts at line origin"
    );
    assert!(
        y_after > 0.0,
        "cursor after a hard break sits below the first line"
    );
    // Sanity: the prefix sat to the right of origin, the post-\n cursor is at origin.
    assert!(x_pre > 0.0);
}

#[test]
fn cursor_position_soft_wraps_into_extra_lines() {
    let mut engine = TextEngine::new();
    // Long enough to exceed a narrow max_width.
    let (_x_no_wrap, y_no_wrap) = engine.cursor_position("Hello World Test", 16.0, 20.0, None);
    let (_x_wrapped, y_wrapped) =
        engine.cursor_position("Hello World Test", 16.0, 20.0, Some(50.0));

    assert!(
        y_wrapped > y_no_wrap,
        "cursor wrapped past max_width should land on a lower line"
    );
}

#[test]
fn caret_at_offset_places_wrap_boundary_on_the_next_row() {
    // The offset that begins the second *visual* row of a soft-wrapped block
    // must place the caret at the start of that row — not at the end of the
    // previous one (the off-by-one-row that prefix shaping produces). This is
    // the FW-2 fix: vertical nav / clicks land on wrapped rows correctly.
    let mut engine = TextEngine::new();
    let text = "Hello World Test Here";
    let wrap = Some(60.0);
    let lh = 20.0;

    // Where row 2 begins: hit-test the far-left of the second row.
    let off = engine.offset_at_point(text, 0.0, lh + 1.0, 16.0, lh, wrap);
    assert!(
        off > 0 && off < text.len(),
        "the string must wrap so row 2 starts mid-string (got offset {off})"
    );

    let (cx, cy) = engine.caret_at_offset(text, off, 16.0, lh, wrap);
    assert!(
        cy >= lh - 0.5,
        "a wrap-boundary caret must sit on the next visual row: cy={cy}"
    );
    assert!(
        cx < 5.0,
        "a wrap-boundary caret must sit near the row's left edge: cx={cx}"
    );

    // Contrast: prefix shaping reports the *previous* row for the same offset —
    // exactly the bug `caret_at_offset` exists to avoid.
    let (_px, py) = engine.cursor_position(&text[..off], 16.0, lh, wrap);
    assert!(
        py < cy,
        "prefix shaping lands a row higher than the true caret row \
         (py={py} cy={cy}) — caret_at_offset must not"
    );
}

#[test]
fn caret_at_offset_matches_prefix_shaping_mid_line() {
    // Away from wrap boundaries, the offset caret agrees with prefix shaping
    // (and with cursor_position at the very end of the text).
    let mut engine = TextEngine::new();
    let text = "abcdef";
    let at3 = engine.caret_at_offset(text, 3, 16.0, 20.0, None);
    let pre3 = engine.cursor_position("abc", 16.0, 20.0, None);
    assert_eq!(at3, pre3, "mid-line caret must match prefix shaping");

    let at_end = engine.caret_at_offset(text, text.len(), 16.0, 20.0, None);
    let pre_end = engine.cursor_position(text, 16.0, 20.0, None);
    assert_eq!(
        at_end, pre_end,
        "end-of-text caret must match prefix shaping"
    );
}

#[test]
fn shape_text_delegates_to_shape_text_attrs_with_default() {
    // Regression guard: the no-attrs `shape_text` must produce byte-for-byte
    // the same glyph stream as `shape_text_attrs(.., &TextAttrs::default())`.
    // If someone "optimizes" `shape_text` to bypass attrs, every TextWidget
    // call that switched to `shape_text_attrs` in Phase 33 silently diverges.
    let mut engine = TextEngine::new();
    let plain = engine.shape_text("Hello, 世界", 16.0, 20.0, None);
    let attrs = engine.shape_text_attrs("Hello, 世界", 16.0, 20.0, None, &TextAttrs::default());

    assert_eq!(plain.glyphs.len(), attrs.glyphs.len());
    assert_eq!(plain.width, attrs.width);
    assert_eq!(plain.height, attrs.height);
    for (a, b) in plain.glyphs.iter().zip(attrs.glyphs.iter()) {
        assert_eq!(a.cache_key, b.cache_key);
        assert_eq!(a.x, b.x);
        assert_eq!(a.y, b.y);
    }
}

#[test]
fn cursor_position_delegates_to_cursor_position_attrs_with_default() {
    let mut engine = TextEngine::new();
    let plain = engine.cursor_position("abc\ndef", 16.0, 20.0, None);
    let attrs = engine.cursor_position_attrs("abc\ndef", 16.0, 20.0, None, &TextAttrs::default());
    assert_eq!(plain, attrs);
}

#[test]
fn offset_at_point_empty_text_is_zero() {
    let mut engine = TextEngine::new();
    assert_eq!(engine.offset_at_point("", 50.0, 0.0, 16.0, 20.0, None), 0);
}

#[test]
fn offset_at_point_left_edge_is_start() {
    // A click at (or left of) the text origin lands before the first glyph.
    let mut engine = TextEngine::new();
    assert_eq!(
        engine.offset_at_point("Hello", 0.0, 0.0, 16.0, 20.0, None),
        0
    );
    assert_eq!(
        engine.offset_at_point("Hello", -20.0, 0.0, 16.0, 20.0, None),
        0,
        "a click in the leading padding clamps to offset 0"
    );
}

#[test]
fn offset_at_point_far_right_is_end() {
    // A click well past the last glyph lands at end-of-text.
    let mut engine = TextEngine::new();
    let n = engine.offset_at_point("Hello", 10_000.0, 0.0, 16.0, 20.0, None);
    assert_eq!(n, "Hello".len());
}

#[test]
fn offset_at_point_round_trips_with_cursor_position() {
    // The two are inverses: feed the x reported by `cursor_position` for a
    // prefix back into `offset_at_point` and recover that prefix's length.
    let mut engine = TextEngine::new();
    let text = "Hello world";
    for prefix_len in [1usize, 3, 6, 9, text.len()] {
        let (x, y) = engine.cursor_position(&text[..prefix_len], 16.0, 20.0, None);
        let got = engine.offset_at_point(text, x, y, 16.0, 20.0, None);
        assert_eq!(
            got, prefix_len,
            "click at the caret x of prefix len {prefix_len} should map back to it (x={x})"
        );
    }
}

#[test]
fn offset_at_point_picks_the_clicked_hard_line() {
    // Multi-line: a click on the second line resolves to an offset past the
    // first line's newline.
    let mut engine = TextEngine::new();
    let text = "abc\ndef";
    // y near the top of the second line (one line_height down).
    let n = engine.offset_at_point(text, 0.0, 20.0, 16.0, 20.0, None);
    assert!(
        n >= 4,
        "click on line 2 should land at/after the newline (offset {n})"
    );
    assert!(n <= text.len());
}

#[test]
fn selection_rects_empty_or_inverted_range_is_empty() {
    let mut engine = TextEngine::new();
    assert!(
        engine
            .selection_rects("Hello", 2, 2, 16.0, 20.0, None)
            .is_empty(),
        "zero-width range produces no rects"
    );
    assert!(
        engine
            .selection_rects("Hello", 4, 1, 16.0, 20.0, None)
            .is_empty(),
        "inverted range produces no rects"
    );
    assert!(
        engine
            .selection_rects("", 0, 0, 16.0, 20.0, None)
            .is_empty()
    );
}

#[test]
fn selection_rects_full_single_line_spans_the_text_width() {
    // Selecting all of a single line yields one rect roughly as wide as the
    // shaped text.
    let mut engine = TextEngine::new();
    let text = "Hello";
    let shaped = engine.shape_text(text, 16.0, 20.0, None);
    let rects = engine.selection_rects(text, 0, text.len(), 16.0, 20.0, None);
    assert_eq!(rects.len(), 1, "single line → one rect, got {rects:?}");
    let r = rects[0];
    assert!(r.origin.y.abs() < 0.5, "single line sits at block top");
    assert!(
        (r.size.width - shaped.width).abs() < 2.0,
        "selection width {} should track text width {}",
        r.size.width,
        shaped.width
    );
}

#[test]
fn selection_rects_partial_is_narrower_than_full() {
    let mut engine = TextEngine::new();
    let text = "Hello world";
    let full = engine.selection_rects(text, 0, text.len(), 16.0, 20.0, None);
    let part = engine.selection_rects(text, 0, 5, 16.0, 20.0, None);
    assert_eq!(full.len(), 1);
    assert_eq!(part.len(), 1);
    assert!(
        part[0].size.width < full[0].size.width,
        "partial selection ({}) should be narrower than full ({})",
        part[0].size.width,
        full[0].size.width
    );
}

#[test]
fn selection_rects_across_newline_spans_two_lines() {
    // A range crossing a hard break produces a rect on each line, on distinct
    // rows.
    let mut engine = TextEngine::new();
    let text = "abc\ndef";
    let rects = engine.selection_rects(text, 1, 6, 16.0, 20.0, None);
    assert!(
        rects.len() >= 2,
        "selection across a newline should span >=2 lines, got {rects:?}"
    );
    let rows: std::collections::BTreeSet<i32> =
        rects.iter().map(|r| r.origin.y.round() as i32).collect();
    assert!(rows.len() >= 2, "rects should land on >=2 distinct rows");
}

#[test]
fn selection_rects_with_trailing_marks_included_line_break() {
    // FW-6: a multi-line selection should show that the line break is part of
    // the selection. The plain variant stops at each row's last glyph; the
    // trailing variant adds one sliver on every row whose selection continues.
    let mut engine = TextEngine::new();
    let text = "abc\ndef";
    let plain = engine.selection_rects(text, 0, text.len(), 16.0, 20.0, None);
    let trailing = engine.selection_rects_with_trailing(text, 0, text.len(), 16.0, 20.0, None);
    assert_eq!(
        trailing.len(),
        plain.len() + 1,
        "first line gains a trailing sliver, last line does not. plain={plain:?} trailing={trailing:?}"
    );
    // The extra rect sits on the top row, flush against the "abc" highlight's
    // right edge.
    let top: Vec<_> = trailing
        .iter()
        .filter(|r| r.origin.y.round() as i32 == 0)
        .collect();
    assert_eq!(top.len(), 2, "top row = highlight + sliver, got {top:?}");
    let hl = top
        .iter()
        .min_by(|a, b| a.origin.x.total_cmp(&b.origin.x))
        .unwrap();
    let sliver = top
        .iter()
        .max_by(|a, b| a.origin.x.total_cmp(&b.origin.x))
        .unwrap();
    assert!(
        sliver.origin.x >= hl.origin.x + hl.size.width - 0.5,
        "sliver starts at the highlight's right edge: hl={hl:?} sliver={sliver:?}"
    );
    assert!(sliver.size.width > 0.0, "sliver has width");
}

#[test]
fn selection_rects_with_trailing_no_sliver_when_selection_ends_text() {
    // A selection ending at the last glyph of the final row needs no sliver —
    // there is no following break to signal.
    let mut engine = TextEngine::new();
    let text = "abc";
    let plain = engine.selection_rects(text, 0, 3, 16.0, 20.0, None);
    let trailing = engine.selection_rects_with_trailing(text, 0, 3, 16.0, 20.0, None);
    assert_eq!(
        plain.len(),
        trailing.len(),
        "single fully-selected line gets no extra sliver"
    );
}

#[test]
fn selection_rects_with_trailing_reveals_selected_blank_line() {
    // A blank line mid-selection yields no glyph rect in the plain variant
    // (an invisible gap). The trailing variant draws a sliver on it so the
    // user sees the blank line is included.
    let mut engine = TextEngine::new();
    let text = "abc\n\ndef";
    let plain = engine.selection_rects(text, 0, text.len(), 16.0, 20.0, None);
    let trailing = engine.selection_rects_with_trailing(text, 0, text.len(), 16.0, 20.0, None);
    let plain_rows: std::collections::BTreeSet<i32> =
        plain.iter().map(|r| r.origin.y.round() as i32).collect();
    let trailing_rows: std::collections::BTreeSet<i32> =
        trailing.iter().map(|r| r.origin.y.round() as i32).collect();
    assert_eq!(
        plain_rows.len(),
        2,
        "plain skips the blank middle row, got {plain:?}"
    );
    assert_eq!(
        trailing_rows.len(),
        3,
        "trailing shows all three rows incl. blank, got {trailing:?}"
    );
}

#[test]
fn monospace_family_makes_iii_and_mmm_equal_width() {
    // The cosmic-text fontdb resolves the `Monospace` generic against
    // whatever monospace face is installed on the platform (Consolas /
    // DejaVu Sans Mono / Liberation Mono). All glyphs in that face advance
    // by the same width, so `iii` and `mmm` should shape to the same width.
    // In the default (proportional) family the ratio is ~0.3.
    let mut engine = TextEngine::new();
    let mono = TextAttrs::default().family(TextFamily::Monospace);
    let iii = engine.shape_text_attrs("iii", 16.0, 20.0, None, &mono);
    let mmm = engine.shape_text_attrs("mmm", 16.0, 20.0, None, &mono);

    assert!(iii.width > 0.0 && mmm.width > 0.0);
    let ratio = iii.width / mmm.width;
    assert!(
        ratio > 0.95 && ratio < 1.05,
        "monospace 'iii' / 'mmm' width ratio = {} (expected ~1.0)",
        ratio
    );
}

#[test]
fn proportional_family_makes_iii_much_narrower_than_mmm() {
    // Sanity-check baseline for the monospace test above. Without it the
    // monospace test might "pass" against a regressed shaper that simply
    // ignored attrs (since default fonts could happen to be ~equal width
    // for `iii` vs `mmm` on some test runner). This guards the contrast.
    let mut engine = TextEngine::new();
    let iii = engine.shape_text("iii", 16.0, 20.0, None);
    let mmm = engine.shape_text("mmm", 16.0, 20.0, None);
    assert!(
        iii.width < mmm.width * 0.6,
        "proportional 'iii' should be much narrower than 'mmm': {} vs {}",
        iii.width,
        mmm.width
    );
}

#[test]
fn bold_weight_reaches_the_shaper() {
    // The cosmic-text `CacheKey` includes the resolved font face id, so a
    // bold weight that actually reaches the shaper (and finds a bold variant
    // in the system's fallback font) produces a different cache key than the
    // normal-weight variant. If the system doesn't have a bold variant for
    // the resolved family, the shaper falls back to the same face — in which
    // case the cache keys match and the only signal we have is "didn't
    // panic". That fallthrough is acceptable: the plumbing test is the
    // delegation test above; this is a soft confirmation that the weight
    // field is not silently dropped on the path through `as_cosmic`.
    let mut engine = TextEngine::new();
    let normal = engine.shape_text_attrs("Ag", 32.0, 40.0, None, &TextAttrs::default());
    let bold = engine.shape_text_attrs(
        "Ag",
        32.0,
        40.0,
        None,
        &TextAttrs::default().weight(FontWeight::BOLD),
    );

    assert_eq!(normal.glyphs.len(), bold.glyphs.len());
    // Either the cache key differs (bold variant found) OR widths differ
    // (some shapers adjust advances even without a separate face) OR both
    // are equal (fallback). All three are valid outcomes; what is NOT
    // acceptable is a panic or empty glyph list.
    assert!(!bold.glyphs.is_empty());
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

#[test]
fn shape_rich_with_one_default_span_matches_shape_text() {
    // Sanity: rich path with a single default-attrs span should produce the
    // same glyph stream + extents as the plain shape_text path. If they
    // diverge, the rich path picked up some accidental Attrs default
    // (alignment, metrics, etc.) that shape_text doesn't.
    let mut engine = TextEngine::new();
    let plain = engine.shape_text("Hello", 16.0, 20.0, None);
    let rich = engine.shape_rich(&[TextSpan::new("Hello")], 16.0, 20.0, None);
    assert_eq!(plain.glyphs.len(), rich.glyphs.len());
    assert!(
        (plain.width - rich.width).abs() < 0.01,
        "width differs: plain={} rich={}",
        plain.width,
        rich.width
    );
    for (a, b) in plain.glyphs.iter().zip(rich.glyphs.iter()) {
        assert_eq!(a.cache_key, b.cache_key);
        assert_eq!(a.x, b.x);
        assert_eq!(a.y, b.y);
    }
}

#[test]
fn shape_rich_two_default_spans_total_width_matches_concat() {
    // Splitting "Hello" into ["Hel", "lo"] with the same default attrs
    // must shape to the same total width as "Hello" in one piece — i.e.
    // set_rich_text is not introducing inter-span padding.
    let mut engine = TextEngine::new();
    let one = engine.shape_text("Hello", 16.0, 20.0, None);
    let two = engine.shape_rich(
        &[TextSpan::new("Hel"), TextSpan::new("lo")],
        16.0,
        20.0,
        None,
    );
    assert!(
        (one.width - two.width).abs() < 0.5,
        "single 'Hello' width {} vs split rich width {}",
        one.width,
        two.width
    );
    assert_eq!(one.glyphs.len(), two.glyphs.len());
}

#[test]
fn shape_rich_with_span_color_propagates_to_glyphs() {
    // The whole point of plumbing color through `set_rich_text` is so a
    // per-span color override reaches the glyph. If this regresses,
    // markdown_demo's inline code stops being a different color from body.
    let mut engine = TextEngine::new();
    let red = Color::rgb(1.0, 0.0, 0.0);
    let shaped = engine.shape_rich(
        &[TextSpan::new("plain"), TextSpan::new("red").color(red)],
        16.0,
        20.0,
        None,
    );
    // The first ~5 glyphs are "plain" (no color), the next ~3 are "red".
    // We can't assume exact glyph counts (shaping may cluster), but we can
    // assert SOME glyph has the red color set and SOME has None.
    let any_red = shaped.glyphs.iter().any(|g| {
        g.color
            .is_some_and(|c| (c.r - 1.0).abs() < 0.01 && c.g < 0.01 && c.b < 0.01)
    });
    let any_none = shaped.glyphs.iter().any(|g| g.color.is_none());
    assert!(
        any_red,
        "no glyph carried the red span color: {:?}",
        shaped.glyphs
    );
    assert!(
        any_none,
        "no glyph carried None color (plain span should not inherit red)"
    );
}

#[test]
fn shape_rich_default_glyph_color_is_none() {
    // Spans without an explicit color must not invent one (else the renderer
    // can't tell "use widget color" from "use this color").
    let mut engine = TextEngine::new();
    let shaped = engine.shape_rich(
        &[TextSpan::new("hello"), TextSpan::new(" world")],
        16.0,
        20.0,
        None,
    );
    assert!(
        shaped.glyphs.iter().all(|g| g.color.is_none()),
        "rich spans without color should emit None per-glyph"
    );
}

#[test]
fn shape_rich_monospace_span_glyph_widths_differ_from_default() {
    // Per-span attrs must actually reach the shaper. A monospace `iii` span
    // and a proportional `iii` span produce *different* glyph cache keys
    // (different font face id). If they match, the span attrs were dropped.
    let mut engine = TextEngine::new();
    let proportional = engine.shape_rich(&[TextSpan::new("iii")], 16.0, 20.0, None);
    let mono = engine.shape_rich(&[TextSpan::new("iii").monospace()], 16.0, 20.0, None);
    assert_eq!(proportional.glyphs.len(), mono.glyphs.len());
    // Either the cache key differs (different face resolved) or the
    // advance widths differ (different metrics). One of those MUST be true
    // — if both are equal the span attrs never reached cosmic-text.
    let key_diff = proportional
        .glyphs
        .iter()
        .zip(mono.glyphs.iter())
        .any(|(a, b)| a.cache_key != b.cache_key);
    let width_diff = (proportional.width - mono.width).abs() > 1.0;
    assert!(
        key_diff || width_diff,
        "monospace span shaped identically to default — attrs dropped on path through shape_rich"
    );
}

#[test]
fn shape_text_has_no_decoration_lines() {
    // The single-attrs path has no span structure and so no decorations; a
    // caller can rely on "empty" meaning "this didn't come from shape_rich".
    let mut engine = TextEngine::new();
    let shaped = engine.shape_text("Hello", 16.0, 20.0, None);
    assert!(shaped.decoration_lines.is_empty());
}

#[test]
fn shape_rich_without_decoration_emits_no_lines() {
    // Spans that opt out of decoration must not draw stray underlines.
    let mut engine = TextEngine::new();
    let shaped = engine.shape_rich(
        &[TextSpan::new("plain"), TextSpan::new(" text").bold()],
        16.0,
        20.0,
        None,
    );
    assert!(shaped.decoration_lines.is_empty());
}

#[test]
fn shape_rich_underline_sits_below_the_baseline() {
    // An underlined span emits exactly one thin line on its single visual line,
    // positioned below the glyph baseline (larger Y, since Y grows downward).
    let mut engine = TextEngine::new();
    let shaped = engine.shape_rich(&[TextSpan::new("Hello").underline()], 32.0, 40.0, None);
    assert_eq!(
        shaped.decoration_lines.len(),
        1,
        "one underline on one line, got {:?}",
        shaped.decoration_lines
    );
    let baseline = shaped.glyphs[0].y as f32;
    let line = shaped.decoration_lines[0];
    assert!(
        line.rect.origin.y > baseline,
        "underline y {} should sit below baseline {}",
        line.rect.origin.y,
        baseline
    );
    assert!(
        line.rect.size.height >= 1.0,
        "underline must be >=1px thick"
    );
    assert!(line.rect.size.width > 0.0, "underline must span the text");
    assert!(
        line.color.is_none(),
        "uncolored span → fall back to widget color"
    );
}

#[test]
fn shape_rich_strikethrough_crosses_above_the_baseline() {
    // A strike-through is centered through the text, i.e. above the baseline
    // (smaller Y than the baseline).
    let mut engine = TextEngine::new();
    let shaped = engine.shape_rich(&[TextSpan::new("Hello").strikethrough()], 32.0, 40.0, None);
    assert_eq!(shaped.decoration_lines.len(), 1);
    let baseline = shaped.glyphs[0].y as f32;
    let line = shaped.decoration_lines[0];
    assert!(
        line.rect.origin.y < baseline,
        "strike-through y {} should sit above baseline {}",
        line.rect.origin.y,
        baseline
    );
}

#[test]
fn shape_rich_decoration_carries_the_span_color() {
    // The decoration must take the span's explicit color so it matches the
    // glyphs it decorates; an uncolored span reports None to fall back to the
    // widget color.
    let mut engine = TextEngine::new();
    let red = Color::rgb(1.0, 0.0, 0.0);
    let shaped = engine.shape_rich(
        &[
            TextSpan::new("warn").strikethrough().color(red),
            TextSpan::new(" ok").underline(),
        ],
        16.0,
        20.0,
        None,
    );
    assert_eq!(shaped.decoration_lines.len(), 2);
    let colored = shaped
        .decoration_lines
        .iter()
        .find(|l| l.color.is_some())
        .expect("the colored span's decoration should carry Some(color)");
    let c = colored.color.unwrap();
    assert!((c.r - 1.0).abs() < 0.01 && c.g < 0.01 && c.b < 0.01);
    assert!(
        shaped.decoration_lines.iter().any(|l| l.color.is_none()),
        "the uncolored underlined span should report None"
    );
}

#[test]
fn shape_rich_underline_and_strikethrough_emit_two_lines() {
    // Both flags on one span produce two distinct decoration lines.
    let mut engine = TextEngine::new();
    let shaped = engine.shape_rich(
        &[TextSpan::new("both").underline().strikethrough()],
        32.0,
        40.0,
        None,
    );
    assert_eq!(shaped.decoration_lines.len(), 2);
    let ys: Vec<i32> = shaped
        .decoration_lines
        .iter()
        .map(|l| l.rect.origin.y.round() as i32)
        .collect();
    assert_ne!(
        ys[0], ys[1],
        "underline and strike-through must not coincide"
    );
}

#[test]
fn shape_rich_wrapping_decorated_span_gets_one_line_per_visual_line() {
    // A decorated span long enough to wrap gets a decoration line per visual
    // line (so the whole wrapped run is decorated), on distinct rows.
    let mut engine = TextEngine::new();
    let spans = [TextSpan::new("one two three four five six seven eight").underline()];
    let shaped = engine.shape_rich(&spans, 16.0, 20.0, Some(80.0));
    assert!(
        shaped.decoration_lines.len() >= 2,
        "a wrapped underlined span should produce >=2 lines, got {}",
        shaped.decoration_lines.len()
    );
    let rows: std::collections::BTreeSet<i32> = shaped
        .decoration_lines
        .iter()
        .map(|l| l.rect.origin.y.round() as i32)
        .collect();
    assert!(
        rows.len() >= 2,
        "decoration lines should land on >=2 distinct rows, got {:?}",
        rows
    );
}

#[test]
fn shape_text_has_no_span_boxes() {
    // Only the rich path has span structure; the single-attrs path must leave
    // span_boxes empty so a caller can use "non-empty" as "this came from
    // shape_rich".
    let mut engine = TextEngine::new();
    let shaped = engine.shape_text("Hello", 16.0, 20.0, None);
    assert!(shaped.span_boxes.is_empty());
}

#[test]
fn shape_rich_emits_one_box_per_span_left_to_right() {
    // Three spans on one line → three boxes, tagged with their span index and
    // ordered left-to-right without overlap. This is the geometry the widget
    // hit-tests a click against.
    let mut engine = TextEngine::new();
    let shaped = engine.shape_rich(
        &[
            TextSpan::new("Hello "),
            TextSpan::new("world"),
            TextSpan::new("!"),
        ],
        16.0,
        20.0,
        None,
    );

    assert_eq!(
        shaped.span_boxes.len(),
        3,
        "one box per span on a single line"
    );
    // Boxes report their owning span index in order.
    assert_eq!(shaped.span_boxes[0].span, 0);
    assert_eq!(shaped.span_boxes[1].span, 1);
    assert_eq!(shaped.span_boxes[2].span, 2);
    // Left-to-right, each box starting at/after the previous box's right edge.
    for w in shaped.span_boxes.windows(2) {
        assert!(
            w[1].rect.origin.x >= w[0].rect.right() - 0.5,
            "span boxes should not overlap horizontally: {:?} then {:?}",
            w[0].rect,
            w[1].rect
        );
    }
    // Every box has positive area and sits at the top line (line_top ≈ 0).
    for b in &shaped.span_boxes {
        assert!(b.rect.size.width > 0.0, "box {:?} has zero width", b);
        assert!(b.rect.size.height > 0.0, "box {:?} has zero height", b);
        assert!(
            b.rect.origin.y.abs() < 0.5,
            "single-line box should sit at top"
        );
    }
}

#[test]
fn shape_rich_box_x_covers_the_clicked_span() {
    // The middle span's box must horizontally bracket where its glyphs are
    // painted — i.e. a click in the box maps back to span 1, not its
    // neighbours. We assert span 1's box starts past span 0 and ends before
    // span 2 begins.
    let mut engine = TextEngine::new();
    let shaped = engine.shape_rich(
        &[
            TextSpan::new("aaaa "),
            TextSpan::new("LINK"),
            TextSpan::new(" zzzz"),
        ],
        16.0,
        20.0,
        None,
    );
    assert_eq!(shaped.span_boxes.len(), 3);
    let link = shaped.span_boxes[1].rect;
    assert!(
        link.origin.x >= shaped.span_boxes[0].rect.right() - 0.5,
        "link box should start at/after the preceding span"
    );
    assert!(
        link.right() <= shaped.span_boxes[2].rect.origin.x + 0.5,
        "link box should end at/before the following span"
    );
}

#[test]
fn shape_rich_wrapping_span_gets_one_box_per_line() {
    // A single span long enough to wrap across two visual lines produces two
    // boxes for that span index, on different line_top rows — so a multi-line
    // link's whole footprint stays clickable.
    let mut engine = TextEngine::new();
    let spans = [TextSpan::new("one two three four five six seven eight")];
    let shaped = engine.shape_rich(&spans, 16.0, 20.0, Some(80.0));

    let span0: Vec<_> = shaped.span_boxes.iter().filter(|b| b.span == 0).collect();
    assert!(
        span0.len() >= 2,
        "a wrapped span should produce >=2 boxes, got {}",
        span0.len()
    );
    let rows: std::collections::BTreeSet<i32> = span0
        .iter()
        .map(|b| b.rect.origin.y.round() as i32)
        .collect();
    assert!(
        rows.len() >= 2,
        "wrapped span boxes should land on >=2 distinct lines, got rows {:?}",
        rows
    );
}

#[test]
fn shape_rich_wrap_breaks_inside_long_attributed_span() {
    // Gap #3's raison d'être: a row-of-widgets layout could only break
    // between runs, so "really_long_bold_token" inside one bold widget
    // couldn't wrap. The inline shaper sees the spans as a single line and
    // CAN wrap inside an attributed span when no inter-span break fits.
    //
    // We approximate the gap-#3 scenario by giving the bold span enough
    // shape-room to need wrapping under a narrow max_width.
    let mut engine = TextEngine::new();
    let spans = [
        TextSpan::new("a "),
        TextSpan::new("really really really really long bold phrase").bold(),
    ];
    let natural = engine.shape_rich(&spans, 16.0, 20.0, None);
    let wrapped = engine.shape_rich(&spans, 16.0, 20.0, Some(60.0));
    assert!(
        wrapped.height > natural.height,
        "narrow max_width should force the bold span to wrap (natural h={} wrapped h={})",
        natural.height,
        wrapped.height
    );
    // And the wrapped width should be <= the max_width we asked for.
    assert!(
        wrapped.width <= 60.0 + 0.5,
        "wrapped width {} exceeds requested max_width 60",
        wrapped.width
    );
}

// --- Shape cache (added with the unified measure+paint shaping cache) --------
//
// These guard the *observable* contract: a cache hit returns geometry identical
// to a cold shape, and every input that changes the output is part of the key
// (a missing key dimension would surface as a wrong-but-confident cache hit).
// The cache's internal mechanics (population, eviction, clear) are unit-tested
// inside `engine.rs`.

/// Compare two shaping results for full geometric equality.
fn assert_shaped_eq(a: &shroud_text::ShapedText, b: &shroud_text::ShapedText, ctx: &str) {
    assert_eq!(a.glyphs.len(), b.glyphs.len(), "{ctx}: glyph count");
    assert_eq!(a.width, b.width, "{ctx}: width");
    assert_eq!(a.height, b.height, "{ctx}: height");
    for (i, (ga, gb)) in a.glyphs.iter().zip(b.glyphs.iter()).enumerate() {
        assert_eq!(ga.cache_key, gb.cache_key, "{ctx}: glyph {i} cache_key");
        assert_eq!(ga.x, gb.x, "{ctx}: glyph {i} x");
        assert_eq!(ga.y, gb.y, "{ctx}: glyph {i} y");
    }
}

#[test]
fn cache_hit_matches_a_cold_shape() {
    // A fresh engine shapes the text once (cold). A second engine shapes it,
    // then shapes it again (the warm, cache-served call). The warm result must
    // be byte-for-byte the cold one — the whole point of the cache.
    let mut cold = TextEngine::new();
    let reference = cold.shape_text("The quick brown fox", 16.0, 20.0, Some(120.0));

    let mut warm = TextEngine::new();
    let _ = warm.shape_text("The quick brown fox", 16.0, 20.0, Some(120.0));
    let hit = warm.shape_text("The quick brown fox", 16.0, 20.0, Some(120.0));

    assert_shaped_eq(&hit, &reference, "warm cache hit vs cold shape");
}

#[test]
fn cache_key_distinguishes_max_width() {
    // A long line shaped unwrapped, then wrapped narrow: if max_width were left
    // out of the key, the second call would wrongly return the unwrapped result
    // (same height). The wrapped one must be taller.
    let mut engine = TextEngine::new();
    let text = "wrapping must change the cached result not reuse it";
    let unwrapped = engine.shape_text(text, 16.0, 20.0, None);
    let wrapped = engine.shape_text(text, 16.0, 20.0, Some(80.0));
    assert!(
        wrapped.height > unwrapped.height,
        "wrapped (h={}) must differ from unwrapped (h={}) — max_width must be in the key",
        wrapped.height,
        unwrapped.height
    );
}

#[test]
fn cache_key_distinguishes_font_size() {
    let mut engine = TextEngine::new();
    let small = engine.shape_text("Hello", 16.0, 20.0, None);
    let large = engine.shape_text("Hello", 32.0, 40.0, None);
    assert!(
        large.width > small.width,
        "32px (w={}) must not reuse 16px (w={}) — metrics must be in the key",
        large.width,
        small.width
    );
}

#[test]
fn cache_key_distinguishes_attrs() {
    // Same text, different weight. A bold glyph comes from a different font
    // instance, so its cache_key differs from the normal one. If attrs were not
    // keyed, the bold call would return the normal cached glyphs (equal keys).
    let mut engine = TextEngine::new();
    let normal = engine.shape_text_attrs("Hello", 16.0, 20.0, None, &TextAttrs::default());
    let bold = engine.shape_text_attrs(
        "Hello",
        16.0,
        20.0,
        None,
        &TextAttrs::default().weight(FontWeight::BOLD),
    );
    assert!(!normal.glyphs.is_empty() && !bold.glyphs.is_empty());
    assert_ne!(
        normal.glyphs[0].cache_key, bold.glyphs[0].cache_key,
        "bold must not reuse the normal-weight cached glyphs — attrs must be in the key"
    );
}

#[test]
fn rich_cache_hit_matches_cold_shape() {
    let spans = vec![
        TextSpan::new("plain "),
        TextSpan::new("bold").bold(),
        TextSpan::new(" tail"),
    ];
    let mut cold = TextEngine::new();
    let reference = cold.shape_rich(&spans, 16.0, 20.0, Some(200.0));

    let mut warm = TextEngine::new();
    let _ = warm.shape_rich(&spans, 16.0, 20.0, Some(200.0));
    let hit = warm.shape_rich(&spans, 16.0, 20.0, Some(200.0));

    assert_shaped_eq(&hit, &reference, "rich warm cache hit vs cold shape");
    assert_eq!(
        hit.span_boxes.len(),
        reference.span_boxes.len(),
        "rich cache hit must preserve span boxes"
    );
}

#[test]
fn clear_shape_cache_preserves_correctness() {
    let mut engine = TextEngine::new();
    let before = engine.shape_text("round trip", 16.0, 20.0, None);
    engine.clear_shape_cache();
    let after = engine.shape_text("round trip", 16.0, 20.0, None);
    assert_shaped_eq(&after, &before, "shape after clear");
}

// ── Attrs-carrying caret / selection geometry (FW-19 / G12) ──────────
// An editable `Input` with a non-default weight must place its caret and
// selection against glyphs shaped at the *same* weight. These prove the
// `_attrs` geometry variants thread the attrs all the way through, so the
// caret sits at the block width that the matching `shape_text_attrs` reports
// (and doesn't silently fall back to a normal-weight shape).

#[test]
fn caret_at_end_tracks_the_weight_it_shapes_with() {
    let mut engine = TextEngine::new();
    let text = "Wjgy Mix";
    let (fs, lh) = (28.0, 28.0 * 1.2);

    // Regular: the default `caret_at_offset` matches the default shaping.
    let regular = engine.shape_text_attrs(text, fs, lh, None, &TextAttrs::default());
    let (reg_x, _) = engine.caret_at_offset(text, text.len(), fs, lh, None);
    assert!(
        (reg_x - regular.width).abs() < 1.0,
        "regular caret x ({reg_x}) should sit at the shaped width ({})",
        regular.width
    );

    // Bold: the caret must line up with the *bold* shaped width, not the
    // regular one. If `_attrs` dropped the weight, this would land at the
    // narrower normal-weight width and (given a real bold face exists on this
    // system, per `cache_key_distinguishes_attrs`) miss the block edge.
    let bold_attrs = TextAttrs::default().weight(FontWeight::BOLD);
    let bold = engine.shape_text_attrs(text, fs, lh, None, &bold_attrs);
    let (bold_x, _) = engine.caret_at_offset_attrs(text, text.len(), fs, lh, None, &bold_attrs);
    assert!(
        (bold_x - bold.width).abs() < 1.0,
        "bold caret x ({bold_x}) should sit at the bold shaped width ({})",
        bold.width
    );
}

#[test]
fn selection_rects_attrs_span_the_weight_they_shape_with() {
    let mut engine = TextEngine::new();
    let text = "Selection";
    let (fs, lh) = (22.0, 22.0 * 1.2);
    let bold_attrs = TextAttrs::default().weight(FontWeight::BOLD);

    let bold = engine.shape_text_attrs(text, fs, lh, None, &bold_attrs);
    let rects = engine.selection_rects_attrs(text, 0, text.len(), fs, lh, None, &bold_attrs);
    let covered: f32 = rects.iter().map(|r| r.size.width).sum();
    assert!(
        (covered - bold.width).abs() < 1.5,
        "bold selection width ({covered}) should match the bold shaped width ({})",
        bold.width
    );
}

#[test]
fn offset_at_point_attrs_matches_the_default_for_default_attrs() {
    // The default-attrs delegation must be behavior-preserving: the new
    // `_attrs` path fed `TextAttrs::default()` resolves the same offset the
    // pre-existing `offset_at_point` does.
    let mut engine = TextEngine::new();
    let text = "click target";
    let (fs, lh) = (18.0, 18.0 * 1.2);
    for x in [0.0_f32, 15.0, 40.0, 200.0] {
        let base = engine.offset_at_point(text, x, 5.0, fs, lh, None);
        let via_attrs =
            engine.offset_at_point_attrs(text, x, 5.0, fs, lh, None, &TextAttrs::default());
        assert_eq!(base, via_attrs, "default-attrs offset must match at x={x}");
    }
}

// ── ComposedBlock (IME composing fast path) ──────────────────────────────
//
// `shape_composing` folds the glyphs, caret, and preedit underline that a
// composing `Input` used to get from three separate shapes into one. These
// tests pin it to be behavior-preserving: for each representative text it must
// reproduce, exactly, what `shape_text_attrs` (glyphs + height),
// `caret_at_offset_attrs` (caret), and `selection_rects_attrs` (underline)
// return on their own.

#[test]
fn shape_composing_matches_the_three_separate_shapes() {
    let mut engine = TextEngine::new();
    let (fs, lh) = (18.0, 18.0 * 1.3);
    let attrs = TextAttrs::default();

    // Plain ASCII, multi-hard-line, a soft-wrapping long line, CJK, a caret at
    // the very end, and a hard-newline interior boundary — the cases the caret
    // fallback and the wrap-affinity path care about.
    let cases: &[(&str, Option<f32>)] = &[
        ("hello world", None),
        ("first line\nsecond line", None),
        (
            "the quick brown fox jumps over the lazy dog again and again",
            Some(120.0),
        ),
        ("\u{65E5}\u{672C}\u{8A9E}\u{306E}\u{5165}\u{529B}", None), // 日本語の入力
        ("mixed \u{304B}\u{306A} text\nand a second row", Some(90.0)),
    ];

    for (text, wrap) in cases {
        let text = *text;
        let wrap = *wrap;

        // The separate reference shapes.
        let ref_shaped = engine.shape_text_attrs(text, fs, lh, wrap, &attrs);

        // Try a caret at every char boundary, and an underline over every
        // boundary-aligned range, so the fold is checked exhaustively per text.
        let boundaries: Vec<usize> = (0..=text.len())
            .filter(|&i| text.is_char_boundary(i))
            .collect();

        for &caret in &boundaries {
            for w in boundaries.windows(2) {
                let (ps, pe) = (w[0], w[1]);
                let ref_caret = engine.caret_at_offset_attrs(text, caret, fs, lh, wrap, &attrs);
                let ref_underline =
                    engine.selection_rects_attrs(text, ps, pe, fs, lh, wrap, &attrs);

                let block = engine.shape_composing(text, fs, lh, wrap, &attrs, caret, (ps, pe));

                assert_eq!(
                    block.caret, ref_caret,
                    "caret mismatch at offset {caret} in {text:?} (wrap {wrap:?})"
                );
                assert_eq!(
                    block.underline, ref_underline,
                    "underline mismatch for range {ps}..{pe} in {text:?} (wrap {wrap:?})"
                );
                assert_eq!(
                    block.shaped.glyphs.len(),
                    ref_shaped.glyphs.len(),
                    "glyph count mismatch in {text:?} (wrap {wrap:?})"
                );
                for (a, b) in block.shaped.glyphs.iter().zip(&ref_shaped.glyphs) {
                    assert_eq!(
                        (a.x, a.y),
                        (b.x, b.y),
                        "glyph position mismatch in {text:?} (wrap {wrap:?})"
                    );
                }
                assert_eq!(
                    block.shaped.height, ref_shaped.height,
                    "content height mismatch in {text:?} (wrap {wrap:?})"
                );
            }
        }
    }
}

// ── EditBuffer (focused editing single-shape path) ────────────────────────
//
// `shape_edit_plain` / `shape_edit_rich` fold the glyphs, content height,
// caret, selection, and click hit-tests a focused (non-composing) `Input`
// used to get from separate shapes into one buffer. These tests pin every
// derived query to reproduce, exactly, what the standalone engine methods
// return on their own.

#[test]
fn edit_buffer_plain_matches_the_separate_shapes() {
    let mut engine = TextEngine::new();
    let (fs, lh) = (18.0, 18.0 * 1.3);
    let attrs = TextAttrs::default();

    let cases: &[(&str, Option<f32>)] = &[
        ("", None),
        ("hello world", None),
        ("first line\nsecond line", None),
        ("a\n\nb", None), // empty interior line
        (
            "the quick brown fox jumps over the lazy dog again and again",
            Some(120.0),
        ),
        ("\u{65E5}\u{672C}\u{8A9E}\u{306E}\u{5165}\u{529B}", None), // 日本語の入力
        ("mixed \u{304B}\u{306A} text\nand a second row", Some(90.0)),
        ("trailing newline\n", None),
    ];

    for (text, wrap) in cases {
        let (text, wrap) = (*text, *wrap);
        let edit = engine.shape_edit_plain(text, fs, lh, wrap, &attrs);

        // Glyphs + block extent.
        let ref_shaped = engine.shape_text_attrs(text, fs, lh, wrap, &attrs);
        assert_eq!(
            edit.shaped().glyphs.len(),
            ref_shaped.glyphs.len(),
            "glyph count mismatch in {text:?} (wrap {wrap:?})"
        );
        for (a, b) in edit.shaped().glyphs.iter().zip(&ref_shaped.glyphs) {
            assert_eq!(
                (a.x, a.y),
                (b.x, b.y),
                "glyph position mismatch in {text:?} (wrap {wrap:?})"
            );
        }
        assert_eq!(
            (edit.shaped().width, edit.shaped().height),
            (ref_shaped.width, ref_shaped.height),
            "block extent mismatch in {text:?} (wrap {wrap:?})"
        );

        let boundaries: Vec<usize> = (0..=text.len())
            .filter(|&i| text.is_char_boundary(i))
            .collect();

        // Caret at every char boundary.
        for &off in &boundaries {
            let want = engine.caret_at_offset_attrs(text, off, fs, lh, wrap, &attrs);
            let got = engine.edit_caret(&edit, text, off, fs, lh, wrap, &attrs);
            assert_eq!(
                got, want,
                "caret mismatch at offset {off} in {text:?} (wrap {wrap:?})"
            );
        }

        // Trailing-sliver selection over every boundary-aligned range.
        for (i, &lo) in boundaries.iter().enumerate() {
            for &hi in &boundaries[i..] {
                let want =
                    engine.selection_rects_with_trailing_attrs(text, lo, hi, fs, lh, wrap, &attrs);
                let got = edit.selection_rects_with_trailing(text, lo, hi, fs);
                assert_eq!(
                    got, want,
                    "selection mismatch for {lo}..{hi} in {text:?} (wrap {wrap:?})"
                );
            }
        }

        // Click hit-tests on a grid across (and past) the block.
        let grid_w = ref_shaped.width.max(fs) + 12.0;
        let grid_h = ref_shaped.height.max(lh) + 12.0;
        let mut y = -6.0;
        while y < grid_h {
            let mut x = -6.0;
            while x < grid_w {
                let want = engine.offset_at_point_attrs(text, x, y, fs, lh, wrap, &attrs);
                let got = edit.hit(text, x, y);
                assert_eq!(
                    got, want,
                    "hit mismatch at ({x}, {y}) in {text:?} (wrap {wrap:?})"
                );
                x += 9.0;
            }
            y += lh / 2.0;
        }
    }
}

#[test]
fn edit_buffer_rich_matches_rich_glyphs_and_plain_geometry() {
    // A focused field with a live highlighter shapes color-only spans. The
    // highlighter invariant (see `build_highlight_spans`) is that color-only
    // spans shape to the identical layout as the plain value — which is what
    // lets the rich `EditBuffer` answer caret / hit / selection queries that
    // agree with the plain-attrs standalone methods, while its glyphs carry
    // the standalone rich shape's colors.
    let mut engine = TextEngine::new();
    let (fs, lh) = (18.0, 18.0 * 1.3);
    let attrs = TextAttrs::default();
    let wrap = Some(110.0);

    let (p1, p2, p3, p4) = (
        "alpha ",
        "bold and",
        "\nkana \u{304B}\u{306A}",
        " tail that wraps onward",
    );
    let text = format!("{p1}{p2}{p3}{p4}");
    let red = Color::rgb(0.9, 0.3, 0.2);
    let blue = Color::rgb(0.2, 0.4, 0.9);
    let spans = [
        TextSpan::new(p1),
        TextSpan::new(p2).color(red),
        TextSpan::new(p3).color(blue),
        TextSpan::new(p4),
    ];

    let edit = engine.shape_edit_rich(&spans, fs, lh, wrap);

    // Glyphs (colors included), span boxes, and decorations match the
    // standalone rich shape.
    let ref_rich = engine.shape_rich(&spans, fs, lh, wrap);
    assert_eq!(edit.shaped().glyphs.len(), ref_rich.glyphs.len());
    for (a, b) in edit.shaped().glyphs.iter().zip(&ref_rich.glyphs) {
        assert_eq!((a.x, a.y), (b.x, b.y), "rich glyph position mismatch");
        assert_eq!(a.color, b.color, "rich glyph color mismatch");
    }
    assert_eq!(edit.shaped().span_boxes, ref_rich.span_boxes);
    assert_eq!(edit.shaped().decoration_lines, ref_rich.decoration_lines);
    assert_eq!(
        (edit.shaped().width, edit.shaped().height),
        (ref_rich.width, ref_rich.height)
    );

    // Caret / selection / hit agree with the *plain* standalone methods.
    let boundaries: Vec<usize> = (0..=text.len())
        .filter(|&i| text.is_char_boundary(i))
        .collect();
    for &off in &boundaries {
        let want = engine.caret_at_offset_attrs(&text, off, fs, lh, wrap, &attrs);
        let got = engine.edit_caret(&edit, &text, off, fs, lh, wrap, &attrs);
        assert_eq!(got, want, "rich caret mismatch at offset {off}");
    }
    for (i, &lo) in boundaries.iter().enumerate() {
        for &hi in &boundaries[i..] {
            let want =
                engine.selection_rects_with_trailing_attrs(&text, lo, hi, fs, lh, wrap, &attrs);
            let got = edit.selection_rects_with_trailing(&text, lo, hi, fs);
            assert_eq!(got, want, "rich selection mismatch for {lo}..{hi}");
        }
    }
    let mut y = -6.0;
    while y < ref_rich.height + 12.0 {
        let mut x = -6.0;
        while x < ref_rich.width.max(fs) + 12.0 {
            let want = engine.offset_at_point_attrs(&text, x, y, fs, lh, wrap, &attrs);
            let got = edit.hit(&text, x, y);
            assert_eq!(got, want, "rich hit mismatch at ({x}, {y})");
            x += 9.0;
        }
        y += lh / 2.0;
    }
}

#[test]
fn caret_at_line_end_matches_the_prefix_shape_answer() {
    // A caret whose offset points at a `\n` has no glyph to highlight;
    // `caret_from_buffer` answers it from the full buffer's rows. Pin that to
    // the historical ground truth — shape the prefix `text[..off]` and read
    // the end of its last run (`cursor_position_attrs`) — for every newline,
    // including empty lines, a leading newline, and a soft-wrapped line whose
    // hard break lands mid-paragraph.
    let mut engine = TextEngine::new();
    let (fs, lh) = (18.0, 18.0 * 1.3);
    let attrs = TextAttrs::default();

    let cases: &[(&str, Option<f32>)] = &[
        ("first line\nsecond", None),
        ("a\n\nb\n", None),
        ("\nleading", None),
        ("wrap wrap wrap wrap wrap\nnext", Some(60.0)),
        ("\u{65E5}\u{672C}\u{8A9E}\n\u{304B}\u{306A}", None),
    ];
    for (text, wrap) in cases {
        let (text, wrap) = (*text, *wrap);
        for (off, b) in text.bytes().enumerate() {
            if b != b'\n' {
                continue;
            }
            let want = engine.cursor_position_attrs(&text[..off], fs, lh, wrap, &attrs);
            let got = engine.caret_at_offset_attrs(text, off, fs, lh, wrap, &attrs);
            assert_eq!(
                got, want,
                "caret at \\n offset {off} in {text:?} (wrap {wrap:?})"
            );
        }
    }
}
