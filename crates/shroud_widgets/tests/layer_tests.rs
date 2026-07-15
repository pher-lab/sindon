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

use shroud_core::Rect;
use shroud_core::{Color, Point, Theme};
use shroud_reactive::Signal;
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::layer::{HAlign, LayerAnchor, LayerOptions, Placement, VAlign};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::shortcut::Shortcut;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Button, Container, Input, ScrollView, TextWidget};

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

// ── G15: interactive-layer push resets the trigger's hover ────────────
//
// A hoverable trigger (gear / ⋮ menu button) that opens an interactive
// popover on click used to stay stuck in its hover highlight after the
// popover closed. Root cause: pushing an interactive layer nulled the
// tree-wide `hovered` pointer *silently* — the trigger never received the
// paired MouseLeave, so its hover visual (`Container`'s fade) was orphaned
// at "hovered". A probe (`entered=1, exited=0, hovered=None` after push)
// confirmed the pointer was live at push time, refuting the old "hovered was
// already None" diagnosis. The fix emits a real MouseLeave to the live hover
// chain just before the push nulls the pointer.
#[test]
fn g15_interactive_layer_push_clears_trigger_hover() {
    let entered = Rc::new(Cell::new(0u32));
    let exited = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(800.0).height(600.0));
    let trigger = {
        let e = entered.clone();
        let x = exited.clone();
        tree.add_child(
            main,
            Container::column()
                .width(50.0)
                .height(50.0)
                .hoverable()
                .on_hover_enter(move |_rect, _ctx| e.set(e.get() + 1))
                .on_hover_exit(move |_ctx| x.set(x.get() + 1))
                .on_press(|_pos, ctx| {
                    ctx.push_layer(
                        LayerOptions::popover(),
                        Container::column().width(120.0).height(80.0),
                        |_t, _root| {},
                    );
                }),
        )
    };

    measured_layout(&mut tree, 800.0, 600.0);

    // 1) Cursor moves over the trigger → hovered + MouseEnter.
    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(25.0, 25.0),
        },
    );
    assert_eq!(tree.hovered(), Some(trigger), "trigger hovered after move");
    assert_eq!(entered.get(), 1, "trigger got MouseEnter");
    assert_eq!(exited.get(), 0, "no MouseLeave yet");

    // 2) Press the trigger → on_press pushes an interactive popover layer.
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(25.0, 25.0),
            button: MouseButton::Left,
        },
    );
    assert_eq!(tree.layer_count(), 1, "interactive layer is up");

    // 3) The fix: the trigger got its paired MouseLeave (hover visual reset),
    //    and the tree-wide pointer is cleared so no stale leave fires at pop.
    assert_eq!(tree.hovered(), None, "push clears the hover pointer");
    assert_eq!(
        exited.get(),
        1,
        "trigger received its MouseLeave → hover visual reset (G15 fixed)"
    );
    assert_eq!(entered.get(), 1, "no spurious re-enter");
}

// A *non-interactive* (click-through) layer — the tooltip preset — must NOT
// clear the main-tree hover: input stays with the main tree, so the trigger
// is still genuinely hovered. Clearing it would make the trigger see a
// spurious re-enter on the next move and re-open the very tip it just opened
// (the FW-13 click-through contract). Guards the `if options.interactive`
// gate on the G15 fix.
#[test]
fn noninteractive_layer_push_preserves_trigger_hover() {
    let exited = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(800.0).height(600.0));
    let trigger = {
        let x = exited.clone();
        tree.add_child(
            main,
            Container::column()
                .width(50.0)
                .height(50.0)
                .hoverable()
                .on_hover_exit(move |_ctx| x.set(x.get() + 1))
                .on_press(|_pos, ctx| {
                    ctx.push_layer(
                        LayerOptions::tooltip(),
                        Container::column().width(120.0).height(40.0),
                        |_t, _root| {},
                    );
                }),
        )
    };

    measured_layout(&mut tree, 800.0, 600.0);

    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(25.0, 25.0),
        },
    );
    assert_eq!(tree.hovered(), Some(trigger), "trigger hovered after move");

    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(25.0, 25.0),
            button: MouseButton::Left,
        },
    );
    assert_eq!(tree.layer_count(), 1, "tooltip layer is up");

    // Non-interactive push leaves the hover chain intact.
    assert_eq!(
        tree.hovered(),
        Some(trigger),
        "click-through layer must not clear main-tree hover"
    );
    assert_eq!(exited.get(), 0, "trigger must not receive a spurious leave");
}

// ── pop re-resolves hover under a stationary cursor ───────────────────
//
// The other half of G15. Push emits the trigger's MouseLeave and leaves
// `hovered` null; nothing ever re-resolves it, because the hover chain is
// only recomputed from a live `MouseMove`. So dismissing a layer by clicking
// a button behind it left that button un-hovered — the dismiss swallows the
// press (by design), and no move follows, so the button lit up only once the
// user jiggled the mouse. `resync_hover` replays the hit-test at the last
// known cursor position once per frame after a pop.

