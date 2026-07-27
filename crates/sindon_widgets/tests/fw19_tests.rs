//! FW-19 minimal (G6 / G4 / G12): Button padding / min-width / disabled,
//! Container asymmetric padding, and Input font-weight + reactive chrome.
//!
//! The bold-caret *geometry* plumbing (that the caret / selection track the
//! shaped weight) is covered at the engine level in
//! `sindon_text/tests/text_engine_tests.rs`; here we exercise the widget-facing
//! builders and behaviors that graduate from the knot-ui-repro backlog.

use sindon_core::{Color, Point, Theme};
use sindon_reactive::{Reactive, Signal};
use sindon_text::TextEngine;
use sindon_widgets::event::{EventContext, MouseButton, WidgetEvent};
use sindon_widgets::paint::PaintContext;
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

// ── G4: Container asymmetric padding ──────────────────────────────

#[test]
fn container_padding_xy_insets_child_per_axis() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(200.0)
            .height(120.0)
            .padding_xy(24.0, 8.0),
    );
    // Fixed-size child so cross-axis stretch doesn't move its origin.
    let child = tree.add_child(root, Container::column().width(40.0).height(20.0));
    tree.compute_layout(400.0, 300.0);

    let r = tree.layout_rect(root);
    let c = tree.layout_rect(child);
    assert!(
        (c.origin.x - (r.origin.x + 24.0)).abs() < 0.5,
        "px-6 should inset the child 24px, got {}",
        c.origin.x - r.origin.x
    );
    assert!(
        (c.origin.y - (r.origin.y + 8.0)).abs() < 0.5,
        "py-2 should inset the child 8px, got {}",
        c.origin.y - r.origin.y
    );
}

#[test]
fn container_padding_trbl_insets_each_edge() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(200.0)
            .height(120.0)
            .padding_trbl(4.0, 8.0, 12.0, 16.0),
    );
    let child = tree.add_child(root, Container::column().width(20.0).height(20.0));
    tree.compute_layout(400.0, 300.0);

    let r = tree.layout_rect(root);
    let c = tree.layout_rect(child);
    // The child's top-left is offset by (left, top) = (16, 4).
    assert!(
        (c.origin.x - (r.origin.x + 16.0)).abs() < 0.5,
        "left padding should inset 16px, got {}",
        c.origin.x - r.origin.x
    );
    assert!(
        (c.origin.y - (r.origin.y + 4.0)).abs() < 0.5,
        "top padding should inset 4px, got {}",
        c.origin.y - r.origin.y
    );
}

#[test]
fn container_padding_xy_clamps_negative() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(100.0)
            .height(100.0)
            .padding_xy(-5.0, -9.0),
    );
    let child = tree.add_child(root, Container::column().width(10.0).height(10.0));
    tree.compute_layout(400.0, 300.0);

    let r = tree.layout_rect(root);
    let c = tree.layout_rect(child);
    assert!(
        (c.origin.x - r.origin.x).abs() < 0.5,
        "negative x padding clamps to 0"
    );
    assert!(
        (c.origin.y - r.origin.y).abs() < 0.5,
        "negative y padding clamps to 0"
    );
}

// ── G6: Button padding / min-width / disabled ─────────────────────

// A measured button's box height is its content height plus the vertical
// padding Taffy adds. Bumping `padding_y` from 8 to 16 must add exactly the
// extra 2 * (16 - 8) = 16px, without disturbing the content measurement.
#[test]
fn button_padding_y_grows_box_height_by_twice_the_delta() {
    fn box_height(pad_y: f32) -> f32 {
        let mut tree = WidgetTree::new();
        let root = tree.set_root(Container::column().width(300.0).height(200.0));
        let btn = tree.add_child(root, Button::new("Save").padding_y(pad_y));
        let mut engine = TextEngine::new();
        let theme = Theme::default();
        tree.compute_layout_with_measure(600.0, 400.0, &mut engine, &theme);
        tree.layout_rect(btn).size.height
    }
    let default = box_height(8.0);
    let tall = box_height(16.0);
    assert!(
        (tall - default - 16.0).abs() < 1.0,
        "py 8→16 should add 16px of height, got {}",
        tall - default
    );
}

