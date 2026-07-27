//! Integration tests for Phase 24 — context-menu primitive.
//!
//! Covers `Container::on_context_menu` firing on `MouseDown { button: Right }`,
//! the position handed to the handler, the right-click-does-not-steal-focus
//! contract enforced by `WidgetTree::dispatch_with_target`, and the shared
//! `MenuItem` widget's click + drag-off semantics.

use std::cell::Cell;
use std::rc::Rc;

use sindon_core::{Color, Point, Theme};
use sindon_reactive::Signal;
use sindon_text::TextEngine;
use sindon_widgets::event::{EventContext, MouseButton, WidgetEvent};
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Button, Container, MenuItem};

fn measured_layout(tree: &mut WidgetTree, w: f32, h: f32) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(w, h, &mut engine, &theme);
}

fn dispatch(tree: &mut WidgetTree, ev: WidgetEvent) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(&ev, &mut ctx);
}

fn right_mouse_down(tree: &mut WidgetTree, x: f32, y: f32) {
    dispatch(
        tree,
        WidgetEvent::MouseDown {
            position: Point::new(x, y),
            button: MouseButton::Right,
        },
    );
}

fn left_mouse_down(tree: &mut WidgetTree, x: f32, y: f32) {
    dispatch(
        tree,
        WidgetEvent::MouseDown {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
    );
}

fn left_mouse_up(tree: &mut WidgetTree, x: f32, y: f32) {
    dispatch(
        tree,
        WidgetEvent::MouseUp {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
    );
}

#[test]
fn container_on_context_menu_fires_on_right_click_inside_rect() {
    let fired = Rc::new(Cell::new(0u32));
    let last_pos = Rc::new(Cell::new(Point::new(-1.0, -1.0)));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(300.0).height(200.0).padding(20.0));
    let captured_pos = Rc::clone(&last_pos);
    let captured_fired = Rc::clone(&fired);
    let panel = tree.add_child(
        root,
        Container::row()
            .width(120.0)
            .height(80.0)
            .background(Color::rgb(0.2, 0.2, 0.2))
            .on_context_menu(move |pos, _ctx| {
                captured_fired.set(captured_fired.get() + 1);
                captured_pos.set(pos);
            }),
    );
    measured_layout(&mut tree, 300.0, 200.0);

    let r = tree.layout_rect(panel);
    right_mouse_down(&mut tree, r.origin.x + 30.0, r.origin.y + 20.0);

    assert_eq!(fired.get(), 1, "handler fires once on right-click");
    let p = last_pos.get();
    assert!(
        (p.x - (r.origin.x + 30.0)).abs() < 0.5,
        "handler receives x in subtree-local coords (got {})",
        p.x
    );
    assert!(
        (p.y - (r.origin.y + 20.0)).abs() < 0.5,
        "handler receives y in subtree-local coords (got {})",
        p.y
    );
}

#[test]
fn container_on_context_menu_does_not_fire_for_left_click() {
    let fired = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(120.0));
    let captured = Rc::clone(&fired);
    let panel = tree.add_child(
        root,
        Container::row()
            .width(100.0)
            .height(60.0)
            .on_context_menu(move |_pos, _ctx| {
                captured.set(captured.get() + 1);
            }),
    );
    measured_layout(&mut tree, 200.0, 120.0);

    let r = tree.layout_rect(panel);
    left_mouse_down(&mut tree, r.origin.x + 10.0, r.origin.y + 10.0);
    left_mouse_up(&mut tree, r.origin.x + 10.0, r.origin.y + 10.0);

    assert_eq!(fired.get(), 0, "left-click must not trigger context menu");
}

#[test]
fn container_without_on_context_menu_stays_inert_to_right_click() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(120.0));
    let _panel = tree.add_child(
        root,
        Container::row()
            .width(100.0)
            .height(60.0)
            .background(Color::rgb(0.3, 0.3, 0.3)),
    );
    measured_layout(&mut tree, 200.0, 120.0);
    // The bare assertion is that dispatch does not panic and there is
    // nothing for it to do — the previous tests rely on this being the
    // default behaviour, but make it explicit here.
    right_mouse_down(&mut tree, 50.0, 30.0);
    assert_eq!(tree.layer_count(), 0);
}

#[test]
fn right_click_does_not_change_focus() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(80.0).gap(20.0));
    let a = tree.add_child(root, Button::new("A"));
    let b = tree.add_child(root, Button::new("B"));
    measured_layout(&mut tree, 400.0, 80.0);

    // Seed focus on A via a left click.
    let ra = tree.layout_rect(a);
    left_mouse_down(&mut tree, ra.origin.x + 5.0, ra.origin.y + 5.0);
    assert_eq!(tree.focused(), Some(a), "left-click focuses A");

    // Right-click on B — focus must stay on A.
    let rb = tree.layout_rect(b);
    right_mouse_down(&mut tree, rb.origin.x + 5.0, rb.origin.y + 5.0);
    assert_eq!(
        tree.focused(),
        Some(a),
        "right-click on B must not steal focus from A"
    );

    // Sanity: left-click on B still moves focus (we did not regress the
    // primary-click focus path).
    left_mouse_down(&mut tree, rb.origin.x + 5.0, rb.origin.y + 5.0);
    assert_eq!(tree.focused(), Some(b), "left-click on B moves focus");
}

#[test]
fn right_click_outside_any_focusable_does_not_blur() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0).padding(20.0));
    let a = tree.add_child(root, Button::new("A"));
    measured_layout(&mut tree, 400.0, 200.0);

    let ra = tree.layout_rect(a);
    left_mouse_down(&mut tree, ra.origin.x + 5.0, ra.origin.y + 5.0);
    assert_eq!(tree.focused(), Some(a));

    // Right-click in a non-focusable region (the column padding). Left
    // click here would blur to None; right click must leave focus alone.
    right_mouse_down(&mut tree, 5.0, 5.0);
    assert_eq!(
        tree.focused(),
        Some(a),
        "right-click off-target preserves focus"
    );
}