/// Main root with a `trigger` (opens a centered popover) at (0,0)-(50,50)
/// and a second hoverable `other` button at (0,60)-(50,110), both well
/// clear of the centered layer's rect. Returns the probes as
/// (tree, trigger, other, other_entered, other_pressed).
#[allow(clippy::type_complexity)]
fn tree_with_trigger_and_other() -> (WidgetTree, usize, usize, Rc<Cell<u32>>, Rc<Cell<u32>>) {
    let entered = Rc::new(Cell::new(0u32));
    let pressed = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(800.0).height(600.0));
    let trigger = tree.add_child(
        main,
        Container::column()
            .width(50.0)
            .height(50.0)
            .hoverable()
            .on_press(|_pos, ctx| {
                ctx.push_layer(
                    LayerOptions::popover(),
                    Container::column().width(120.0).height(80.0),
                    |_t, _root| {},
                );
            }),
    );
    let other = {
        let e = entered.clone();
        let p = pressed.clone();
        tree.add_child(
            main,
            Container::column()
                .width(50.0)
                .height(50.0)
                .hoverable()
                .on_hover_enter(move |_rect, _ctx| e.set(e.get() + 1))
                .on_press(move |_pos, _ctx| p.set(p.get() + 1)),
        )
    };

    measured_layout(&mut tree, 800.0, 600.0);
    (tree, trigger, other, entered, pressed)
}

/// Emulate one frame of the event loop's post-layout resync.
fn frame_resync(tree: &mut WidgetTree) -> bool {
    let mut ctx = EventContext::new();
    let changed = tree.resync_hover(&mut ctx);
    measured_layout(tree, 800.0, 600.0);
    changed
}

#[test]
fn outside_click_dismiss_resyncs_hover_onto_the_button_under_the_cursor() {
    let (mut tree, trigger, other, entered, pressed) = tree_with_trigger_and_other();

    // Open the popover from the trigger; push clears hover (G15).
    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(25.0, 25.0),
        },
    );
    assert_eq!(tree.hovered(), Some(trigger));
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(25.0, 25.0),
            button: MouseButton::Left,
        },
    );
    assert_eq!(tree.layer_count(), 1, "popover is up");
    assert_eq!(tree.hovered(), None, "push cleared hover");

    // Travel to `other`. The layer owns the pointer, so a move outside its
    // rect only clears hover — `other` is not hovered while the layer is up.
    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(25.0, 85.0),
        },
    );
    assert_eq!(tree.hovered(), None, "layer owns the pointer");
    assert_eq!(entered.get(), 0, "no hover leaks through an open layer");

    // Click `other` to dismiss. The press is swallowed by the outside-click
    // path — that contract stays — and the pop alone does not re-resolve
    // hover, since no `MouseMove` follows.
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(25.0, 85.0),
            button: MouseButton::Left,
        },
    );
    assert_eq!(tree.layer_count(), 0, "outside click dismissed the layer");
    assert_eq!(
        pressed.get(),
        0,
        "dismiss click is swallowed, not delivered"
    );
    assert_eq!(
        tree.hovered(),
        None,
        "hover still stale right after the pop"
    );

    // The fix: the next frame's resync sees the cursor over `other`.
    assert!(frame_resync(&mut tree), "hover chain changed");
    assert_eq!(tree.hovered(), Some(other), "cursor is over `other`");
    assert_eq!(entered.get(), 1, "`other` lit up without a mouse move");
}

#[test]
fn escape_dismiss_resyncs_hover_too() {
    let (mut tree, _trigger, other, entered, _pressed) = tree_with_trigger_and_other();

    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(25.0, 25.0),
            button: MouseButton::Left,
        },
    );
    // Park the cursor over `other` while the layer is up, then dismiss with
    // the keyboard — the pointer never moves again.
    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(25.0, 85.0),
        },
    );
    dispatch(
        &mut tree,
        WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Escape),
        },
    );
    assert_eq!(tree.layer_count(), 0, "Escape dismissed the layer");

    assert!(frame_resync(&mut tree));
    assert_eq!(tree.hovered(), Some(other));
    assert_eq!(entered.get(), 1, "`other` lit up after a keyboard dismiss");
}

#[test]
fn pop_with_cursor_over_dead_space_hovers_nothing_interactive() {
    let (mut tree, _trigger, other, entered, _pressed) = tree_with_trigger_and_other();

    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(25.0, 25.0),
            button: MouseButton::Left,
        },
    );
    // Dismiss by clicking empty background, far from either button.
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(700.0, 500.0),
            button: MouseButton::Left,
        },
    );
    assert_eq!(tree.layer_count(), 0);

    frame_resync(&mut tree);
    assert_ne!(
        tree.hovered(),
        Some(other),
        "cursor is nowhere near `other`"
    );
    assert_eq!(entered.get(), 0, "no phantom hover on dismiss");
}

