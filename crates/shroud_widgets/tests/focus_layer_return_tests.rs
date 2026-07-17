//! Dismissing a layer hands focus back to the widget that opened it.
//!
//! A dialog's fields go down with its subtree and `remove` drops focus with
//! them, leaving focus nowhere: the next Tab restarts at the top of the window
//! and the user loses their place. The tree already knows where they logically
//! are — the layer stamps its opener at push — so the pop returns focus there.
//!
//! These drive the real path (a Button handler pushing a layer, so the opener
//! is stamped the way it is in an app) rather than `WidgetTree::push_layer`,
//! which is the boot/test entrypoint and deliberately records no opener.

use shroud_core::{Point, Theme};
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::layer::LayerOptions;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Button, Container, MenuItem};

fn layout(tree: &mut WidgetTree) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 400.0, &mut engine, &theme);
}

fn tab(tree: &mut WidgetTree, ctx: &mut EventContext) {
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Tab),
        },
        ctx,
    );
}

fn escape(tree: &mut WidgetTree, ctx: &mut EventContext) {
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Escape),
        },
        ctx,
    );
}

/// Click a widget, by its laid-out rect.
fn click(tree: &mut WidgetTree, ctx: &mut EventContext, idx: usize) {
    let r = tree.layout_rect(idx);
    let p = Point::new(r.origin.x + 2.0, r.origin.y + 2.0);
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: p,
            button: MouseButton::Left,
        },
        ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: p,
            button: MouseButton::Left,
        },
        ctx,
    );
}

/// The event loop applies a queued focus at the top of each redraw; nothing
/// in a dispatch does it, so tests have to turn the frame over too.
fn frame(tree: &mut WidgetTree, ctx: &mut EventContext) {
    tree.flush_pending_focus(ctx);
    layout(tree);
}

/// A window with a plain Button, then a Button that opens a modal holding one
/// focusable field. Returns (tree, first button, trigger).
fn modal_app() -> (WidgetTree, usize, usize) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(400.0));
    let first = tree.add_child(root, Button::new("first"));
    let trigger = tree.add_child(
        root,
        Button::new("open").on_click(|ctx| {
            ctx.push_layer(
                LayerOptions::modal(),
                Container::column().width(200.0).height(100.0),
                |tree, layer| {
                    tree.add_child(layer, Button::new("field"));
                },
            );
        }),
    );
    layout(&mut tree);
    (tree, first, trigger)
}

#[test]
fn escaping_a_dialog_returns_focus_to_its_trigger() {
    let (mut tree, _first, trigger) = modal_app();
    let mut ctx = EventContext::new();

    click(&mut tree, &mut ctx, trigger);
    frame(&mut tree, &mut ctx);
    assert_eq!(tree.layer_count(), 1, "sanity: the click opened the modal");

    // Into the dialog's field, then dismiss from the keyboard.
    tab(&mut tree, &mut ctx);
    let field = tree.focused().expect("Tab should enter the dialog");
    assert_ne!(field, trigger, "focus should be inside the layer");

    escape(&mut tree, &mut ctx);
    frame(&mut tree, &mut ctx);

    assert_eq!(tree.layer_count(), 0, "sanity: Escape dismissed the modal");
    assert_eq!(
        tree.focused(),
        Some(trigger),
        "focus should come back to the button that opened the dialog"
    );
}

#[test]
fn tab_after_a_dismiss_continues_from_the_trigger() {
    // The point of restoring: Tab picks up where the user was, instead of
    // restarting at the top of the window.
    let (mut tree, first, trigger) = modal_app();
    let mut ctx = EventContext::new();

    click(&mut tree, &mut ctx, trigger);
    frame(&mut tree, &mut ctx);
    tab(&mut tree, &mut ctx);
    escape(&mut tree, &mut ctx);
    frame(&mut tree, &mut ctx);

    // Forward from the trigger wraps to the first button; the giveaway for a
    // lost place is landing on `first` from a Shift+Tab that should have gone
    // the other way. Check the unambiguous direction: back off the trigger.
    ctx.modifiers.shift = true;
    tab(&mut tree, &mut ctx);
    assert_eq!(
        tree.focused(),
        Some(first),
        "Shift+Tab should step back from the trigger, not from the window top"
    );
}

#[test]
fn a_dialog_tabbed_into_returns_a_visible_ring_to_the_trigger() {
    // The user is navigating by keyboard when they Escape, so the trigger they
    // land on has to show where focus went.
    let (mut tree, _first, trigger) = modal_app();
    let mut ctx = EventContext::new();

    click(&mut tree, &mut ctx, trigger);
    frame(&mut tree, &mut ctx);
    tab(&mut tree, &mut ctx); // keyboard focus inside the dialog → ring on
    escape(&mut tree, &mut ctx);
    frame(&mut tree, &mut ctx);

    assert_eq!(tree.focused(), Some(trigger));
    assert!(
        tree.focus_visible(),
        "a keyboard user must see the ring on the trigger they land back on"
    );
}

