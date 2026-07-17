//! Scroll-into-view on focus: Tab (and programmatic focus) must bring the
//! widget it lands on into the viewport of every `ScrollView` around it.
//!
//! Without this the tree happily focuses a widget hundreds of pixels below the
//! visible area: the ring is painted off-screen, so Tab reads as "nothing
//! happened", and the Enter that follows fires a button the user cannot see.
//!
//! The reveal *arithmetic* (minimal move, edge alignment, clamping, spans
//! taller than the viewport) is unit-tested next to it in
//! `scroll_view.rs`; these tests cover the wiring — which focus reasons reveal,
//! that the walk finds the scroll ancestor, and that the pass is ordered after
//! layout.

use shroud_core::{Point, Theme};
use shroud_reactive::animation::test_clock::{self, ClockGuard};
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Button, Container, ScrollView};
use std::any::Any;

const VIEWPORT_H: f32 = 200.0;

/// A scroll viewport with more buttons than fit in it. Returns the tree, the
/// ScrollView index, and every button index in tab order.
///
/// Scrolling glides on the wall clock, so anything that reads the *displayed*
/// offset is racing a real timer; the returned guard holds the clock still and
/// must outlive the tree's use. Assertions here are about where the scroll is
/// heading (the target), not how it gets there.
fn scrolling_list() -> (WidgetTree, usize, Vec<usize>, ClockGuard) {
    let clock = test_clock::freeze();
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(VIEWPORT_H));
    let sv = tree.add_child(root, ScrollView::new().width(400.0).height(VIEWPORT_H));
    let buttons = (0..12)
        .map(|i| tree.add_child(sv, Button::new(format!("B{i}"))))
        .collect();
    (tree, sv, buttons, clock)
}

/// One frame's worth of layout — including the post-layout pass that consumes
/// a pending reveal.
fn frame(tree: &mut WidgetTree) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, VIEWPORT_H, &mut engine, &theme);
}

fn tab(tree: &mut WidgetTree, ctx: &mut EventContext) {
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Tab),
        },
        ctx,
    );
    frame(tree);
}

/// The ScrollView's logical scroll target. Reading the target rather than the
/// eased `scroll_offset` keeps these tests independent of the animation clock.
fn scroll_y(tree: &WidgetTree, sv: usize) -> f32 {
    (tree.widget(sv) as &dyn Any)
        .downcast_ref::<ScrollView>()
        .expect("sv index holds a ScrollView")
        .scroll_y()
}

/// Whether the widget is fully inside the scroll viewport as painted — the
/// property the whole feature exists to guarantee.
fn is_visible(tree: &WidgetTree, sv: usize, idx: usize) -> bool {
    let view = tree.layout_rect(sv);
    let r = tree.layout_rect(idx);
    let scroll = scroll_y(tree, sv);
    let top = r.origin.y - scroll;
    let bottom = top + r.size.height;
    top >= view.origin.y - 0.5 && bottom <= view.origin.y + view.size.height + 0.5
}

#[test]
fn tab_onto_an_offscreen_widget_scrolls_it_into_view() {
    let (mut tree, sv, buttons, _clock) = scrolling_list();
    frame(&mut tree);

    // Sanity: the list really does overflow, i.e. the last button starts below
    // the viewport. Without that this test would pass vacuously.
    let last = *buttons.last().unwrap();
    assert!(
        tree.layout_rect(last).origin.y > VIEWPORT_H,
        "fixture must overflow the viewport"
    );

    let mut ctx = EventContext::new();
    for _ in 0..buttons.len() {
        tab(&mut tree, &mut ctx);
    }

    assert_eq!(
        tree.focused(),
        Some(last),
        "Tab should end on the last button"
    );
    assert!(
        scroll_y(&tree, sv) > 0.0,
        "the viewport should have followed focus down"
    );
    assert!(
        is_visible(&tree, sv, last),
        "the focused button must be inside the viewport"
    );
}

#[test]
fn every_tab_stop_is_visible_when_focus_lands_on_it() {
    // The guarantee is per-stop, not just at the end: walking the whole list
    // must never leave focus somewhere the user can't see.
    let (mut tree, sv, buttons, _clock) = scrolling_list();
    frame(&mut tree);
    let mut ctx = EventContext::new();

    for expected in &buttons {
        tab(&mut tree, &mut ctx);
        assert_eq!(tree.focused(), Some(*expected));
        assert!(
            is_visible(&tree, sv, *expected),
            "focus landed on a widget outside the viewport"
        );
    }

    // And back up again — Shift+Tab has the same duty.
    ctx.modifiers.shift = true;
    for expected in buttons.iter().rev().skip(1) {
        tab(&mut tree, &mut ctx);
        assert_eq!(tree.focused(), Some(*expected));
        assert!(
            is_visible(&tree, sv, *expected),
            "Shift+Tab landed on a widget outside the viewport"
        );
    }
    // Wrapping back to the first stop must reach the very top of the content.
    assert_eq!(scroll_y(&tree, sv), 0.0);
}

#[test]
fn tab_within_the_visible_area_does_not_scroll() {
    // The reveal moves the minimum distance, which for an already-visible
    // target is nothing at all — otherwise every Tab would jitter the view.
    let (mut tree, sv, _, _clock) = scrolling_list();
    frame(&mut tree);
    let mut ctx = EventContext::new();

    tab(&mut tree, &mut ctx);
    assert_eq!(scroll_y(&tree, sv), 0.0, "first stop is already at the top");
}