#[test]
fn resync_hover_is_inert_without_a_pop() {
    let (mut tree, trigger, _other, _entered, _pressed) = tree_with_trigger_and_other();

    // No pop yet: nothing to re-resolve, even with a live hover chain.
    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(25.0, 25.0),
        },
    );
    assert!(!frame_resync(&mut tree), "no pop → no work");
    assert_eq!(tree.hovered(), Some(trigger), "hover chain left alone");

    // And a pop that happens before the pointer has ever been seen (boot-time
    // dismiss, keyboard-only session) has no position to replay.
    let mut fresh = WidgetTree::new();
    fresh.set_root(Container::column().width(800.0).height(600.0));
    let layer = fresh.push_layer(
        LayerOptions::popover(),
        Container::column().width(120.0).height(80.0),
    );
    measured_layout(&mut fresh, 800.0, 600.0);
    assert!(fresh.pop_layer(layer));
    let mut ctx = EventContext::new();
    assert!(
        !fresh.resync_hover(&mut ctx),
        "cursor position unknown → no hover invented"
    );
    assert_eq!(fresh.hovered(), None);
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

#[test]
fn replace_screen_tears_down_open_layers() {
    // A layer belongs to the screen that opened it: a `replace_screen` fired
    // while a modal is up (e.g. an idle auto-lock, or a restore returning to
    // the lock screen from inside its own confirm dialog) must drop the layer
    // so it doesn't hover over the new screen.
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(400.0).height(300.0));
    tree.push_layer(
        LayerOptions::modal(),
        Container::column().width(60.0).height(40.0),
    );
    tree.push_layer(
        LayerOptions::modal(),
        Container::column().width(60.0).height(40.0),
    );
    assert_eq!(tree.layer_count(), 2, "two stacked modals are open");

    let mut ctx = EventContext::new();
    ctx.replace_screen(|t| {
        t.set_root(Container::row().width(400.0).height(300.0));
    });
    tree.apply_pending_commands(&mut ctx);

    assert_eq!(
        tree.layer_count(),
        0,
        "replace_screen must tear down every open layer"
    );
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
    assert!(
        (last.x - 300.0).abs() < 0.5,
        "layer x = {}, want ~300",
        last.x
    );
    assert!(
        (last.y - 250.0).abs() < 0.5,
        "layer y = {}, want ~250",
        last.y
    );
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
fn scrollview_clip_inside_centered_layer_is_offset_to_viewport() {
    // Regression for the Restore-modal bug: a ScrollView pushes a clip = its
    // layout rect, which inside a layer is layer-*local*. The tree paints the
    // layer subtree under a push_offset(layer_offset), so the ScrollView's
    // clipped children draw offset — but push_clip used to leave the clip at
    // local coords, scissoring the content away. The clip must be folded by
    // the same layer offset the content is drawn at.
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(800.0).height(600.0));
    // A 200x100 modal card, centered → offset ((800-200)/2, (600-100)/2)
    // = (300, 250). It holds a ScrollView with a background child so a rect
    // (carrying the active clip) is emitted.
    let card = tree.push_layer(
        LayerOptions::modal(),
        Container::column().width(200.0).height(100.0),
    );
    let sv = tree.add_child(card, ScrollView::new().width_full().height_full());
    tree.add_child(
        sv,
        Container::column()
            .width(50.0)
            .height(50.0)
            .background(Color::rgb(0.6, 0.2, 0.2)),
    );
    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // The child's rect carries the ScrollView's clip. Its clip must be shifted
    // to the layer's viewport position (~x>=300, ~y>=250), not left at 0.
    let child = ctx
        .rects
        .iter()
        .find(|r| (r.width - 50.0).abs() < 0.5 && (r.height - 50.0).abs() < 0.5)
        .expect("child rect present");
    let clip = child.clip_rect.expect("child inside ScrollView is clipped");
    assert!(
        clip.origin.x >= 299.5 && clip.origin.y >= 249.5,
        "ScrollView clip must be offset into viewport space, got x={} y={}",
        clip.origin.x,
        clip.origin.y
    );
    // And the child itself is painted inside its own clip (not scissored away).
    assert!(
        child.x >= clip.origin.x - 0.5 && child.x < clip.origin.x + clip.size.width,
        "child x={} should sit within clip x={}..{}",
        child.x,
        clip.origin.x,
        clip.origin.x + clip.size.width
    );
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

/// Push a layer with the given anchor on top of a fixed-size main tree.
/// Returns (tree, layer_root); the layer is a fixed 150x80 container so
/// placement math is easy to check.
fn tree_with_anchored_layer(anchor: LayerAnchor) -> (WidgetTree, usize) {
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(800.0).height(600.0));
    let layer = tree.push_layer(
        LayerOptions::popover().anchor(anchor),
        Container::column()
            .width(150.0)
            .height(80.0)
            .background(Color::rgb(0.3, 0.3, 0.4)),
    );
    (tree, layer)
}

