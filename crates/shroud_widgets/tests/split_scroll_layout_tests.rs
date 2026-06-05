//! Regression for the live-split preview: a `ScrollView` inside a flex *row*
//! pane must clamp to the row's height so tall content stays scrollable.
//!
//! The bug: wrapping the editor + preview panes in a flex *row* re-introduced
//! the "grow viewport balloons to its content" problem one level up. The row
//! grows to fill the editor pane's leftover height, but its automatic minimum
//! size is its (tall) content, so the row itself ballooned to the preview's
//! content height — and every pane stretched to that, leaving the ScrollView
//! nothing to scroll. The row needs `overflow_hidden` (automatic min size 0),
//! exactly like the scroll wrapper does, so it clamps to the allocated height.

use shroud_core::{Point, Theme};
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, WidgetEvent};
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, ScrollView, TextWidget};

const VIEWPORT_W: f32 = 800.0;
const VIEWPORT_H: f32 = 600.0;

/// Build `root(column, full) > row(grow) > pane(col, overflow_hidden) >
/// ScrollView(grow) > column(many tall lines)`, mirroring the editor's split.
/// `row_overflow_hidden` toggles the fix on the wrapping row.
fn build(row_overflow_hidden: bool) -> (WidgetTree, usize, usize) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(VIEWPORT_W)
            .height(VIEWPORT_H)
            .padding(0.0),
    );
    // A header takes some vertical space, exactly like the editor pane.
    tree.add_child(root, TextWidget::new("header"));

    let mut row = Container::row().width_full().grow(1.0).gap(16.0);
    if row_overflow_hidden {
        row = row.overflow_hidden();
    }
    let row = tree.add_child(root, row);

    let pane = tree.add_child(
        row,
        Container::column()
            .grow(1.0)
            .flex_basis(0.0)
            .overflow_hidden(),
    );
    let scroll = tree.add_child(pane, ScrollView::new().width_full().grow(1.0));
    let content = tree.add_child(scroll, Container::column().width_full().gap(8.0));
    // Far more content than the viewport can show.
    for i in 0..60 {
        tree.add_child(content, TextWidget::new(format!("line {i}")));
    }

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(VIEWPORT_W, VIEWPORT_H, &mut engine, &theme);
    (tree, scroll, content)
}

#[test]
fn scrollview_in_a_row_pane_clamps_to_viewport() {
    // With `overflow_hidden` on the wrapping row, the row clamps to the editor
    // pane's leftover height, the pane stretches to that, and the ScrollView
    // viewport stays far shorter than its content — i.e. there is room to scroll.
    let (tree, scroll, content) = build(true);
    let scroll_h = tree.layout_rect(scroll).size.height;
    let content_h = tree.layout_rect(content).size.height;

    assert!(
        scroll_h <= VIEWPORT_H,
        "the ScrollView viewport must fit within the window, got {scroll_h}"
    );
    assert!(
        content_h > scroll_h + 50.0,
        "content ({content_h}) must overflow the viewport ({scroll_h}) so it scrolls"
    );
}

#[test]
fn scroll_offset_reclamps_when_content_shrinks() {
    // Repro for the live-preview wart: scroll down in a long note, then switch
    // to a short one. The ScrollView node isn't rebuilt (only its content is),
    // so a stale offset would leave the short note's top scrolled out of view.
    // The tree must re-clamp the offset to the new (smaller) content extent.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let scroll = tree.add_child(root, ScrollView::new().width_full().height(300.0));
    let content = tree.add_child(scroll, Container::column().width_full().gap(8.0));
    for i in 0..40 {
        tree.add_child(content, TextWidget::new(format!("line {i}")));
    }

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);

    // Scroll to the bottom (a big negative delta; the handler clamps to max).
    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(200.0, 150.0),
            delta_x: 0.0,
            delta_y: -5000.0,
        },
        &mut ctx,
    );
    let scrolled = tree
        .widget_as::<ScrollView>(scroll)
        .expect("scroll view")
        .scroll_y();
    assert!(scrolled > 0.0, "should have scrolled down, got {scrolled}");

    // Switch to a short note: replace the tall content with a single line.
    for child in tree.children(content) {
        tree.remove(child);
    }
    tree.add_child(content, TextWidget::new("just one line"));
    tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);

    let after = tree
        .widget_as::<ScrollView>(scroll)
        .expect("scroll view")
        .scroll_y();
    assert_eq!(
        after, 0.0,
        "offset must reset to 0 when content no longer overflows, got {after}"
    );
}

#[test]
fn row_without_overflow_hidden_balloons_the_panes() {
    // Documents the bug this guards against: a plain `grow` row takes its
    // automatic minimum from its tall content, so the whole split balloons and
    // the ScrollView grows to its content — nothing scrolls.
    let (tree, scroll, content) = build(false);
    let scroll_h = tree.layout_rect(scroll).size.height;
    let content_h = tree.layout_rect(content).size.height;

    assert!(
        scroll_h >= content_h,
        "without the row overflow_hidden the ScrollView balloons to content ({content_h}), got {scroll_h}"
    );
}
