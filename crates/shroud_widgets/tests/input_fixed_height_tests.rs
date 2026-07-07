//! G3 residual (FW-25): a fixed pixel height for a multi-line `Input`
//! (CSS `resize-none h-24` on a `<textarea>`).
//!
//! `Input::min_height` is only a *floor*: it happens to coincide with a fixed
//! box today because an `Input` reports no intrinsic content size (so a flex
//! parent sizes it to the floor), but a stretching / growing parent — or any
//! future content measurement — would let it balloon past the design's height.
//! `Input::height(px)` sets a *definite* size that caps the box exactly, and in
//! multi-line mode turns it into a scrolling viewport. These tests pin:
//!
//!   1. `height(px)` caps the box where `min_height(px)` would be stretched,
//!   2. a fixed-height multi-line field clips + scrolls overflow (scrollbar),
//!   3. content that fits draws no scrollbar, and
//!   4. `height(px)` fixes a single-line box too.

use shroud_core::{Point, Rect, Theme};
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, MouseButton, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Input};

fn paint(tree: &WidgetTree) -> PaintContext {
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
    ctx
}

#[test]
fn fixed_height_caps_where_min_height_is_stretched() {
    // Cross-axis stretch is the flex default: a child of a `row` with no
    // definite height stretches to the row's height. A `min_height(96)` field
    // therefore balloons to the 300px row (the floor is satisfied, stretch
    // grows it), while a definite `height(96)` stays exactly 96 — the true
    // `h-24` cap the approximation lacked.
    for (definite, expected) in [(false, 300.0f32), (true, 96.0f32)] {
        let mut tree = WidgetTree::new();
        let root = tree.set_root(Container::row().width(400.0).height(300.0));
        let field = if definite {
            Input::new().multiline().height(96.0)
        } else {
            Input::new().multiline().min_height(96.0)
        };
        let idx = tree.add_child(root, field);

        let mut engine = TextEngine::new();
        let theme = Theme::default();
        tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);

        let h = tree.layout_rect(idx).size.height;
        assert!(
            (h - expected).abs() < 0.5,
            "definite={definite}: expected height {expected}, got {h}"
        );
    }
}

/// Mirrors the private consts in `input.rs`: the scrollbar draws a
/// `SCROLLBAR_WIDTH`-wide track + thumb at the field's inner right edge.
const SCROLLBAR_WIDTH: f32 = 6.0;
const SCROLLBAR_INSET: f32 = 2.0;

fn scrollbar_bars(ctx: &PaintContext, field: Rect) -> Vec<(f32, f32)> {
    let bar_x = field.right() - 1.0 - SCROLLBAR_WIDTH - SCROLLBAR_INSET;
    ctx.rects
        .iter()
        .filter(|r| (r.width - SCROLLBAR_WIDTH).abs() < 0.01 && (r.x - bar_x).abs() < 0.5)
        .map(|r| (r.y, r.height))
        .collect()
}

/// Build a focused multi-line field with a fixed `height(96)` holding enough
/// lines to overflow, returning the tree + laid-out rect.
fn build_fixed_height(n_lines: usize) -> (WidgetTree, Rect) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(600.0));
    let body: String = (0..n_lines).map(|i| format!("line {i}\n")).collect();
    let idx = tree.add_child(
        root,
        Input::new()
            .multiline()
            .height(96.0)
            .with_value(body.as_str()),
    );

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 600.0, &mut engine, &theme);
    let rect = tree.layout_rect(idx);

    // Focus so the caret + scroll machinery run.
    let mut ev = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ev,
    );
    (tree, rect)
}

#[test]
fn fixed_height_multiline_clips_and_scrolls_overflow() {
    let (tree, rect) = build_fixed_height(30);
    // The box is exactly 96 regardless of the 30 lines of content.
    assert!(
        (rect.size.height - 96.0).abs() < 0.5,
        "box should be 96, got {}",
        rect.size.height
    );

    let ctx = paint(&tree);

    // Overflow is clipped to the field's padding box (never past the border).
    assert!(!ctx.glyphs.is_empty(), "body text should produce glyphs");
    for g in &ctx.glyphs {
        assert!(
            g.clip_rect.is_some(),
            "a fixed-height multi-line glyph must be clipped to the field box"
        );
    }

    // A scrollbar (track + thumb) is drawn because content overflows.
    assert_eq!(
        scrollbar_bars(&ctx, rect).len(),
        2,
        "overflowing fixed-height field draws a track + thumb"
    );
}

#[test]
fn fixed_height_multiline_no_scrollbar_when_content_fits() {
    // One short line inside a 96px box has nothing to scroll.
    let (tree, rect) = build_fixed_height(1);
    let ctx = paint(&tree);
    assert!(
        scrollbar_bars(&ctx, rect).is_empty(),
        "a fixed-height field whose content fits draws no scrollbar"
    );
}

#[test]
fn fixed_height_single_line_pins_the_box() {
    // `height(px)` also fixes a single-line box (text stays vertically centered).
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(600.0));
    let idx = tree.add_child(root, Input::new().with_value("hello").height(48.0));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 600.0, &mut engine, &theme);

    assert!(
        (tree.layout_rect(idx).size.height - 48.0).abs() < 0.5,
        "single-line height(48) should pin the box at 48, got {}",
        tree.layout_rect(idx).size.height
    );
}
