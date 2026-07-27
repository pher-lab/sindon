//! `TreeView` keyboard roving, wired up in a real tree.
//!
//! The key *arithmetic* (which row an arrow lands on, what → and ← mean at each
//! kind of node, how type-ahead matches) is unit-tested next to the widget in
//! `tree_view.rs`. These tests cover what only a live tree can show:
//!
//! - the cursor drags the scroll viewport along with it, via
//!   [`EventContext::reveal`](sindon_widgets::event::EventContext::reveal) —
//!   focus itself never moves, so nothing else would;
//! - focus stays parked on the host, including across the row rebuild that an
//!   expand triggers (a focused *row* would be tombstoned by its own toggle);
//! - a click hands the keyboard over without the focus change yanking the
//!   scroll position.
//!
//! ⚠ Scrolling *eases*: the offset paint and hit-testing use lags the target a
//! reveal sets. So the clock is frozen and stepped explicitly after every key
//! (see [`press`]), and geometry is read from the displayed offset — computing
//! a click position from the target would aim at a row that is not there yet.

use std::any::Any;
use std::time::Duration;

use sindon_core::{Point, Theme};
use sindon_reactive::animation::test_clock::{self, ClockGuard};
use sindon_text::TextEngine;
use sindon_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Container, ScrollView, TreeItem, TreeView};

const VIEWPORT_H: f32 = 200.0;
/// Rows are 26px, so 20 of them overflow a 200px viewport several times over.
const ROWS: u64 = 20;
/// Comfortably longer than the scroll ease, so each key settles before the next.
const SETTLE: Duration = Duration::from_millis(1000);

/// A flat 20-row tree inside a scroll viewport. Returns the tree, the
/// `ScrollView` index, the `TreeView` host index, and the frozen-clock guard.
fn tree_in_viewport() -> (WidgetTree, usize, usize, ClockGuard) {
    let clock = test_clock::freeze();
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(VIEWPORT_H));
    let sv = tree.add_child(root, ScrollView::new().width(400.0).height(VIEWPORT_H));
    let items = (0..ROWS)
        .map(|i| TreeItem::new(i, format!("item {i}")))
        .collect();
    let host = TreeView::new(items).build(&mut tree, sv);
    (tree, sv, host, clock)
}

/// A nested tree: one closed parent with three children, then a leaf.
fn nested_tree() -> (WidgetTree, usize, ClockGuard) {
    let clock = test_clock::freeze();
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(VIEWPORT_H));
    let items = vec![
        TreeItem::with_children(
            1,
            "src",
            vec![
                TreeItem::new(2, "main.rs"),
                TreeItem::new(3, "lib.rs"),
                TreeItem::new(4, "app.rs"),
            ],
        ),
        TreeItem::new(5, "README.md"),
    ];
    let host = TreeView::new(items).build(&mut tree, root);
    (tree, host, clock)
}

/// One frame of layout, including the reactive-children sync that (re)builds
/// the rows and the post-layout pass that consumes a pending reveal.
fn frame(tree: &mut WidgetTree) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, VIEWPORT_H, &mut engine, &theme);
}

/// Press a key, then let any scrolling it armed finish easing.
fn press(tree: &mut WidgetTree, clock: &ClockGuard, named: NamedKey) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(named),
        },
        &mut ctx,
    );
    frame(tree);
    clock.advance(SETTLE);
    frame(tree);
}

/// Tab into the tree — the host is the only focusable thing in these fixtures.
fn tab_into_the_tree(tree: &mut WidgetTree, clock: &ClockGuard) {
    press(tree, clock, NamedKey::Tab);
}

/// The row node indices, in display order.
fn rows(tree: &WidgetTree, host: usize) -> Vec<usize> {
    let reactive = tree.children(host);
    assert_eq!(reactive.len(), 1, "the host owns exactly the row list");
    tree.children(reactive[0])
}

/// The scroll offset paint and hit-testing actually use.
fn scrolled_by(tree: &WidgetTree, sv: usize) -> f32 {
    tree.widget(sv).scroll_offset().1
}

/// The `ScrollView`'s logical scroll target, which the displayed offset eases
/// toward.
fn scroll_target(tree: &WidgetTree, sv: usize) -> f32 {
    (tree.widget(sv) as &dyn Any)
        .downcast_ref::<ScrollView>()
        .expect("sv index holds a ScrollView")
        .scroll_y()
}

/// Whether a row is fully inside the scroll viewport as painted.
fn is_visible(tree: &WidgetTree, sv: usize, idx: usize) -> bool {
    let view = tree.layout_rect(sv);
    let r = tree.layout_rect(idx);
    let top = r.origin.y - scrolled_by(tree, sv);
    let bottom = top + r.size.height;
    top >= view.origin.y - 0.5 && bottom <= view.origin.y + view.size.height + 0.5
}

