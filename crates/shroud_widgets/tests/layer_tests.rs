//! Overlay-layer integration tests for `WidgetTree`.
//!
//! These exercise the Phase 21 modal/layer primitive end-to-end:
//! - push/pop semantics + tombstoning the layer subtree
//! - paint pipeline (scrim + layer subtree on top of main)
//! - ViewportCenter anchor positions the layer in viewport coords
//! - event routing while a layer is up (in/outside, dismiss paths)
//! - Tab routing trapped inside the topmost layer

use std::cell::Cell;
use std::rc::Rc;

use shroud_core::{Color, Point, Theme};
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::layer::{LayerAnchor, LayerOptions};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_reactive::Signal;
use shroud_widgets::{Button, Container, Input, TextWidget};

/// Run layout with measure so leaf widgets (Button, TextWidget) get an
/// intrinsic size — without this, Button collapses to 0x0 and click
/// hit-tests miss.
fn measured_layout(tree: &mut WidgetTree, w: f32, h: f32) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(w, h, &mut engine, &theme);
}

// ── helpers ───────────────────────────────────────────────────────

fn click_counter() -> (Rc<Cell<u32>>, Rc<Cell<u32>>) {
    (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)))
}

/// Build a tree with `main` root + a small layer using ViewportCenter
/// + the supplied `LayerOptions`. Returns (tree, main_root, layer_root).
fn tree_with_layer(options: LayerOptions) -> (WidgetTree, usize, usize) {
    let mut tree = WidgetTree::new();
    let main = tree.set_root(
        Container::column()
            .width(800.0)
            .height(600.0)
            .background(Color::rgb(0.1, 0.1, 0.1)),
    );
    tree.add_child(main, Container::column().width(50.0).height(50.0));
    let layer = tree.push_layer(
        options,
        Container::column()
            .width(200.0)
            .height(100.0)
            .background(Color::rgb(0.2, 0.2, 0.3)),
    );
    (tree, main, layer)
}

fn dispatch(tree: &mut WidgetTree, ev: WidgetEvent) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(&ev, &mut ctx);
}

// ── basic push/pop ────────────────────────────────────────────────

#[test]
fn push_layer_registers_in_stack() {
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(400.0).height(300.0));
    assert_eq!(tree.layer_count(), 0);

    let layer = tree.push_layer(
        LayerOptions::default(),
        Container::column().width(120.0).height(80.0),
    );
    assert_eq!(tree.layer_count(), 1);
    assert_eq!(tree.top_layer_root(), Some(layer));
    assert!(tree.contains(layer));
}

#[test]
fn pop_top_layer_tombstones_subtree() {
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(400.0).height(300.0));
    let layer = tree.push_layer(
        LayerOptions::default(),
        Container::column().width(120.0).height(80.0),
    );
    tree.add_child(layer, Container::row().width(40.0).height(40.0));
    let main_len_before = tree.len();
    assert!(main_len_before >= 3); // root + layer root + layer child

    let popped = tree.pop_top_layer();
    assert_eq!(popped, Some(layer));
    assert!(!tree.contains(layer));
    assert_eq!(tree.layer_count(), 0);
}

#[test]
fn pop_layer_by_root_removes_specific() {
    // Two layers; pop the bottom one and confirm the top survives.
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(400.0).height(300.0));
    let lower = tree.push_layer(
        LayerOptions::default(),
        Container::column().width(80.0).height(40.0),
    );
    let upper = tree.push_layer(
        LayerOptions::default(),
        Container::column().width(80.0).height(40.0),
    );

    assert!(tree.pop_layer(lower));
    assert!(!tree.contains(lower));
    assert!(tree.contains(upper));
    assert_eq!(tree.layer_count(), 1);
    assert_eq!(tree.top_layer_root(), Some(upper));
}

#[test]
fn remove_layer_root_directly_clears_layer_entry() {
    // Belt-and-braces: if a user reaches `WidgetTree::remove` directly
    // with the layer root, the LayerEntry must drop too so paint /
    // dispatch don't iterate over a tombstoned slot.
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(400.0).height(300.0));
    let layer = tree.push_layer(
        LayerOptions::default(),
        Container::column().width(60.0).height(40.0),
    );
    assert_eq!(tree.layer_count(), 1);
    tree.remove(layer);
    assert_eq!(tree.layer_count(), 0);
}

// ── layout + paint ────────────────────────────────────────────────