/// Return the (x, y) of the rect painted for the 150x80 layer container.
fn layer_paint_xy(tree: &mut WidgetTree) -> (f32, f32) {
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    let r = ctx
        .rects
        .iter()
        .find(|r| (r.width - 150.0).abs() < 0.5 && (r.height - 80.0).abs() < 0.5)
        .expect("layer rect present");
    (r.x, r.y)
}

#[test]
fn anchor_rect_below_places_under_trigger() {
    // Trigger at (100, 200) sized 80x32 → popover top-left should be
    // (100, 232).
    let trigger = Rect::new(100.0, 200.0, 80.0, 32.0);
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::AnchorRect {
        rect: trigger,
        prefer: Placement::Below,
        align: HAlign::Start,
    });
    tree.compute_layout(800.0, 600.0);
    let (x, y) = layer_paint_xy(&mut tree);
    assert!((x - 100.0).abs() < 0.5, "x = {}, want 100", x);
    assert!((y - 232.0).abs() < 0.5, "y = {}, want 232", y);
}

#[test]
fn anchor_rect_above_places_over_trigger() {
    // Trigger at (50, 300) sized 80x32 → popover (80 tall) sits with its
    // bottom edge at y=300, so top-left = (50, 220).
    let trigger = Rect::new(50.0, 300.0, 80.0, 32.0);
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::AnchorRect {
        rect: trigger,
        prefer: Placement::Above,
        align: HAlign::Start,
    });
    tree.compute_layout(800.0, 600.0);
    let (x, y) = layer_paint_xy(&mut tree);
    assert!((x - 50.0).abs() < 0.5, "x = {}, want 50", x);
    assert!((y - 220.0).abs() < 0.5, "y = {}, want 220", y);
}

#[test]
fn anchor_rect_auto_flips_when_below_overflows() {
    // Trigger near the viewport bottom; below would put popover bottom
    // at 580+80 = 660 > 600. Auto must flip above: popover bottom = trigger
    // top (540), top-left y = 460.
    let trigger = Rect::new(100.0, 540.0, 80.0, 40.0);
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::AnchorRect {
        rect: trigger,
        prefer: Placement::Auto,
        align: HAlign::Start,
    });
    tree.compute_layout(800.0, 600.0);
    let (_x, y) = layer_paint_xy(&mut tree);
    assert!(
        (y - 460.0).abs() < 0.5,
        "y = {}, want 460 (flipped above)",
        y
    );
}

#[test]
fn anchor_rect_auto_stays_below_when_room_is_there() {
    let trigger = Rect::new(100.0, 50.0, 80.0, 32.0);
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::AnchorRect {
        rect: trigger,
        prefer: Placement::Auto,
        align: HAlign::Start,
    });
    tree.compute_layout(800.0, 600.0);
    let (_x, y) = layer_paint_xy(&mut tree);
    assert!((y - 82.0).abs() < 0.5, "y = {}, want 82 (below)", y);
}

#[test]
fn anchor_rect_x_clamps_to_viewport_right() {
    // Trigger far right; popover would land at x=750 + width 150 = 900
    // > 800. Must clamp to x = 650 so right edge sits at 800.
    let trigger = Rect::new(750.0, 100.0, 40.0, 32.0);
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::AnchorRect {
        rect: trigger,
        prefer: Placement::Below,
        align: HAlign::Start,
    });
    tree.compute_layout(800.0, 600.0);
    let (x, _y) = layer_paint_xy(&mut tree);
    assert!((x - 650.0).abs() < 0.5, "x = {}, want 650 (clamped)", x);
}

#[test]
fn anchor_rect_x_clamps_to_viewport_left() {
    // Negative trigger.x (e.g., scrolled trigger off-screen) clamps to 0.
    let trigger = Rect::new(-30.0, 100.0, 40.0, 32.0);
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::AnchorRect {
        rect: trigger,
        prefer: Placement::Below,
        align: HAlign::Start,
    });
    tree.compute_layout(800.0, 600.0);
    let (x, _y) = layer_paint_xy(&mut tree);
    assert!((x - 0.0).abs() < 0.5, "x = {}, want 0 (clamped)", x);
}

#[test]
fn anchor_rect_end_aligns_right_edges() {
    // align=End: the popover's right edge meets the trigger's right edge
    // (CSS `right-0`). Trigger right = 100+80 = 180; popover width 150 →
    // x = 180 - 150 = 30. Vertical still drops below (y = 232).
    let trigger = Rect::new(100.0, 200.0, 80.0, 32.0);
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::AnchorRect {
        rect: trigger,
        prefer: Placement::Below,
        align: HAlign::End,
    });
    tree.compute_layout(800.0, 600.0);
    let (x, y) = layer_paint_xy(&mut tree);
    assert!((x - 30.0).abs() < 0.5, "x = {}, want 30 (right-aligned)", x);
    assert!((y - 232.0).abs() < 0.5, "y = {}, want 232", y);
}