// `min_width` is a floor: a single-glyph button that would size to ~a dozen
// pixels is widened to the floor, while a plain button stays narrow.
#[test]
fn button_min_width_floors_box_width() {
    fn box_width(min_w: Option<f32>) -> f32 {
        let mut tree = WidgetTree::new();
        // Row so the button hugs its main-axis content instead of stretching.
        let root = tree.set_root(Container::row().width(400.0).height(60.0));
        let mut btn = Button::new("B");
        if let Some(w) = min_w {
            btn = btn.min_width(w);
        }
        let node = tree.add_child(root, btn);
        let mut engine = TextEngine::new();
        let theme = Theme::default();
        tree.compute_layout_with_measure(600.0, 400.0, &mut engine, &theme);
        tree.layout_rect(node).size.width
    }
    let natural = box_width(None);
    let floored = box_width(Some(120.0));
    assert!(
        natural < 120.0,
        "a single-glyph button should be narrower than the floor, got {natural}"
    );
    assert!(
        floored >= 120.0,
        "min_width(120) should floor the box width, got {floored}"
    );
}

#[test]
fn disabled_button_does_not_fire_click() {
    let clicked = Rc::new(Cell::new(false));
    let sink = clicked.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(100.0));
    tree.add_child(
        root,
        Button::new("Go")
            .disabled(true)
            .on_click(move |_ctx| sink.set(true)),
    );
    tree.compute_layout(400.0, 300.0);

    let mut ec = EventContext::new();
    press_and_release(&mut tree, &mut ec, Point::new(20.0, 10.0));
    assert!(!clicked.get(), "a disabled button must not fire on_click");
}

// The disabled gate is reactive: flipping the bound signal re-enables the
// button on the very next dispatch (it's read live in `event`, no relayout).
#[test]
fn reactive_disabled_gates_click() {
    let count = Rc::new(Cell::new(0u32));
    let sink = count.clone();
    let gate = Signal::new(true);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(100.0));
    tree.add_child(
        root,
        Button::new("Go")
            .disabled(gate)
            .on_click(move |_ctx| sink.set(sink.get() + 1)),
    );
    tree.compute_layout(400.0, 300.0);

    let mut ec = EventContext::new();
    press_and_release(&mut tree, &mut ec, Point::new(20.0, 10.0));
    assert_eq!(count.get(), 0, "click blocked while disabled");

    gate.set(false);
    press_and_release(&mut tree, &mut ec, Point::new(20.0, 10.0));
    assert_eq!(count.get(), 1, "click fires once the gate opens");
}

// With no explicit `disabled_background`, a disabled button dims its fill to
// half alpha (a theme-agnostic "greyed out"); enabled it paints opaque.
#[test]
fn disabled_button_halves_background_alpha() {
    fn bg_alpha(disabled: bool) -> f32 {
        let mut tree = WidgetTree::new();
        let root = tree.set_root(Container::column().width(120.0).height(60.0));
        tree.add_child(
            root,
            Button::new("Go")
                .background(Color::rgb(0.1, 0.4, 0.9))
                .disabled(disabled),
        );
        tree.compute_layout(300.0, 200.0);
        let mut ctx = PaintContext::default();
        tree.paint(&mut ctx);
        // The root column has no fill, so the first rect is the button's bg.
        ctx.rects[0].color.a
    }
    assert!(
        (bg_alpha(false) - 1.0).abs() < 1e-6,
        "enabled bg stays opaque"
    );
    assert!(
        (bg_alpha(true) - 0.5).abs() < 1e-6,
        "disabled bg dims to half alpha"
    );
}

// A button constructed already-disabled snaps to the disabled fill on its very
// first paint — a form that loads already-invalid shows its submit greyed from
// the start rather than animating the dim in on mount.
#[test]
fn disabled_from_construction_does_not_fade_in() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(120.0).height(60.0));
    tree.add_child(
        root,
        Button::new("Go")
            .background(Color::rgb(0.1, 0.4, 0.9))
            .disabled(true),
    );
    tree.compute_layout(300.0, 200.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert!(
        (ctx.rects[0].color.a - 0.5).abs() < 1e-6,
        "first paint snaps to the disabled fill, got alpha {}",
        ctx.rects[0].color.a
    );
}

