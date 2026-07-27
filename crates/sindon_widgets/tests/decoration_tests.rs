//! Integration test for inline text decoration (`TextSpan::underline` /
//! `strikethrough`).
//!
//! Decoration lines are drawn as filled rectangles, so the proof that the
//! engine geometry reaches the screen is that a decorated rich `TextWidget`
//! deposits a thin rect into the `PaintContext` rect batch — and an undecorated
//! one deposits none. A bare `Container` with no background paints no rect of
//! its own, so any rect present came from the decoration.

use sindon_core::{Color, Theme};
use sindon_text::{TextEngine, TextSpan};
use sindon_widgets::paint::PaintContext;
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Container, TextWidget};

/// Lay out and paint `tree`, returning the rects accumulated this frame.
fn rects_after_paint(tree: &mut WidgetTree) -> Vec<sindon_render::DrawRect> {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 200.0, &mut engine, &theme);
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
    ctx.rects
}

#[test]
fn strikethrough_span_paints_a_thin_decoration_rect() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    tree.add_child(
        root,
        // Rich (not plain) so the decoration path runs; a second span keeps it
        // off the all-plain fast collapse if one ever existed at this layer.
        TextWidget::rich(vec![
            TextSpan::new("deleted").strikethrough(),
            TextSpan::new(" kept"),
        ])
        .font_size(32.0),
    );

    let rects = rects_after_paint(&mut tree);
    // The only rect on screen is the strike-through: thin (a few px tall) and
    // meaningfully wide.
    assert!(
        rects.iter().any(|r| r.height <= 4.0 && r.width > 10.0),
        "expected a thin, wide strike-through rect, got sizes {:?}",
        sizes(&rects)
    );
}

#[test]
fn undecorated_rich_text_paints_no_rects() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    tree.add_child(
        root,
        TextWidget::rich(vec![TextSpan::new("plain"), TextSpan::new(" words").bold()]),
    );

    let rects = rects_after_paint(&mut tree);
    assert!(
        rects.is_empty(),
        "no decoration and no background should mean no rects, got sizes {:?}",
        sizes(&rects)
    );
}

#[test]
fn decoration_takes_the_span_color() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let red = Color::rgb(1.0, 0.0, 0.0);
    tree.add_child(
        root,
        TextWidget::rich(vec![TextSpan::new("warn").strikethrough().color(red)]).font_size(32.0),
    );

    let rects = rects_after_paint(&mut tree);
    assert!(
        rects
            .iter()
            .any(|r| (r.color.r - 1.0).abs() < 0.01 && r.color.g < 0.01 && r.color.b < 0.01),
        "strike-through should inherit the red span color, got colors {:?}",
        rects
            .iter()
            .map(|r| (r.color.r, r.color.g, r.color.b))
            .collect::<Vec<_>>()
    );
}

/// `(width, height)` of each rect — `DrawRect` isn't `Debug`, so summarize the
/// geometry for assertion messages.
fn sizes(rects: &[sindon_render::DrawRect]) -> Vec<(f32, f32)> {
    rects.iter().map(|r| (r.width, r.height)).collect()
}
