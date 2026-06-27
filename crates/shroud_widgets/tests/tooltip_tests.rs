//! FW-13 tooltip primitives.
//!
//! Two halves, tested together because they only make sense as a pair:
//!
//! - `Container::on_hover_enter` / `on_hover_exit` — surface the tree's
//!   internal `MouseEnter`/`MouseLeave` to the app, the enter handler
//!   carrying the container's own layout rect so it can anchor a popover.
//! - `LayerOptions::tooltip()` — a click-through (non-interactive) overlay.
//!   It paints on top but is skipped by event routing, so it does *not*
//!   steal the `MouseLeave` that drives the dismiss. The headline test,
//!   [`tooltip_dismisses_on_hover_exit_end_to_end`], proves that a tip
//!   pushed on enter is reliably popped on exit — the property that breaks
//!   if a tooltip is pushed as an ordinary input-capturing layer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use shroud_core::{Color, Point, Rect, Theme};
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, MouseButton, WidgetEvent};
use shroud_widgets::layer::{LayerAnchor, LayerOptions, Placement};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Button, Container, TextWidget};

fn measured_layout(tree: &mut WidgetTree, w: f32, h: f32) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(w, h, &mut engine, &theme);
}

fn dispatch(tree: &mut WidgetTree, ev: WidgetEvent) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(&ev, &mut ctx);
}

fn mouse_move(tree: &mut WidgetTree, x: f32, y: f32) {
    dispatch(
        tree,
        WidgetEvent::MouseMove {
            position: Point::new(x, y),
        },
    );
}