// Flipping the disabled signal *eases* rather than snaps: with the default fade,
// the frame right after the flip still paints essentially the enabled (opaque)
// fill — the 120 ms transition has only just begun — instead of jumping to the
// half-alpha disabled fill. Mirrors the reference's `transition-colors`.
#[test]
fn disabled_change_eases_not_snaps() {
    let gate = Signal::new(false);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(120.0).height(60.0));
    tree.add_child(
        root,
        Button::new("Go")
            .background(Color::rgb(0.1, 0.4, 0.9))
            .disabled(gate),
    );
    tree.compute_layout(300.0, 200.0);

    // Frame 1, enabled: primes the fade to 0 and paints an opaque fill.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert!(
        (ctx.rects[0].color.a - 1.0).abs() < 1e-6,
        "enabled paints opaque"
    );

    // Flip to disabled and paint again immediately: the fade has barely
    // advanced, so the fill is still ~opaque — it has *not* snapped to 0.5.
    gate.set(true);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(
        ctx2.rects[0].color.a > 0.9,
        "disabled fade eases from the enabled fill, got alpha {}",
        ctx2.rects[0].color.a
    );
}

// `disabled_transition(ZERO)` restores the pre-animation instant swap: the frame
// right after the flip paints the fully-disabled half-alpha fill, no in-between.
#[test]
fn disabled_transition_zero_snaps() {
    let gate = Signal::new(false);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(120.0).height(60.0));
    tree.add_child(
        root,
        Button::new("Go")
            .background(Color::rgb(0.1, 0.4, 0.9))
            .disabled(gate)
            .disabled_transition(Duration::ZERO),
    );
    tree.compute_layout(300.0, 200.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    gate.set(true);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(
        (ctx2.rects[0].color.a - 0.5).abs() < 1e-6,
        "ZERO transition swaps instantly to half alpha, got {}",
        ctx2.rects[0].color.a
    );
}

// The disabled *behavior* — dropping out of the Tab order — switches instantly
// regardless of the color fade still being in flight.
#[test]
fn disabled_focusable_switches_instantly_during_fade() {
    let gate = Signal::new(false);
    let button = Button::new("Go").disabled(gate);
    assert!(button.focusable(), "enabled button is focusable");
    gate.set(true);
    assert!(
        !button.focusable(),
        "focusable() flips the instant disabled is set, not after the fade"
    );
}

// ── G12: Input reactive chrome ────────────────────────────────────

// The color setters now take `Reactive<Color>`, so an explicit background can
// track a live theme swap instead of freezing at its construction value.
#[test]
fn input_background_follows_its_signal() {
    let bg = Signal::new(Color::rgb(1.0, 0.0, 0.0));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    tree.add_child(root, Input::new().background(bg));
    tree.compute_layout(400.0, 300.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    let first = ctx.rects[0].color;
    assert!(
        (first.r - 1.0).abs() < 1e-6 && first.g < 1e-6,
        "input bg should start at the signal's red"
    );

    bg.set(Color::rgb(0.0, 1.0, 0.0));
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    let second = ctx2.rects[0].color;
    assert!(
        second.g > 0.99 && second.r < 1e-6,
        "input bg should track the signal to green"
    );
}

// A `Reactive::derive` closure is accepted by the chrome setters (the point of
// the `impl Into<Reactive<Color>>` conversion) — mirrors the `TextWidget` test.
#[test]
fn input_chrome_setters_accept_reactive_derive() {
    let _input = Input::new()
        .background(Reactive::derive(|| Color::rgb(0.1, 0.1, 0.1)))
        .border_color(Reactive::derive(|| Color::rgb(0.2, 0.2, 0.2)))
        .text_color(Reactive::derive(|| Color::WHITE));
}

fn press_and_release(tree: &mut WidgetTree, ec: &mut EventContext, at: Point) {
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: at,
            button: MouseButton::Left,
        },
        ec,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: at,
            button: MouseButton::Left,
        },
        ec,
    );
}
