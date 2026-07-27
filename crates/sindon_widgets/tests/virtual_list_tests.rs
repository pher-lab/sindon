//! Tests for the `VirtualList` primitive: a fixed-row-height list that
//! materializes only the rows in (or near) the viewport. The windowing runs
//! from the tree's layout pass (`sync_virtual_lists`), reading the enclosing
//! `ScrollView`'s offset, so these drive it via `compute_layout_with_measure`
//! and `dispatch_event(Scroll { .. })`.
//!
//! The scroll views here use `scroll_transition(Duration::ZERO)` so a single
//! dispatch jumps the offset instantly (no eased glide to pump across frames).

use std::time::Duration;

use sindon_core::{Point, Theme};
use sindon_text::TextEngine;
use sindon_widgets::event::{EventContext, WidgetEvent};
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Container, ScrollView, VirtualList};

const W: f32 = 400.0;

/// A million rows still lay out a screenful of widgets, not a million — the
/// whole point of the primitive — and the scroll extent still spans them all.
#[test]
fn materializes_a_bounded_window_and_follows_scroll() {
    const N: usize = 5000;
    const ROW_H: f32 = 40.0;
    const VH: f32 = 300.0;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(VH));
    let sv = tree.add_child(
        root,
        ScrollView::new()
            .width_full()
            .height(VH)
            .scroll_transition(Duration::ZERO),
    );
    let vl = VirtualList::new(ROW_H)
        .items(|| N)
        .on_row(|tree, parent, _i| {
            tree.add_child(parent, Container::row().width_full().height(ROW_H));
        })
        .build(&mut tree, sv);

    let theme = Theme::default();
    let mut engine = TextEngine::new();
    tree.compute_layout_with_measure(W, VH, &mut engine, &theme);

    let initial = tree.children(vl).len();
    assert!(
        initial > 0 && initial < 100,
        "a {N}-item list must materialize only a screenful, got {initial}"
    );

    // Content height is pinned to the full logical extent (N * row_h), not the
    // ~screenful of rows that actually exist — so the scrollbar and the scroll
    // clamp span every row. If it weren't pinned, max_scroll would reflect only
    // the handful of materialized rows.
    let max_scroll = tree
        .widget_as::<ScrollView>(sv)
        .expect("scroll view present")
        .max_scroll_y(VH);
    let expected_max = N as f32 * ROW_H - VH;
    assert!(
        (max_scroll - expected_max).abs() < ROW_H,
        "content must be pinned to N*row_h: max_scroll {max_scroll} vs expected {expected_max}"
    );

    // Scroll to the bottom (a huge delta the handler clamps to max). The window
    // must follow and stay bounded, with a tall leading spacer offsetting the
    // first materialized row to its true logical position.
    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(200.0, 150.0),
            delta_x: 0.0,
            delta_y: -1_000_000.0,
        },
        &mut ctx,
    );
    tree.compute_layout_with_measure(W, VH, &mut engine, &theme);

    let scrolled = tree.children(vl).len();
    assert!(
        scrolled > 0 && scrolled < 100,
        "after scrolling to the bottom the window stays bounded, got {scrolled}"
    );

    // First child is the leading spacer; near the bottom it must be tall (it
    // stands in for the ~5000 rows scrolled above the window).
    let spacer = tree.children(vl)[0];
    let spacer_h = tree.layout_rect(spacer).size.height;
    assert!(
        spacer_h > VH * 10.0,
        "leading spacer must offset the window to its logical y, got {spacer_h}"
    );
}

/// The materialized window is sized by the viewport, not the item count: a
/// hundred-item list and a million-item list build the same screenful.
#[test]
fn window_size_is_independent_of_item_count() {
    fn window_len(n: usize) -> usize {
        const ROW_H: f32 = 40.0;
        const VH: f32 = 320.0;

        let mut tree = WidgetTree::new();
        let root = tree.set_root(Container::column().width(W).height(VH));
        let sv = tree.add_child(
            root,
            ScrollView::new()
                .width_full()
                .height(VH)
                .scroll_transition(Duration::ZERO),
        );
        let vl = VirtualList::new(ROW_H)
            .items(move || n)
            .on_row(|tree, parent, _i| {
                tree.add_child(parent, Container::row().width_full().height(ROW_H));
            })
            .build(&mut tree, sv);

        let theme = Theme::default();
        let mut engine = TextEngine::new();
        tree.compute_layout_with_measure(W, VH, &mut engine, &theme);
        tree.children(vl).len()
    }

    let small = window_len(100);
    let huge = window_len(1_000_000);
    assert!(
        huge < 100,
        "a million-item list still materializes only a screenful, got {huge}"
    );
    assert_eq!(
        small, huge,
        "window size must not grow with item count ({small} vs {huge})"
    );
}
