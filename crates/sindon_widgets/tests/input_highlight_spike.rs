//! B-1 live-highlight spike: widget-level proof.
//!
//! The engine-level proof lives in
//! `sindon_text/tests/highlight_layout_spike.rs` (color-only rich shaping is
//! layout-identical to plain shaping). These tests confirm the *widget* wiring:
//! a `highlighter` closure actually tints the classified glyphs when an `Input`
//! paints, and attaching one does not move a single glyph — so the caret /
//! selection geometry (still computed from the plain buffer) stays aligned.

use sindon_core::{Color, Theme};
use sindon_text::TextEngine;
use sindon_widgets::paint::PaintContext;
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Container, Input};

const TEXT: &str = "let count = 42";

/// Lay out and paint a single `Input`, returning each painted glyph's screen
/// position and color. `DrawGlyph` isn't `Clone` (it owns a bitmap), so project
/// to the fields the assertions need.
fn paint_glyphs(input: Input) -> Vec<(f32, f32, Color)> {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(120.0));
    tree.add_child(root, input);

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 120.0, &mut engine, &theme);
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
    ctx.glyphs.iter().map(|g| (g.x, g.y, g.color)).collect()
}

fn close(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 0.02
        && (a.g - b.g).abs() < 0.02
        && (a.b - b.b).abs() < 0.02
        && (a.a - b.a).abs() < 0.02
}

#[test]
fn highlighter_colors_the_classified_range() {
    let red = Color::rgb(0.9, 0.1, 0.1);
    let glyphs = paint_glyphs(
        Input::new()
            .with_value(TEXT)
            // Tint the leading `let` keyword; the rest renders default.
            .highlighter(move |buf| {
                if buf.starts_with("let") {
                    vec![(0, 3, red)]
                } else {
                    vec![]
                }
            }),
    );

    assert!(
        glyphs.iter().any(|g| close(g.2, red)),
        "the classified keyword should produce red glyphs, got {:?}",
        glyphs.iter().map(|g| g.2).collect::<Vec<_>>()
    );
    assert!(
        glyphs.iter().any(|g| !close(g.2, red)),
        "the un-classified gap should keep the default text color",
    );
}

#[test]
fn attaching_a_highlighter_does_not_move_any_glyph() {
    let red = Color::rgb(0.9, 0.1, 0.1);
    let plain = paint_glyphs(Input::new().with_value(TEXT));
    let highlighted = paint_glyphs(
        Input::new()
            .with_value(TEXT)
            .highlighter(move |_| vec![(0, 3, red), (4, 9, red)]),
    );

    let positions = |v: &[(f32, f32, Color)]| v.iter().map(|g| (g.0, g.1)).collect::<Vec<_>>();
    assert_eq!(
        positions(&plain),
        positions(&highlighted),
        "color-only highlighting must leave glyph positions byte-identical",
    );
}

#[test]
fn malformed_ranges_do_not_panic_and_degrade_to_plain() {
    // Overlapping, reversed, out-of-bounds, and mid-codepoint ranges must all be
    // skipped rather than panicking the paint-side slice. With a multibyte
    // buffer this also guards the char-boundary check.
    let blue = Color::rgb(0.1, 0.2, 0.9);
    let glyphs = paint_glyphs(Input::new().with_value("値value").highlighter(move |_| {
        vec![
            (2, 1, blue),     // reversed → skip
            (1, 3, blue),     // splits the 3-byte 値 → skip
            (100, 200, blue), // out of bounds → clamped to empty → skip
            (3, 8, blue),     // valid: the ASCII "value"
        ]
    }));
    assert!(
        glyphs.iter().any(|g| close(g.2, blue)),
        "the one valid range should still color its glyphs",
    );
}