#[test]
fn anchor_rect_center_centers_over_trigger() {
    // align=Center: popover centered over the trigger. Trigger center x =
    // 140; popover width 150 → x = 140 - 75 = 65.
    let trigger = Rect::new(100.0, 200.0, 80.0, 32.0);
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::AnchorRect {
        rect: trigger,
        prefer: Placement::Below,
        align: HAlign::Center,
    });
    tree.compute_layout(800.0, 600.0);
    let (x, _y) = layer_paint_xy(&mut tree);
    assert!((x - 65.0).abs() < 0.5, "x = {}, want 65 (centered)", x);
}

#[test]
fn viewport_anchor_top_center_with_offset() {
    // A top-center banner (`top-2 left-1/2 -translate-x-1/2`). Viewport
    // 800x600, layer 150x80 → x = (800-150)/2 = 325, y = 0 + 8 offset.
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::Viewport {
        h: HAlign::Center,
        v: VAlign::Start,
        offset: (0.0, 8.0),
    });
    tree.compute_layout(800.0, 600.0);
    let (x, y) = layer_paint_xy(&mut tree);
    assert!((x - 325.0).abs() < 0.5, "x = {}, want 325 (h-center)", x);
    assert!((y - 8.0).abs() < 0.5, "y = {}, want 8 (top + offset)", y);
}

#[test]
fn viewport_anchor_bottom_right_with_negative_offset() {
    // Bottom-right corner inset by 16px on each axis. x = 800-150-16 = 634,
    // y = 600-80-16 = 504.
    let (mut tree, _layer) = tree_with_anchored_layer(LayerAnchor::Viewport {
        h: HAlign::End,
        v: VAlign::End,
        offset: (-16.0, -16.0),
    });
    tree.compute_layout(800.0, 600.0);
    let (x, y) = layer_paint_xy(&mut tree);
    assert!(
        (x - 634.0).abs() < 0.5,
        "x = {}, want 634 (right - inset)",
        x
    );
    assert!(
        (y - 504.0).abs() < 0.5,
        "y = {}, want 504 (bottom - inset)",
        y
    );
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
        Button::new("main")
            .background(Color::WHITE)
            .on_click(move |_| {
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
        Button::new("layer")
            .background(Color::WHITE)
            .on_click(move |_| {
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
    assert_eq!(
        main_clicks.get(),
        0,
        "main button must not see layer-scope click"
    );
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
    tree.push_layer(opts, Container::column().width(50.0).height(40.0));

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
    assert_eq!(
        main_clicks.get(),
        0,
        "main tree must not see swallowed click"
    );
}

// ── menu-switch: one-click switch between peer menu triggers ──────────
//
// Two toolbar triggers (gear + ⋮) each open their own popover. With one menu
// open, a single click on the *other* trigger should close the open menu AND
// open the second: the trigger opts into `Button::menu_switch(true)`, so the
// dismissing pointer-down is re-routed to it instead of being swallowed. A
// plain button (the header lock) does not opt in and stays two-click, proving
// this is a scoped re-route to opted-in triggers, not a general pass-through.

/// Center point of a laid-out rect, for clicking a widget by index.
fn center(r: Rect) -> Point {
    Point::new(
        r.origin.x + r.size.width / 2.0,
        r.origin.y + r.size.height / 2.0,
    )
}

/// One full primary click (press + release) at `p`.
fn click_at(tree: &mut WidgetTree, p: Point) {
    dispatch(
        tree,
        WidgetEvent::MouseDown {
            position: p,
            button: MouseButton::Left,
        },
    );
    dispatch(
        tree,
        WidgetEvent::MouseUp {
            position: p,
            button: MouseButton::Left,
        },
    );
}

/// A `menu_switch` trigger that opens a centered popover on click and bumps
/// `opens` each time. Centered so top-left triggers sit outside its rect.
fn switch_trigger(label: &str, opens: Rc<Cell<u32>>) -> Button {
    Button::new(label.to_string())
        .background(Color::WHITE)
        .menu_switch(true)
        .on_click(move |ctx| {
            opens.set(opens.get() + 1);
            ctx.push_layer(
                LayerOptions::popover(),
                Container::column().width(120.0).height(80.0),
                |_t, _root| {},
            );
        })
}

#[test]
fn menu_switch_switches_between_peer_triggers_in_one_click() {
    let a_opens = Rc::new(Cell::new(0u32));
    let b_opens = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(800.0).height(600.0));
    let a = tree.add_child(main, switch_trigger("A", a_opens.clone()));
    let b = tree.add_child(main, switch_trigger("B", b_opens.clone()));
    measured_layout(&mut tree, 800.0, 600.0);

    let a_pt = center(tree.layout_rect(a));
    let b_pt = center(tree.layout_rect(b));

    // Open A's menu.
    click_at(&mut tree, a_pt);
    measured_layout(&mut tree, 800.0, 600.0);
    assert_eq!(tree.layer_count(), 1, "A's menu is open");
    assert_eq!(a_opens.get(), 1);

    // One click on B: closes A's menu and opens B's in the same click.
    click_at(&mut tree, b_pt);
    measured_layout(&mut tree, 800.0, 600.0);
    assert_eq!(
        tree.layer_count(),
        1,
        "still exactly one menu open after the switch"
    );
    assert_eq!(
        b_opens.get(),
        1,
        "B's menu opened on the single switch click"
    );
    assert_eq!(a_opens.get(), 1, "A did not reopen");
}

#[test]
fn menu_switch_opens_new_menu_on_the_press_no_empty_gap() {
    let a_opens = Rc::new(Cell::new(0u32));
    let b_opens = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(800.0).height(600.0));
    let a = tree.add_child(main, switch_trigger("A", a_opens.clone()));
    let b = tree.add_child(main, switch_trigger("B", b_opens.clone()));
    measured_layout(&mut tree, 800.0, 600.0);
    let a_pt = center(tree.layout_rect(a));
    let b_pt = center(tree.layout_rect(b));

    click_at(&mut tree, a_pt);
    measured_layout(&mut tree, 800.0, 600.0);
    assert_eq!(tree.layer_count(), 1, "A's menu is open");

    // Press (MouseDown only, no release yet) on B: the switch must complete on
    // the press — A closed and B opened within the one event — so a menu is
    // always up. If the open waited for the release, layer_count would be 0
    // here: the empty-frame flicker the user reported.
    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: b_pt,
            button: MouseButton::Left,
        },
    );
    assert_eq!(
        tree.layer_count(),
        1,
        "a menu is up on the press alone — no empty gap between the two"
    );
    assert_eq!(b_opens.get(), 1, "B's menu opened on the press");
    assert_eq!(a_opens.get(), 1, "A did not reopen");

    // The trailing real release is harmless — swallowed by B's open menu.
    dispatch(
        &mut tree,
        WidgetEvent::MouseUp {
            position: b_pt,
            button: MouseButton::Left,
        },
    );
    measured_layout(&mut tree, 800.0, 600.0);
    assert_eq!(
        tree.layer_count(),
        1,
        "still exactly one menu after release"
    );
    assert_eq!(b_opens.get(), 1, "release did not re-open or double-fire");
}