#[test]
fn menu_item_fires_on_left_click_release() {
    let fired = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let captured = Rc::clone(&fired);
    let item = tree.set_root(MenuItem::new("Click me", move |_ctx| {
        captured.set(captured.get() + 1);
    }));
    measured_layout(&mut tree, 200.0, 60.0);

    let r = tree.layout_rect(item);
    // Press → hover prerequisite for `pressed`; release.
    dispatch(&mut tree, WidgetEvent::MouseEnter);
    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(r.origin.x + 5.0, r.origin.y + 5.0),
        },
    );
    left_mouse_down(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);
    left_mouse_up(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);

    assert_eq!(fired.get(), 1, "MenuItem fires on MouseUp after MouseDown");
}

#[test]
fn menu_item_label_is_inset_by_horizontal_padding() {
    // G16: the label must honour the `px-3` padding declared in `style`, not
    // hug the box's left edge. The leftmost glyph should sit ~12px in from the
    // row origin (small tolerance for the glyph's left bearing).
    let mut tree = WidgetTree::new();
    let item = tree.set_root(MenuItem::new("Delete", |_ctx| {}));
    measured_layout(&mut tree, 200.0, 60.0);

    let r = tree.layout_rect(item);
    let mut ctx = sindon_widgets::paint::PaintContext::default();
    tree.paint(&mut ctx);
    assert!(!ctx.glyphs.is_empty(), "label should paint glyphs");
    let min_x = ctx.glyphs.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
    let inset = min_x - r.origin.x;
    assert!(
        inset >= 9.0,
        "label should be inset by ~12px, got {inset}px from the row's left edge"
    );
}

#[test]
fn menu_item_drag_off_cancels_click() {
    let fired = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let captured = Rc::clone(&fired);
    let item = tree.set_root(MenuItem::new("Click me", move |_ctx| {
        captured.set(captured.get() + 1);
    }));
    measured_layout(&mut tree, 200.0, 60.0);

    let r = tree.layout_rect(item);
    left_mouse_down(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);
    // MouseLeave clears `pressed` — same path as Dropdown's OptionItem
    // and Button's drag-off cancel. A subsequent MouseUp must not fire
    // the handler.
    dispatch(&mut tree, WidgetEvent::MouseLeave);
    left_mouse_up(&mut tree, r.origin.x + 200.0, r.origin.y + 200.0);

    assert_eq!(fired.get(), 0, "drag-off cancels MenuItem activation");
}

#[test]
fn disabled_menu_item_never_fires() {
    // A disabled row (Tailwind `disabled:opacity-40`) swallows activation: a
    // full press → release cycle must not call the handler.
    let fired = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let captured = Rc::clone(&fired);
    let item = tree.set_root(
        MenuItem::new("Export all notes", move |_ctx| {
            captured.set(captured.get() + 1);
        })
        .disabled(true),
    );
    measured_layout(&mut tree, 200.0, 60.0);

    let r = tree.layout_rect(item);
    dispatch(&mut tree, WidgetEvent::MouseEnter);
    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(r.origin.x + 5.0, r.origin.y + 5.0),
        },
    );
    left_mouse_down(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);
    left_mouse_up(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);

    assert_eq!(fired.get(), 0, "disabled MenuItem never fires on_click");
}

#[test]
fn menu_item_disabled_reactively_gates_click() {
    // The gate is a `Signal` the menu binds to app state — flipping it with no
    // event delivered must change whether the next click fires.
    let fired = Rc::new(Cell::new(0u32));
    let disabled = Signal::new(false);

    let mut tree = WidgetTree::new();
    let captured = Rc::clone(&fired);
    let item = tree.set_root(
        MenuItem::new("Export all notes", move |_ctx| {
            captured.set(captured.get() + 1);
        })
        .disabled(disabled),
    );
    measured_layout(&mut tree, 200.0, 60.0);

    let r = tree.layout_rect(item);
    dispatch(&mut tree, WidgetEvent::MouseEnter);
    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(r.origin.x + 5.0, r.origin.y + 5.0),
        },
    );
    // Enabled: fires.
    left_mouse_down(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);
    left_mouse_up(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);
    assert_eq!(fired.get(), 1, "enabled MenuItem fires");

    // Disabled reactively (no event) → the same click no longer fires.
    disabled.set(true);
    left_mouse_down(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);
    left_mouse_up(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);
    assert_eq!(fired.get(), 1, "disabled MenuItem stops firing");
}

#[test]
fn disabled_menu_item_dims_the_label() {
    // The visible half of the gap: `disabled:opacity-40` dims the label. Compare
    // the painted glyph alpha enabled vs disabled — disabled should be ~half.
    fn max_label_alpha(disabled: bool) -> f32 {
        let mut tree = WidgetTree::new();
        let item = MenuItem::new("Export all notes", |_ctx| {});
        tree.set_root(if disabled { item.disabled(true) } else { item });
        measured_layout(&mut tree, 200.0, 60.0);
        let mut ctx = sindon_widgets::paint::PaintContext::default();
        tree.paint(&mut ctx);
        ctx.glyphs.iter().map(|g| g.color.a).fold(0.0_f32, f32::max)
    }

    let enabled_a = max_label_alpha(false);
    let disabled_a = max_label_alpha(true);
    assert!(enabled_a > 0.0, "enabled label should paint glyphs");
    assert!(
        (disabled_a - enabled_a * 0.5).abs() < 1e-3,
        "disabled label should paint at half alpha: enabled={enabled_a}, disabled={disabled_a}"
    );
}
