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

use sindon_core::{Point, Theme};
use sindon_text::TextEngine;
use sindon_widgets::event::{EventContext, WidgetEvent};
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Container, ScrollView, TextWidget};

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

// --- Sidebar chain (the post-unlock shell, distinct from the editor split) ---

/// Replicates the *sidebar* nesting from `vault_screen`:
/// `column(full) > row(grow) > pane(column, height_full) > [header,
/// ScrollView(grow) > list(60 lines), settings]`. The grow row sits between the
/// definite-height shell and the `height_full` pane, and the pane has a trailing
/// sibling (the settings button) *after* the ScrollView. `row_overflow_hidden`
/// toggles the fix on that grow row.
fn build_sidebar_chain(row_overflow_hidden: bool) -> (WidgetTree, usize, usize, usize) {
    let mut tree = WidgetTree::new();
    let shell = tree.set_root(Container::column().width(VIEWPORT_W).height(VIEWPORT_H));

    let mut row = Container::row().width_full().grow(1.0);
    if row_overflow_hidden {
        row = row.overflow_hidden();
    }
    let row = tree.add_child(shell, row);

    let pane = tree.add_child(
        row,
        Container::column()
            .width(260.0)
            .height_full()
            .padding(16.0)
            .gap(12.0),
    );
    tree.add_child(pane, TextWidget::new("header"));
    let scroll = tree.add_child(pane, ScrollView::new().width_full().grow(1.0));
    let list = tree.add_child(scroll, Container::column().width_full().gap(4.0));
    for i in 0..60 {
        tree.add_child(list, TextWidget::new(format!("note {i}")));
    }
    let settings = tree.add_child(pane, TextWidget::new("settings"));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(VIEWPORT_W, VIEWPORT_H, &mut engine, &theme);
    (tree, scroll, list, settings)
}

#[test]
fn sidebar_chain_with_overflow_hidden_clamps_and_keeps_settings_visible() {
    // The fix: `overflow_hidden` on the grow row pins its automatic minimum to
    // 0, so it clamps to the shell's height instead of ballooning to the tall
    // note list. The `height_full` pane stretches to the clamped row, the
    // ScrollView gets the pane's leftover height (room to scroll), and the
    // trailing settings button stays inside the window.
    let (tree, scroll, list, settings) = build_sidebar_chain(true);
    let scroll_h = tree.layout_rect(scroll).size.height;
    let list_h = tree.layout_rect(list).size.height;
    let settings = tree.layout_rect(settings);
    let settings_bottom = settings.origin.y + settings.size.height;

    assert!(
        scroll_h <= VIEWPORT_H,
        "the ScrollView viewport must fit within the window, got {scroll_h}"
    );
    assert!(
        list_h > scroll_h + 50.0,
        "the note list ({list_h}) must overflow the viewport ({scroll_h}) so it scrolls"
    );
    assert!(
        settings_bottom <= VIEWPORT_H,
        "the settings button (bottom {settings_bottom}) must stay inside the window ({VIEWPORT_H})"
    );
}

#[test]
fn sidebar_chain_without_overflow_hidden_balloons_and_hides_settings() {
    // Documents the bug (knot 2026-06-13: sidebar scrollbar never showed, the
    // wheel did nothing, and the settings button sank off the bottom). Without
    // `overflow_hidden` on the grow row, the row inherits its tall content as an
    // automatic minimum, the pane stretches past the window, the ScrollView
    // balloons to its content (so nothing scrolls / no scrollbar), and the
    // trailing settings button is pushed below the viewport.
    let (tree, scroll, list, settings) = build_sidebar_chain(false);
    let scroll_h = tree.layout_rect(scroll).size.height;
    let list_h = tree.layout_rect(list).size.height;
    let settings = tree.layout_rect(settings);
    let settings_bottom = settings.origin.y + settings.size.height;

    assert!(
        scroll_h >= list_h,
        "without overflow_hidden the ScrollView balloons to its content ({list_h}), got {scroll_h}"
    );
    assert!(
        settings_bottom > VIEWPORT_H,
        "the bug pushes the settings button (bottom {settings_bottom}) below the window ({VIEWPORT_H})"
    );
}