#[test]
fn menu_switch_sibling_trigger_hovers_while_a_peer_menu_is_open() {
    let a_opens = Rc::new(Cell::new(0u32));
    let b_opens = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(800.0).height(600.0));
    let a = tree.add_child(main, switch_trigger("A", a_opens.clone()));
    let b = tree.add_child(main, switch_trigger("B", b_opens.clone()));
    measured_layout(&mut tree, 800.0, 600.0);
    let a_pt = center(tree.layout_rect(a));
    let b_pt = center(tree.layout_rect(b));

    // Open A's menu (the push clears any hover).
    click_at(&mut tree, a_pt);
    measured_layout(&mut tree, 800.0, 600.0);
    assert_eq!(tree.layer_count(), 1, "A's menu is open");
    assert_eq!(tree.hovered(), None, "push cleared hover");

    // Move over B (a sibling switch trigger) while A's menu is open: B lights
    // up instead of the move being swallowed — the "pressable" affordance.
    dispatch(&mut tree, WidgetEvent::MouseMove { position: b_pt });
    assert_eq!(
        tree.hovered(),
        Some(b),
        "sibling switch trigger hovers under an open peer menu"
    );

    // Move into A's centered menu (~viewport center): B is left again — hover
    // enter/leave spans the layer boundary.
    dispatch(
        &mut tree,
        WidgetEvent::MouseMove {
            position: Point::new(400.0, 300.0),
        },
    );
    assert_ne!(
        tree.hovered(),
        Some(b),
        "moving into the menu leaves the sibling trigger"
    );
}

#[test]
fn menu_switch_click_on_owning_trigger_toggles_closed() {
    let a_opens = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(800.0).height(600.0));
    let a = tree.add_child(main, switch_trigger("A", a_opens.clone()));
    measured_layout(&mut tree, 800.0, 600.0);
    let a_pt = center(tree.layout_rect(a));

    // Open A's menu.
    click_at(&mut tree, a_pt);
    measured_layout(&mut tree, 800.0, 600.0);
    assert_eq!(tree.layer_count(), 1, "A's menu is open");
    assert_eq!(a_opens.get(), 1);

    // Click A again: it owns the open menu, so this must toggle it closed and
    // NOT reopen it (the switch re-route is suppressed for the owning trigger).
    click_at(&mut tree, a_pt);
    measured_layout(&mut tree, 800.0, 600.0);
    assert_eq!(
        tree.layer_count(),
        0,
        "clicking the owning trigger toggles its menu closed"
    );
    assert_eq!(
        a_opens.get(),
        1,
        "the owning trigger did not reopen its menu"
    );
}