#[test]
fn viewport_center_positions_layer_at_center() {
    let (mut tree, _main, _layer) = tree_with_layer(LayerOptions::modal());
    tree.compute_layout(800.0, 600.0);

    // Container only emits a rect when it has a background, so we expect:
    // main (bg) + scrim + layer (bg) = 3 rects. The main child has no
    // background.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(ctx.rects.len(), 3);

    // The layer's own rect is the last one — its top-left equals the
    // computed anchor offset (800-200)/2 = 300, (600-100)/2 = 250.
    let last = ctx.rects.last().unwrap();
    assert!((last.x - 300.0).abs() < 0.5, "layer x = {}, want ~300", last.x);
    assert!((last.y - 250.0).abs() < 0.5, "layer y = {}, want ~250", last.y);
    assert_eq!(last.width, 200.0);
    assert_eq!(last.height, 100.0);
}

#[test]
fn modal_scrim_covers_viewport() {
    let (mut tree, _main, _layer) = tree_with_layer(LayerOptions::modal());
    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // Scrim is painted just before the layer content. Find it by looking
    // for the rect that spans the full viewport.
    let scrim = ctx
        .rects
        .iter()
        .find(|r| r.width == 800.0 && r.height == 600.0 && r.color.a > 0.0 && r.color.a < 1.0)
        .expect("scrim rect");
    assert_eq!(scrim.x, 0.0);
    assert_eq!(scrim.y, 0.0);
    assert!(scrim.color.a < 1.0, "scrim should be semi-transparent");
}

#[test]
fn popover_paints_no_scrim() {
    let (mut tree, _main, _layer) = tree_with_layer(LayerOptions::popover());
    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // No semi-transparent full-viewport rect.
    let has_scrim = ctx
        .rects
        .iter()
        .any(|r| r.width == 800.0 && r.height == 600.0 && r.color.a > 0.0 && r.color.a < 1.0);
    assert!(!has_scrim, "popover preset must not paint a scrim");
}

#[test]
fn no_layer_paints_only_main_tree() {
    // Regression: paint() must still work with zero layers active.
    let mut tree = WidgetTree::new();
    let _root = tree.set_root(
        Container::column()
            .width(400.0)
            .height(300.0)
            .background(Color::BLACK),
    );
    tree.compute_layout(400.0, 300.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(ctx.rects.len(), 1);
}

// ── event routing ─────────────────────────────────────────────────

#[test]
fn click_inside_layer_dispatches_to_layer_widget() {
    let (main_clicks, layer_clicks) = click_counter();

    let mut tree = WidgetTree::new();
    let main_root = tree.set_root(Container::column().width(800.0).height(600.0));
    let mc = Rc::clone(&main_clicks);
    tree.add_child(
        main_root,
        Button::new("main").background(Color::WHITE).on_click(move |_| {
            mc.set(mc.get() + 1);
        }),
    );

    let lc = Rc::clone(&layer_clicks);
    let layer_root = tree.push_layer(
        LayerOptions::modal(),
        Container::column().width(200.0).height(100.0).center(),
    );
    tree.add_child(
        layer_root,
        Button::new("layer").background(Color::WHITE).on_click(move |_| {
            lc.set(lc.get() + 1);
        }),
    );

    // Use the measure path so Button sizes from its label.
    measured_layout(&mut tree, 800.0, 600.0);

    // Click in the center of the viewport — lands on the centered layer
    // button.
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(400.0, 300.0),
            button: MouseButton::Left,
        },
    );
    dispatch(
        &mut tree,
        WidgetEvent::MouseUp {
            position: Point::new(400.0, 300.0),
            button: MouseButton::Left,
        },
    );

    assert_eq!(layer_clicks.get(), 1, "layer button should receive click");
    assert_eq!(main_clicks.get(), 0, "main button must not see layer-scope click");
}

#[test]
fn outside_click_dismisses_when_configured() {
    let (mut tree, _main, layer) = tree_with_layer(LayerOptions::modal());
    tree.compute_layout(800.0, 600.0);
    assert!(tree.contains(layer));

    // Click at top-left corner — outside the centered 200x100 layer.
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(5.0, 5.0),
            button: MouseButton::Left,
        },
    );

    assert_eq!(tree.layer_count(), 0, "outside click should dismiss");
    assert!(!tree.contains(layer));
}

#[test]
fn outside_click_no_dismiss_when_disabled() {
    let opts = LayerOptions::modal().dismiss_on_outside_click(false);
    let (mut tree, _main, layer) = tree_with_layer(opts);
    tree.compute_layout(800.0, 600.0);

    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(5.0, 5.0),
            button: MouseButton::Left,
        },
    );
    assert_eq!(tree.layer_count(), 1, "non-dismissable layer must persist");
    assert!(tree.contains(layer));
}

