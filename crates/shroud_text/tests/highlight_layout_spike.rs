//! B-1 live-highlight spike: layout-invariance proof.
//!
//! The editable `Input` computes its caret / selection / click geometry by
//! shaping the buffer as **plain** text (`shape_text` → `cursor_position`,
//! `offset_at_point`, `selection_rects`). To paint syntax highlighting we want
//! to *render* the same buffer through `shape_rich` with per-range colors. That
//! only works if rich shaping lands every glyph at the exact pixel position
//! plain shaping does — otherwise the caret drifts off the colored glyphs.
//!
//! Hypothesis: cosmic-text splits *shape runs* by script / bidi level / font
//! attrs (family / weight / style), **not** by color. So a span list that
//! varies only in color shapes identically to the concatenated plain string.
//! These tests are the empirical check on that hypothesis — the load-bearing
//! question for the whole feature. If any of them fail, color-only highlighting
//! is NOT layout-invariant and the design needs rich-aware geometry helpers.

use shroud_core::Color;
use shroud_text::{ShapedText, TextEngine, TextSpan};

/// Split `text` into `n`-ish color-only spans at char boundaries, each tagged a
/// different color but with default (plain) attrs — exactly the shape a
/// syntax highlighter would emit: same glyphs, different colors.
fn color_only_spans(text: &str, chunks: usize) -> Vec<TextSpan> {
    let palette = [
        Color::rgba(1.0, 0.0, 0.0, 1.0),
        Color::rgba(0.0, 1.0, 0.0, 1.0),
        Color::rgba(0.0, 0.0, 1.0, 1.0),
        Color::rgba(1.0, 1.0, 0.0, 1.0),
    ];
    let total = text.chars().count();
    let per = total.div_ceil(chunks.max(1));
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut color_idx = 0;
    for (i, ch) in text.chars().enumerate() {
        buf.push(ch);
        let at_chunk_edge = (i + 1) % per == 0;
        if at_chunk_edge {
            spans.push(TextSpan::new(std::mem::take(&mut buf)).color(palette[color_idx % 4]));
            color_idx += 1;
        }
    }
    if !buf.is_empty() {
        spans.push(TextSpan::new(buf).color(palette[color_idx % 4]));
    }
    spans
}

/// Assert two shaped results place glyphs identically (position + cache key),
/// ignoring color — color is the *only* thing rich shaping is allowed to add.
fn assert_same_layout(plain: &ShapedText, rich: &ShapedText, ctx: &str) {
    assert_eq!(
        plain.glyphs.len(),
        rich.glyphs.len(),
        "{ctx}: glyph count differs (plain {} vs rich {})",
        plain.glyphs.len(),
        rich.glyphs.len()
    );
    for (i, (p, r)) in plain.glyphs.iter().zip(rich.glyphs.iter()).enumerate() {
        assert_eq!(p.x, r.x, "{ctx}: glyph {i} x differs ({} vs {})", p.x, r.x);
        assert_eq!(p.y, r.y, "{ctx}: glyph {i} y differs ({} vs {})", p.y, r.y);
        assert_eq!(
            p.cache_key, r.cache_key,
            "{ctx}: glyph {i} cache_key differs — different shaping",
        );
    }
    // The rich path must actually be carrying colors (the whole point), while
    // the plain path carries none — otherwise this test is vacuous.
    assert!(
        rich.glyphs.iter().any(|g| g.color.is_some()),
        "{ctx}: rich glyphs should carry per-span colors",
    );
    assert!(
        plain.glyphs.iter().all(|g| g.color.is_none()),
        "{ctx}: plain glyphs should carry no color",
    );
}

#[test]
fn single_line_color_split_preserves_layout() {
    let mut e = TextEngine::new();
    let text = "let x = compute(42) + total_count;";
    let plain = e.shape_text(text, 16.0, 19.2, None);
    let rich = e.shape_rich(&color_only_spans(text, 6), 16.0, 19.2, None);
    assert_same_layout(&plain, &rich, "single line");
}

#[test]
fn split_inside_a_word_preserves_layout() {
    // The harshest case for the hypothesis: a color boundary *inside* a word,
    // where kerning / ligatures could in principle differ between one run and
    // two. A syntax highlighter splits mid-identifier all the time.
    let mut e = TextEngine::new();
    let text = "fn highlight_token(buffer)";
    let plain = e.shape_text(text, 18.0, 21.6, None);
    // Two spans split at byte 5 ("fn hi" | "ghlight_token(buffer)").
    let spans = vec![
        TextSpan::new("fn hi").color(Color::rgba(1.0, 0.0, 0.0, 1.0)),
        TextSpan::new("ghlight_token(buffer)").color(Color::rgba(0.0, 0.5, 1.0, 1.0)),
    ];
    let rich = e.shape_rich(&spans, 18.0, 21.6, None);
    assert_same_layout(&plain, &rich, "split inside word");
}

#[test]
fn wrapped_multiline_color_split_preserves_layout() {
    // Soft-wrap (max_width) is where a drift would show up as a whole line of
    // misaligned glyphs. Confirm the wrap points and per-line baselines match.
    let mut e = TextEngine::new();
    let text =
        "the quick brown fox jumps over the lazy dog and then keeps running well past the edge";
    let wrap = Some(160.0);
    let plain = e.shape_text(text, 16.0, 19.2, wrap);
    let rich = e.shape_rich(&color_only_spans(text, 8), 16.0, 19.2, wrap);
    assert_same_layout(&plain, &rich, "wrapped multiline");
}

#[test]
fn hard_newlines_color_split_preserves_layout() {
    let mut e = TextEngine::new();
    let text = "line one\nl'ine two has more\nthird";
    let plain = e.shape_text(text, 16.0, 19.2, Some(240.0));
    let rich = e.shape_rich(&color_only_spans(text, 5), 16.0, 19.2, Some(240.0));
    assert_same_layout(&plain, &rich, "hard newlines");
}

#[test]
fn multibyte_color_split_preserves_layout() {
    // CJK + ASCII mix, split across the script boundary. On a runner without a
    // CJK font both sides fall back to the same .notdef, so this still passes;
    // where a CJK font exists it checks the fallback-font glyphs line up too.
    let mut e = TextEngine::new();
    let text = "コードcode = 値value;";
    let plain = e.shape_text(text, 17.0, 20.4, None);
    let rich = e.shape_rich(&color_only_spans(text, 5), 17.0, 20.4, None);
    assert_same_layout(&plain, &rich, "multibyte");
}