#[test]
fn a_dialog_clicked_through_returns_a_ringless_trigger() {
    // ...and the mirror image: someone who clicked their way through the dialog
    // is still not navigating by keyboard, so no ring appears out of nowhere.
    let (mut tree, _first, trigger) = modal_app();
    let mut ctx = EventContext::new();

    click(&mut tree, &mut ctx, trigger);
    frame(&mut tree, &mut ctx);

    // Click the dialog's field (pointer focus → no ring).
    let field = tree.focusable_in_tab_order()[0];
    let r = tree.layout_rect(field);
    let off = tree.top_layer_offset().expect("modal is up");
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(r.origin.x + off.0 + 2.0, r.origin.y + off.1 + 2.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
    assert_eq!(tree.focused(), Some(field), "sanity: the click focused it");
    assert!(
        !tree.focus_visible(),
        "sanity: pointer focus paints no ring"
    );

    escape(&mut tree, &mut ctx);
    frame(&mut tree, &mut ctx);

    assert_eq!(tree.focused(), Some(trigger));
    assert!(
        !tree.focus_visible(),
        "a pointer user should not be handed a ring by the dismiss"
    );
}

#[test]
fn dismissing_a_menu_leaves_outside_focus_alone() {
    // A menu's rows are tab stops, but the user need never step onto one: open
    // a menu and dismiss it and focus is still out where it was. The pop did
    // not clear it, so the restore must not fire and drag the user to the
    // trigger.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(400.0));
    let elsewhere = tree.add_child(root, Button::new("elsewhere"));
    let trigger = tree.add_child(
        root,
        Button::new("menu").on_click(|ctx| {
            ctx.push_layer(
                LayerOptions::popover(),
                Container::column().width(120.0),
                |tree, layer| {
                    tree.add_child(layer, MenuItem::new("row", |_| {}));
                },
            );
        }),
    );
    layout(&mut tree);
    let mut ctx = EventContext::new();

    click(&mut tree, &mut ctx, trigger);
    frame(&mut tree, &mut ctx);
    assert_eq!(tree.layer_count(), 1, "sanity: the menu opened");

    // Clicking the trigger focused it, so park focus somewhere unrelated to
    // model what this guard is really about: focus that is *outside* the layer
    // when it closes. It survives the pop, and a restore that fired anyway
    // would drag the user off to the trigger.
    tree.focus(Some(elsewhere), &mut ctx);

    escape(&mut tree, &mut ctx);
    frame(&mut tree, &mut ctx);

    assert_eq!(
        tree.focused(),
        Some(elsewhere),
        "a menu that never held focus must not move it to its trigger"
    );
}

#[test]
fn a_trigger_that_did_not_outlive_its_layer_leaves_focus_cleared() {
    // The opener can be gone by the time its layer closes — a row's menu whose
    // action rebuilt that very row. Restoring must notice rather than focus a
    // tombstoned slot.
    let (mut tree, _first, trigger) = modal_app();
    let mut ctx = EventContext::new();

    click(&mut tree, &mut ctx, trigger);
    frame(&mut tree, &mut ctx);
    tab(&mut tree, &mut ctx);

    tree.remove(trigger);
    escape(&mut tree, &mut ctx);
    frame(&mut tree, &mut ctx);

    assert_eq!(tree.layer_count(), 0);
    assert_eq!(
        tree.focused(),
        None,
        "no trigger to return to → focus stays cleared"
    );
}

#[test]
fn a_pointer_only_trigger_is_not_given_keyboard_focus() {
    // `Container::on_press` is a pure-pointer trigger by design (the a11y-
    // complete one is `Button::on_click_rect`). It is not a tab stop, so
    // parking focus on it would only *look* restored — the next Tab would
    // restart at the top of the window anyway.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(400.0));
    let trigger = tree.add_child(
        root,
        Container::column()
            .width(50.0)
            .height(50.0)
            .on_press(|_pos, ctx| {
                ctx.push_layer(
                    LayerOptions::modal(),
                    Container::column().width(200.0).height(100.0),
                    |tree, layer| {
                        tree.add_child(layer, Button::new("field"));
                    },
                );
            }),
    );
    layout(&mut tree);
    let mut ctx = EventContext::new();

    click(&mut tree, &mut ctx, trigger);
    frame(&mut tree, &mut ctx);
    tab(&mut tree, &mut ctx);
    assert!(tree.focused().is_some(), "sanity: Tab entered the dialog");

    escape(&mut tree, &mut ctx);
    frame(&mut tree, &mut ctx);

    assert_eq!(
        tree.focused(),
        None,
        "a non-focusable trigger is not a place keyboard focus can live"
    );
}