fn left_down(tree: &mut WidgetTree, x: f32, y: f32) {
    dispatch(
        tree,
        WidgetEvent::MouseDown {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
    );
}

fn center(tree: &WidgetTree, idx: usize) -> (f32, f32) {
    let r = tree.layout_rect(idx);
    (
        r.origin.x + r.size.width / 2.0,
        r.origin.y + r.size.height / 2.0,
    )
}

// ── Container hover callbacks ──────────────────────────────────────────────

#[test]
fn on_hover_enter_fires_with_the_container_rect() {
    let enters = Rc::new(Cell::new(0u32));
    let exits = Rc::new(Cell::new(0u32));
    let last_rect = Rc::new(Cell::new(Rect::new(-1.0, -1.0, -1.0, -1.0)));

    let mut tree = WidgetTree::new();
    // Padding leaves bare root area to move the cursor *off* the trigger.
    let root = tree.set_root(Container::column().width(300.0).height(200.0).padding(40.0));
    let en = Rc::clone(&enters);
    let lr = Rc::clone(&last_rect);
    let ex = Rc::clone(&exits);
    let trigger = tree.add_child(
        root,
        Container::row()
            .width(100.0)
            .height(30.0)
            .on_hover_enter(move |rect, _ctx| {
                en.set(en.get() + 1);
                lr.set(rect);
            })
            .on_hover_exit(move |_ctx| ex.set(ex.get() + 1)),
    );
    measured_layout(&mut tree, 300.0, 200.0);

    let (tx, ty) = center(&tree, trigger);
    mouse_move(&mut tree, tx, ty);
    assert_eq!(enters.get(), 1, "MouseEnter fires on_hover_enter once");
    assert_eq!(exits.get(), 0, "no exit yet");

    let want = tree.layout_rect(trigger);
    let got = last_rect.get();
    assert!(
        (got.origin.x - want.origin.x).abs() < 0.5
            && (got.origin.y - want.origin.y).abs() < 0.5
            && (got.size.width - want.size.width).abs() < 0.5
            && (got.size.height - want.size.height).abs() < 0.5,
        "enter rect {got:?} should equal the trigger layout rect {want:?}"
    );

    // Move into the bare padding → leaves the trigger.
    mouse_move(&mut tree, 5.0, 5.0);
    assert_eq!(exits.get(), 1, "MouseLeave fires on_hover_exit once");
    assert_eq!(enters.get(), 1, "no spurious re-enter");
}

#[test]
fn hover_callbacks_do_not_enable_the_hover_background() {
    // A trigger that sets only a hover callback (no `hoverable`, no
    // hover_background) must not start the hover-bg fade — otherwise every
    // tooltip target would highlight just for carrying a tip. Asserted via
    // the paint command list. Both arms use an instant (ZERO) transition so
    // a *hovered* fill, if any, snaps to full opacity on the first paint —
    // dodging the frame-0 "fade still at t=0" false negative.

    // Control: a hover_background trigger DOES paint its fill once hovered —
    // validates the assertion mechanism in the guard below.
    {
        let mut tree = WidgetTree::new();
        let root = tree.set_root(Container::column().width(120.0).height(80.0));
        tree.add_child(
            root,
            Container::row()
                .width(80.0)
                .height(24.0)
                .hover_background(Color::rgb(0.3, 0.3, 0.3))
                .hover_transition(Duration::ZERO),
        );
        tree.compute_layout(120.0, 80.0);
        mouse_move(&mut tree, 40.0, 12.0);
        let mut ctx = PaintContext::default();
        tree.paint(&mut ctx);
        assert_eq!(
            ctx.rects.len(),
            1,
            "hoverable trigger paints its hover fill"
        );
    }

    // Guard: a callback-only trigger paints nothing after a hover. If a
    // regression made `on_hover_enter` imply `hoverable`, the ZERO-transition
    // fade would snap on and this would emit a fill — failing the test.
    {
        let mut tree = WidgetTree::new();
        let root = tree.set_root(Container::column().width(120.0).height(80.0));
        tree.add_child(
            root,
            Container::row()
                .width(80.0)
                .height(24.0)
                .hover_transition(Duration::ZERO)
                .on_hover_enter(|_rect, _ctx| {}),
        );
        tree.compute_layout(120.0, 80.0);
        mouse_move(&mut tree, 40.0, 12.0);
        let mut ctx = PaintContext::default();
        tree.paint(&mut ctx);
        assert!(
            ctx.rects.is_empty(),
            "a hover callback alone must not paint a hover background"
        );
    }
}

// ── Non-interactive (click-through) layer ──────────────────────────────────

/// Build a root with a single `on_press` panel near the top-left, plus a
/// pushed layer with the given options centered in the viewport. Returns
/// (tree, panel_idx, layer_idx, press_count).
fn tree_with_press_panel_and_layer(
    options: LayerOptions,
) -> (WidgetTree, usize, usize, Rc<Cell<u32>>) {
    let presses = Rc::new(Cell::new(0u32));
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(300.0).height(200.0));
    let p = Rc::clone(&presses);
    let panel = tree.add_child(
        root,
        Container::row()
            .width(80.0)
            .height(30.0)
            .background(Color::rgb(0.2, 0.2, 0.2))
            .on_press(move |_pos, _ctx| p.set(p.get() + 1)),
    );
    let layer = tree.push_layer(
        options,
        Container::column()
            .width(120.0)
            .height(60.0)
            .background(Color::rgb(0.2, 0.2, 0.3)),
    );
    measured_layout(&mut tree, 300.0, 200.0);
    (tree, panel, layer, presses)
}

#[test]
fn tooltip_layer_is_click_through() {
    let (mut tree, panel, _layer, presses) =
        tree_with_press_panel_and_layer(LayerOptions::tooltip());
    assert_eq!(tree.layer_count(), 1, "tooltip layer is up");

    // The panel sits at the top-left, well clear of the centered layer. A
    // click on it must reach the main tree even though a layer is painted.
    let (px, py) = center(&tree, panel);
    left_down(&mut tree, px, py);

    assert_eq!(presses.get(), 1, "click passes through the tooltip layer");
    assert_eq!(
        tree.layer_count(),
        1,
        "a non-interactive layer is not dismissed by clicks elsewhere"
    );
}