#[test]
fn clicking_a_widget_does_not_scroll_the_view() {
    // Pointer focus must not reveal: the user pointed at the widget, so it is
    // on screen by definition, and scrolling under a click is disorienting.
    let (mut tree, sv, buttons, _clock) = scrolling_list();
    frame(&mut tree);
    let mut ctx = EventContext::new();

    // Park the viewport mid-list with a wheel scroll.
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(200.0, 100.0),
            delta_x: 0.0,
            delta_y: -120.0,
        },
        &mut ctx,
    );
    frame(&mut tree);
    let parked = scroll_y(&tree, sv);
    assert!(parked > 0.0, "wheel should have scrolled the list");

    // Click a button at the position it is *painted* at. Hit-testing follows
    // the eased offset, not the target, so a click lands on the glyph the user
    // currently sees mid-glide — and the two differ here because the test
    // clock is frozen with the wheel glide still in flight.
    let displayed = tree.widget(sv).scroll_offset().1;
    let target = buttons[1];
    let r = tree.layout_rect(target);
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(r.origin.x + 5.0, r.origin.y - displayed + 5.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
    frame(&mut tree);

    assert_eq!(tree.focused(), Some(target), "sanity: the click focused it");
    assert_eq!(
        scroll_y(&tree, sv),
        parked,
        "a click must leave the scroll offset alone"
    );
}

#[test]
fn programmatic_focus_before_the_first_layout_still_reveals() {
    // The ordering trap: `focus_initially` is applied by the event loop *before*
    // the first `compute_layout` of a new screen, when every rect still reads
    // zero. A reveal computed at the focus call would silently do nothing; it
    // has to wait for the layout pass.
    let (mut tree, sv, buttons, _clock) = scrolling_list();
    let last = *buttons.last().unwrap();

    tree.focus_initially(last);
    let mut ctx = EventContext::new();
    tree.flush_pending_focus(&mut ctx);
    frame(&mut tree);

    assert_eq!(tree.focused(), Some(last));
    assert!(
        is_visible(&tree, sv, last),
        "a screen that opens focused on an off-screen field must show it"
    );
}

#[test]
fn a_removed_target_cancels_the_reveal() {
    // Focus, then delete the widget before the frame lands. The reveal must
    // drop the request rather than measure a tombstoned slot.
    let (mut tree, sv, buttons, _clock) = scrolling_list();
    frame(&mut tree);
    let mut ctx = EventContext::new();

    // Scroll to the bottom, so a reveal that fired for the top button would be
    // unmistakable: it would drag the viewport all the way back up.
    for _ in 0..buttons.len() {
        tab(&mut tree, &mut ctx);
    }
    let settled = scroll_y(&tree, sv);
    assert!(
        settled > 100.0,
        "fixture should end scrolled near the bottom"
    );

    tree.focus(Some(buttons[0]), &mut ctx);
    tree.remove(buttons[0]);
    frame(&mut tree);

    assert_eq!(
        tree.focused(),
        None,
        "removing the focused widget clears it"
    );
    // Not `== settled`: dropping a row shortens the content, and `clamp_scroll`
    // legitimately trims the offset by that row's height. What must not happen
    // is the jump to ~0 that revealing the (now gone) top button would cause.
    assert!(
        scroll_y(&tree, sv) > 100.0,
        "the cancelled reveal must not pull the viewport back to the top, got {}",
        scroll_y(&tree, sv)
    );
}

#[test]
fn nested_scroll_views_each_reveal_the_target() {
    // An inner list inside an outer scroller: the outer viewport sees the
    // target displaced by the inner scroll, so the walk has to fold each
    // settled offset into the next one out.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(VIEWPORT_H));
    let outer = tree.add_child(root, ScrollView::new().width(400.0).height(VIEWPORT_H));
    // Spacer so the inner view starts below the outer viewport's bottom edge —
    // both scrollers must move for the target to be seen.
    tree.add_child(outer, Container::column().width(400.0).height(300.0));
    let inner = tree.add_child(outer, ScrollView::new().width(400.0).height(150.0));
    let buttons: Vec<usize> = (0..8)
        .map(|i| tree.add_child(inner, Button::new(format!("N{i}"))))
        .collect();
    frame(&mut tree);

    let last = *buttons.last().unwrap();
    let mut ctx = EventContext::new();
    tree.focus(Some(last), &mut ctx);
    frame(&mut tree);

    assert!(
        scroll_y(&tree, inner) > 0.0,
        "the inner list should have scrolled to its last button"
    );
    assert!(
        is_visible(&tree, inner, last),
        "target must be inside the inner viewport"
    );

    // The inner viewport itself must now be inside the outer one. Its painted
    // position is its layout y minus the outer scroll (the inner scroll shifts
    // the inner's *children*, not the inner view itself).
    let outer_view = tree.layout_rect(outer);
    let inner_rect = tree.layout_rect(inner);
    let inner_top = inner_rect.origin.y - scroll_y(&tree, outer);
    assert!(
        inner_top >= outer_view.origin.y - 0.5
            && inner_top + inner_rect.size.height <= outer_view.origin.y + VIEWPORT_H + 0.5,
        "the outer view should have scrolled the inner list into view"
    );
}