#[test]
fn plain_button_under_dismiss_click_stays_two_click() {
    let a_opens = Rc::new(Cell::new(0u32));
    let lock_clicks = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(800.0).height(600.0));
    let a = tree.add_child(main, switch_trigger("A", a_opens.clone()));
    let lc = lock_clicks.clone();
    // A consequential control (the lock) — NOT a menu-switch trigger.
    let lock = tree.add_child(
        main,
        Button::new("lock")
            .background(Color::WHITE)
            .on_click(move |_| lc.set(lc.get() + 1)),
    );
    measured_layout(&mut tree, 800.0, 600.0);

    let a_pt = center(tree.layout_rect(a));
    let lock_pt = center(tree.layout_rect(lock));

    click_at(&mut tree, a_pt);
    measured_layout(&mut tree, 800.0, 600.0);
    assert_eq!(tree.layer_count(), 1, "A's menu is open");

    // First click on the lock only dismisses A's menu; its press is swallowed.
    click_at(&mut tree, lock_pt);
    measured_layout(&mut tree, 800.0, 600.0);
    assert_eq!(tree.layer_count(), 0, "menu dismissed by the outside click");
    assert_eq!(
        lock_clicks.get(),
        0,
        "lock is not a switch target — its press is swallowed"
    );

    // A second click is required to actually activate the lock.
    click_at(&mut tree, lock_pt);
    assert_eq!(
        lock_clicks.get(),
        1,
        "lock activates only on the second click"
    );
}

// ── stacked-layer dismissal (location-aware collapse) ─────────────
//
// An outside-click carries a position, and that position encodes how deep
// to dismiss: clicking dead space outside the whole stack collapses all of
// it at once, while clicking an *intermediate* layer peels only the layers
// above it (the dropdown-inside-a-popover case — one click on the popover
// body closes the dropdown but keeps the popover). Escape stays a one-level
// peel; only the pointer path is location-aware.

/// Two stacked popovers at known viewport rects: `lower` fills a 300×300 box
/// pinned at (100,100); `upper` is an 80×50 box pinned at (120,120), nested
/// inside the lower's rect. `lower_dismissable` toggles the lower layer's
/// outside-click dismiss; the upper always dismisses.
fn stacked_popovers(lower_dismissable: bool) -> (WidgetTree, usize, usize) {
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(800.0).height(600.0));
    let lower = tree.push_layer(
        LayerOptions::popover()
            .dismiss_on_outside_click(lower_dismissable)
            .anchor(LayerAnchor::Viewport {
                h: HAlign::Start,
                v: VAlign::Start,
                offset: (100.0, 100.0),
            }),
        Container::column()
            .width(300.0)
            .height(300.0)
            .background(Color::rgb(0.2, 0.2, 0.3)),
    );
    let upper = tree.push_layer(
        LayerOptions::popover().anchor(LayerAnchor::Viewport {
            h: HAlign::Start,
            v: VAlign::Start,
            offset: (120.0, 120.0),
        }),
        Container::column()
            .width(80.0)
            .height(50.0)
            .background(Color::rgb(0.3, 0.3, 0.4)),
    );
    tree.compute_layout(800.0, 600.0);
    (tree, lower, upper)
}

#[test]
fn outside_click_collapses_whole_layer_stack() {
    // A click in dead space outside *both* popovers dismisses the entire
    // stack in one go — not one layer per click.
    let (mut tree, lower, upper) = stacked_popovers(true);
    assert_eq!(tree.layer_count(), 2);

    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            // outside lower (x<400) and outside upper.
            position: Point::new(600.0, 500.0),
            button: MouseButton::Left,
        },
    );

    assert_eq!(
        tree.layer_count(),
        0,
        "click outside the whole stack closes every layer"
    );
    assert!(!tree.contains(lower));
    assert!(!tree.contains(upper));
}

#[test]
fn click_in_lower_layer_peels_only_upper() {
    // A click inside the lower popover but outside the upper one peels just
    // the upper layer — the dropdown-in-popover case: clicking the popover
    // body closes only the dropdown, the popover stays open.
    let (mut tree, lower, upper) = stacked_popovers(true);

    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            // inside lower (100..400) but outside upper (120..200, 120..170).
            position: Point::new(300.0, 300.0),
            button: MouseButton::Left,
        },
    );

    assert_eq!(tree.layer_count(), 1, "only the upper layer peels");
    assert!(tree.contains(lower), "lower popover stays open");
    assert!(!tree.contains(upper), "upper popover dismissed");
    assert_eq!(tree.top_layer_root(), Some(lower));
}