#[test]
fn interactive_popover_is_not_click_through() {
    // Contrast with the tooltip case: an ordinary interactive popover
    // swallows the same click and dismisses (its outside-click path), so
    // the panel never sees it. This is exactly why a tooltip must be
    // non-interactive.
    let (mut tree, panel, _layer, presses) =
        tree_with_press_panel_and_layer(LayerOptions::popover());
    assert_eq!(tree.layer_count(), 1);

    let (px, py) = center(&tree, panel);
    left_down(&mut tree, px, py);

    assert_eq!(presses.get(), 0, "interactive layer swallows the click");
    assert_eq!(tree.layer_count(), 0, "outside-click dismisses the popover");
}

#[test]
fn tooltip_layer_skipped_for_tab_routing() {
    // Tab traversal must keep targeting the main tree's focusables even
    // while a (focusless) tooltip overlay is painted on top.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(120.0).gap(8.0));
    let a = tree.add_child(root, Button::new("A"));
    let b = tree.add_child(root, Button::new("B"));
    tree.push_layer(
        LayerOptions::tooltip(),
        Container::column()
            .width(80.0)
            .height(30.0)
            .background(Color::rgb(0.2, 0.2, 0.3)),
    );
    measured_layout(&mut tree, 200.0, 120.0);

    let order = tree.focusable_in_tab_order();
    assert_eq!(
        order,
        vec![a, b],
        "Tab order is the main tree's buttons, not the tooltip layer"
    );
}

// ── End-to-end: hover shows + leave hides ──────────────────────────────────

#[test]
fn tooltip_dismisses_on_hover_exit_end_to_end() {
    // The whole point of FW-13: on_hover_enter pushes a tooltip() layer,
    // on_hover_exit pops it. Because the tooltip is click-through, the
    // trigger keeps receiving MouseMove → MouseLeave, so the tip is
    // reliably torn down. The text records the lifecycle.
    let log: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(300.0).height(200.0).padding(40.0));

    let enter_log = Rc::clone(&log);
    let exit_log = Rc::clone(&log);
    let trigger = tree.add_child(
        root,
        Container::row()
            .width(100.0)
            .height(30.0)
            .on_hover_enter(move |rect, ctx| {
                enter_log.borrow_mut().push("show");
                ctx.push_layer(
                    LayerOptions::tooltip().anchor(LayerAnchor::AnchorRect {
                        rect,
                        prefer: Placement::Below,
                    }),
                    Container::column()
                        .padding(6.0)
                        .background(Color::rgb(0.1, 0.1, 0.1)),
                    |tree, layer_root| {
                        tree.add_child(layer_root, TextWidget::new("Bold"));
                    },
                );
            })
            .on_hover_exit(move |ctx| {
                exit_log.borrow_mut().push("hide");
                ctx.pop_top_layer();
            }),
    );
    measured_layout(&mut tree, 300.0, 200.0);

    // Hover the trigger → tip appears.
    let (tx, ty) = center(&tree, trigger);
    mouse_move(&mut tree, tx, ty);
    assert_eq!(tree.layer_count(), 1, "tip shown on hover");
    measured_layout(&mut tree, 300.0, 200.0); // place + measure the layer

    // A tiny wiggle still over the trigger must NOT thrash the tip: no new
    // enter/exit (hover target unchanged), layer stays exactly one.
    mouse_move(&mut tree, tx + 2.0, ty + 1.0);
    assert_eq!(
        tree.layer_count(),
        1,
        "tip stable while still on the trigger"
    );

    // Move off the trigger into the bare padding → tip dismisses.
    mouse_move(&mut tree, 5.0, 5.0);
    assert_eq!(tree.layer_count(), 0, "tip dismissed on leave");

    assert_eq!(
        *log.borrow(),
        vec!["show", "hide"],
        "exactly one show/hide cycle"
    );
}