#[test]
fn outside_click_does_not_reach_main_tree() {
    let main_clicks = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let main_root = tree.set_root(Container::column().width(800.0).height(600.0));
    let mc = Rc::clone(&main_clicks);
    tree.add_child(
        main_root,
        Button::new("main")
            .background(Color::WHITE)
            .on_click(move |_| mc.set(mc.get() + 1)),
    );

    let opts = LayerOptions::modal().dismiss_on_outside_click(false);
    tree.push_layer(
        opts,
        Container::column().width(50.0).height(40.0),
    );

    tree.compute_layout(800.0, 600.0);

    // Outside the layer rect, where the main button would otherwise be.
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(5.0, 5.0),
            button: MouseButton::Left,
        },
    );
    dispatch(
        &mut tree,
        WidgetEvent::MouseUp {
            position: Point::new(5.0, 5.0),
            button: MouseButton::Left,
        },
    );
    assert_eq!(main_clicks.get(), 0, "main tree must not see swallowed click");
}

#[test]
fn escape_dismisses_layer() {
    let (mut tree, _main, _layer) = tree_with_layer(LayerOptions::modal());
    tree.compute_layout(800.0, 600.0);

    dispatch(
        &mut tree,
        WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Escape),
        },
    );
    assert_eq!(tree.layer_count(), 0);
}

#[test]
fn escape_no_dismiss_when_disabled() {
    let opts = LayerOptions::modal().dismiss_on_escape(false);
    let (mut tree, _main, _layer) = tree_with_layer(opts);
    tree.compute_layout(800.0, 600.0);

    dispatch(
        &mut tree,
        WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Escape),
        },
    );
    assert_eq!(tree.layer_count(), 1, "Escape must not dismiss when disabled");
}

#[test]
fn keyboard_input_does_not_reach_main_input_while_layer_up() {
    // Inspect via the bound Signal — the canonical bidirectional path
    // gives us a Widget-trait-agnostic way to read the input's buffer.
    let value = Signal::new(String::new());
    let mut tree = WidgetTree::new();
    let main_root = tree.set_root(Container::column().width(800.0).height(600.0));
    let main_input = tree.add_child(main_root, Input::new().value(value).placeholder("main"));

    // Focus the main input first (no layer yet).
    let mut ctx = EventContext::new();
    tree.focus(Some(main_input), &mut ctx);
    assert_eq!(tree.focused(), Some(main_input));

    // Push a layer; main input keeps focus pointer, but events must not
    // reach it.
    tree.push_layer(
        LayerOptions::modal(),
        Container::column().width(100.0).height(80.0),
    );
    tree.compute_layout(800.0, 600.0);

    dispatch(&mut tree, WidgetEvent::CharInput { ch: 'a' });
    assert_eq!(
        value.get_clone(),
        "",
        "main Input must not receive char input under a layer"
    );
}

// ── tab routing trap ──────────────────────────────────────────────

#[test]
fn tab_routing_traps_inside_top_layer() {
    let mut tree = WidgetTree::new();
    let main_root = tree.set_root(Container::column().width(800.0).height(600.0));
    let main_input = tree.add_child(main_root, Input::new().placeholder("main"));
    let _other = tree.add_child(main_root, Input::new().placeholder("main-2"));

    let layer = tree.push_layer(
        LayerOptions::modal(),
        Container::column().width(200.0).height(120.0),
    );
    let layer_input_a = tree.add_child(layer, Input::new().placeholder("layer-a"));
    let layer_input_b = tree.add_child(layer, Input::new().placeholder("layer-b"));

    tree.compute_layout(800.0, 600.0);

    // First Tab in a layered tree: lands on the first focusable in
    // the layer (DFS order), not anything in main.
    let mut ctx = EventContext::new();
    tree.advance_focus(shroud_widgets::FocusDirection::Forward, &mut ctx);
    assert_eq!(tree.focused(), Some(layer_input_a));

    // Tab again → next focusable in layer; wraps within the layer.
    tree.advance_focus(shroud_widgets::FocusDirection::Forward, &mut ctx);
    assert_eq!(tree.focused(), Some(layer_input_b));
    tree.advance_focus(shroud_widgets::FocusDirection::Forward, &mut ctx);
    assert_eq!(tree.focused(), Some(layer_input_a));

    // main_input must never appear in the trapped order.
    let order = tree.focusable_in_tab_order();
    assert!(!order.contains(&main_input));
}

// ── EventContext-driven push/pop ──────────────────────────────────