/// Viewport-space point in the middle of a row — where a user would click it.
fn click_point(tree: &WidgetTree, sv: usize, idx: usize) -> Point {
    let r = tree.layout_rect(idx);
    Point::new(
        r.origin.x + 120.0,
        r.origin.y - scrolled_by(tree, sv) + r.size.height / 2.0,
    )
}

#[test]
fn arrowing_below_the_fold_scrolls_the_row_into_view() {
    let (mut tree, sv, host, clock) = tree_in_viewport();
    frame(&mut tree);
    let rows = rows(&tree, host);
    assert_eq!(rows.len(), ROWS as usize);

    // Sanity: the list really does overflow, so this can't pass vacuously.
    let last = *rows.last().unwrap();
    assert!(
        !is_visible(&tree, sv, last),
        "fixture must overflow the viewport"
    );

    tab_into_the_tree(&mut tree, &clock);
    assert_eq!(
        scroll_target(&tree, sv),
        0.0,
        "entering the tree scrolls nothing"
    );

    for _ in 0..ROWS {
        press(&mut tree, &clock, NamedKey::ArrowDown);
    }

    assert!(
        scrolled_by(&tree, sv) > 0.0,
        "walking past the fold scrolled the viewport"
    );
    assert!(
        is_visible(&tree, sv, last),
        "the row the cursor landed on is on screen"
    );

    // ...and back up again: the reveal has to work in both directions.
    for _ in 0..ROWS {
        press(&mut tree, &clock, NamedKey::ArrowUp);
    }
    assert_eq!(scrolled_by(&tree, sv), 0.0, "returned to the top");
    assert!(is_visible(&tree, sv, rows[0]));
}

#[test]
fn focus_stays_on_the_host_while_the_cursor_moves() {
    let (mut tree, _sv, host, clock) = tree_in_viewport();
    frame(&mut tree);
    tab_into_the_tree(&mut tree, &clock);
    assert_eq!(tree.focused(), Some(host), "Tab lands on the host");

    for _ in 0..5 {
        press(&mut tree, &clock, NamedKey::ArrowDown);
    }
    assert_eq!(
        tree.focused(),
        Some(host),
        "the cursor roves inside the host; focus does not move to a row"
    );
}

#[test]
fn expanding_under_the_cursor_keeps_focus() {
    // The reason focus lives on the host: → rebuilds the row list, and a focused
    // row would be tombstoned by its own toggle.
    let (mut tree, host, clock) = nested_tree();
    frame(&mut tree);
    assert_eq!(rows(&tree, host).len(), 2, "starts collapsed: src, README");

    tab_into_the_tree(&mut tree, &clock);
    press(&mut tree, &clock, NamedKey::ArrowRight);

    assert_eq!(rows(&tree, host).len(), 5, "the node's children appeared");
    assert_eq!(
        tree.focused(),
        Some(host),
        "the rebuild did not drop keyboard focus"
    );

    // The keyboard still works against the rows the rebuild produced.
    press(&mut tree, &clock, NamedKey::ArrowRight);
    press(&mut tree, &clock, NamedKey::Enter);
    assert_eq!(tree.focused(), Some(host));
}

#[test]
fn clicking_a_row_takes_focus_without_jumping_the_scroll() {
    let (mut tree, sv, host, clock) = tree_in_viewport();
    frame(&mut tree);
    tab_into_the_tree(&mut tree, &clock);
    for _ in 0..ROWS {
        press(&mut tree, &clock, NamedKey::ArrowDown);
    }
    let scrolled = scrolled_by(&tree, sv);
    assert!(scrolled > 0.0, "fixture must be scrolled away from the top");

    // Click a row that is currently on screen, at its painted position.
    let at = rows(&tree, host)
        .iter()
        .position(|&idx| is_visible(&tree, sv, idx))
        .expect("some row is visible");
    let point = click_point(&tree, sv, rows(&tree, host)[at]);
    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            button: MouseButton::Left,
            position: point,
        },
        &mut ctx,
    );
    frame(&mut tree);

    assert_eq!(
        tree.focused(),
        Some(host),
        "clicking a row hands the keyboard to the host"
    );
    assert_eq!(
        scrolled_by(&tree, sv),
        scrolled,
        "and the focus change must not yank the viewport to fit the whole tree"
    );

    // The arrows carry on from the row that was clicked, not from where the
    // cursor had been — which would have scrolled the viewport away. (Selecting
    // rebuilt the rows, so re-resolve the display position rather than reusing a
    // tombstoned index.)
    press(&mut tree, &clock, NamedKey::ArrowDown);
    assert_eq!(
        scrolled_by(&tree, sv),
        scrolled,
        "the next row was already on screen, so nothing scrolled"
    );
    assert!(
        is_visible(&tree, sv, rows(&tree, host)[at]),
        "navigation resumed around the clicked row"
    );
}