#[test]
fn nondismissable_layer_blocks_collapse_below_it() {
    // The lower popover opts out of outside-click dismiss. A click outside
    // both still peels the dismissable upper layer, but the collapse stops
    // at the lower one — it eats the outside click and stays (modal-style
    // shielding), never reaching past it.
    let (mut tree, lower, upper) = stacked_popovers(false);

    dispatch(
        &mut tree,
        WidgetEvent::MouseDown {
            position: Point::new(600.0, 500.0),
            button: MouseButton::Left,
        },
    );

    assert_eq!(tree.layer_count(), 1, "upper peels, lower is shielded");
    assert!(tree.contains(lower), "non-dismissable lower must persist");
    assert!(!tree.contains(upper));
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
    assert_eq!(
        tree.layer_count(),
        1,
        "Escape must not dismiss when disabled"
    );
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
        Button::new("open")
            .background(Color::WHITE)
            .on_click(|ctx| {
                ctx.push_layer(
                    LayerOptions::modal(),
                    Container::column().width(120.0).height(60.0),
                    |_tree, _root| {},
                );
            }),
    );
    // Measured layout: the button derives its height from `measure` (it no
    // longer carries a `min_height` in its style), so it must be measured to
    // be tall enough to receive the click — same as the real event loop.
    measured_layout(&mut tree, 400.0, 300.0);

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
        Button::new("close")
            .background(Color::WHITE)
            .on_click(|ctx| {
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
    let snap = starts[0];
    // Main contributed at least the root rect and the label's glyphs.
    assert!(snap.rect >= 1, "main batch rect count = {}", snap.rect);
    assert!(snap.glyph >= 1, "main batch glyph count = {}", snap.glyph);
    // The layer batch follows the breakpoint and contributes its own
    // commands (scrim + layer bg + layer text glyphs).
    assert!(ctx.rects.len() > snap.rect);
    assert!(ctx.glyphs.len() > snap.glyph);
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
    // Give the layer container an explicit size — the text-paint guard
    // (Phase 32) bails on layout width = 0, and compute_layout (used here
    // instead of compute_layout_with_measure) doesn't run the text widget's
    // measure callback to derive a natural intrinsic width.
    let layer = tree.push_layer(
        LayerOptions::modal(),
        Container::column()
            .width(200.0)
            .height(100.0)
            .padding(10.0)
            .background(Color::WHITE),
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

// ── shortcut routing × layer interaction (Phase 27 / A-11) ────────

/// Helper: dispatch a synthetic `KeyDown { Character(ch) }` with the
/// given modifier set, mirroring what `translate_character` produces
/// in the event loop.
fn dispatch_key_with_mods(tree: &mut WidgetTree, ch: char, mods: Modifiers) {
    let mut ctx = EventContext::new();
    ctx.modifiers = mods;
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Character(ch),
        },
        &mut ctx,
    );
}

#[test]
fn modal_with_default_options_does_not_block_global_shortcut() {
    // Ctrl+L registered as Global must still fire while a default modal
    // is on top — the modal opts out of `block_shortcuts` by default so
    // lock/panic shortcuts keep working.
    let (mut tree, _main, _layer) = tree_with_layer(LayerOptions::modal());
    let fired = Rc::new(Cell::new(false));
    let f = fired.clone();
    tree.shortcut_router_mut().register(
        Shortcut::global(Modifiers::CTRL, Key::Character('l')),
        move |_| f.set(true),
    );

    dispatch_key_with_mods(&mut tree, 'l', Modifiers::CTRL);

    assert!(
        fired.get(),
        "Global shortcut must fire through default modal"
    );
}

#[test]
fn modal_with_block_shortcuts_true_suppresses_even_global() {
    // A "trapping" modal (`.block_shortcuts(true)`) must swallow every
    // shortcut — including Global — so a confirm dialog can't be
    // hijacked by a stray Ctrl+L.
    let (mut tree, _main, _layer) = tree_with_layer(LayerOptions::modal().block_shortcuts(true));
    let fired = Rc::new(Cell::new(false));
    let f = fired.clone();
    tree.shortcut_router_mut().register(
        Shortcut::global(Modifiers::CTRL, Key::Character('l')),
        move |_| f.set(true),
    );

    dispatch_key_with_mods(&mut tree, 'l', Modifiers::CTRL);

    assert!(
        !fired.get(),
        "Global shortcut must be suppressed by block_shortcuts modal"
    );
}

#[test]
fn default_scope_shortcut_suppressed_with_focused_input_in_modal() {
    // Modal that contains a focused Input must NOT fire a default-scope
    // (WhenNoTextInput) shortcut whose key collides with text input — the
    // Knot case where Ctrl+N inside a backup-name field shouldn't open a
    // new note. Layer itself uses default options (no block_shortcuts),
    // so the suppression comes from the focused widget's accepts_text.
    let mut tree = WidgetTree::new();
    let main = tree.set_root(Container::column().width(800.0).height(600.0));
    tree.add_child(main, Container::column().width(50.0).height(50.0));
    let layer = tree.push_layer(
        LayerOptions::modal(),
        Container::column().width(200.0).height(100.0),
    );
    let input_idx = tree.add_child(layer, Input::new().value(Signal::new(String::new())));
    tree.focus_initially(input_idx);
    measured_layout(&mut tree, 800.0, 600.0);
    let mut focus_ctx = EventContext::new();
    tree.flush_pending_focus(&mut focus_ctx);

    let fired = Rc::new(Cell::new(false));
    let f = fired.clone();
    tree.shortcut_router_mut().register(
        Shortcut::new(Modifiers::CTRL, Key::Character('n')),
        move |_| f.set(true),
    );

    dispatch_key_with_mods(&mut tree, 'n', Modifiers::CTRL);

    assert!(
        !fired.get(),
        "default-scope shortcut must be suppressed while a text input has focus"
    );
}