#[test]
fn handler_push_layer_via_event_context() {
    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(
        main,
        Button::new("open").background(Color::WHITE).on_click(|ctx| {
            ctx.push_layer(
                LayerOptions::modal(),
                Container::column().width(120.0).height(60.0),
                |_tree, _root| {},
            );
        }),
    );
    tree.compute_layout(400.0, 300.0);

    assert_eq!(tree.layer_count(), 0);
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(50.0, 20.0),
            button: MouseButton::Left,
        },
    );
    dispatch(
        &mut tree,
        WidgetEvent::MouseUp {
            position: Point::new(50.0, 20.0),
            button: MouseButton::Left,
        },
    );
    assert_eq!(tree.layer_count(), 1);
}

#[test]
fn handler_pop_top_layer_via_event_context() {
    let mut tree = WidgetTree::new();
    let _main = tree.set_root(Container::column().width(400.0).height(300.0));
    let layer = tree.push_layer(
        LayerOptions::popover().dismiss_on_outside_click(false),
        Container::column().width(200.0).height(100.0).center(),
    );
    tree.add_child(
        layer,
        Button::new("close").background(Color::WHITE).on_click(|ctx| {
            ctx.pop_top_layer();
        }),
    );
    measured_layout(&mut tree, 400.0, 300.0);

    assert_eq!(tree.layer_count(), 1);

    // Click the close button at viewport center (layer centered, button
    // centered inside the layer).
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(200.0, 150.0),
            button: MouseButton::Left,
        },
    );
    dispatch(
        &mut tree,
        WidgetEvent::MouseUp {
            position: Point::new(200.0, 150.0),
            button: MouseButton::Left,
        },
    );
    assert_eq!(tree.layer_count(), 0);
}

#[test]
fn anchor_default_is_viewport_center() {
    let opts = LayerOptions::default();
    assert!(matches!(opts.anchor, LayerAnchor::ViewportCenter));
}

#[test]
fn layer_starts_recorded_for_each_layer() {
    // Renderer-facing contract: every `push_layer` produces a
    // breakpoint in `PaintContext::layer_starts` so the wgpu pass
    // flushes rects → glyphs per layer in z order. A modal with text
    // contributes both rect (background) and glyph entries, so the
    // recorded snapshot must be non-zero on both axes for a normal
    // tree with content above the layer.
    let mut tree = WidgetTree::new();
    let _main = tree.set_root(
        Container::column()
            .width(800.0)
            .height(600.0)
            .background(Color::BLACK),
    );
    tree.add_child(_main, TextWidget::new("background label").font_size(16.0));
    let layer = tree.push_layer(
        LayerOptions::modal(),
        Container::column().padding(16.0).background(Color::WHITE),
    );
    tree.add_child(layer, TextWidget::new("layer text").font_size(16.0));
    measured_layout(&mut tree, 800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let starts = ctx.layer_starts();
    assert_eq!(starts.len(), 1, "one layer → one breakpoint");
    let (r0, g0, _s0) = starts[0];
    // Main contributed at least the root rect and the label's glyphs.
    assert!(r0 >= 1, "main batch rect count = {r0}");
    assert!(g0 >= 1, "main batch glyph count = {g0}");
    // The layer batch follows the breakpoint and contributes its own
    // commands (scrim + layer bg + layer text glyphs).
    assert!(ctx.rects.len() > r0);
    assert!(ctx.glyphs.len() > g0);
}

#[test]
fn layer_text_painted_after_scrim() {
    // Order check: scrim → layer background → layer text glyphs.
    let mut tree = WidgetTree::new();
    let _main = tree.set_root(
        Container::column()
            .width(800.0)
            .height(600.0)
            .background(Color::rgb(0.0, 0.0, 0.0)),
    );
    let layer = tree.push_layer(
        LayerOptions::modal(),
        Container::column().padding(10.0).background(Color::WHITE),
    );
    tree.add_child(layer, TextWidget::new("Hi").font_size(20.0));

    tree.compute_layout(800.0, 600.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // Find the scrim by looking for a semi-transparent full-viewport rect.
    let scrim_pos = ctx
        .rects
        .iter()
        .position(|r| r.width == 800.0 && r.height == 600.0 && r.color.a < 1.0)
        .expect("scrim rect");
    // The layer container rect comes after the scrim.
    let layer_bg = ctx
        .rects
        .iter()
        .rposition(|r| r.color.r == 1.0 && r.color.g == 1.0 && r.color.b == 1.0)
        .expect("layer bg rect");
    assert!(
        layer_bg > scrim_pos,
        "layer background must paint after scrim (got bg at {layer_bg}, scrim at {scrim_pos})"
    );
    assert!(!ctx.glyphs.is_empty(), "layer text should produce glyphs");
}
