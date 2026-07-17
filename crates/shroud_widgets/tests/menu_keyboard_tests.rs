//! An open menu is reachable from the keyboard.
//!
//! Keyboard events go to the topmost interactive layer's subtree, so while a
//! menu is up the trigger behind it cannot receive them. That is correct — but
//! it used to mean a menu was a keyboard dead end: `MenuItem` was not a tab
//! stop, so the layer's tab order was empty, Tab and Enter both landed nowhere,
//! and the focus ring sat on the trigger behind claiming otherwise.
//!
//! Rows being tab stops is what closes that: the menu's rows *are* the tab
//! order of its layer, so Tab / ↓ step in, Enter fires, and Escape hands focus
//! back to the trigger through the tree's existing return path (which only arms
//! when the layer is what held focus).
//!
//! These drive the real path — a handler pushing a layer, so the opener is
//! stamped the way it is in an app — rather than `WidgetTree::push_layer`,
//! which is the boot/test entrypoint and records no opener.

use std::cell::RefCell;
use std::rc::Rc;

use shroud_core::{Point, Theme};
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::layer::LayerOptions;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Button, Container, MenuItem};

const ROWS: [&str; 3] = ["cut", "copy", "paste"];

fn layout(tree: &mut WidgetTree) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 400.0, &mut engine, &theme);
}

fn press(tree: &mut WidgetTree, ctx: &mut EventContext, k: NamedKey) {
    tree.dispatch_event(&WidgetEvent::KeyDown { key: Key::Named(k) }, ctx);
}

fn tab(tree: &mut WidgetTree, ctx: &mut EventContext) {
    press(tree, ctx, NamedKey::Tab);
}

/// Click a widget by its laid-out rect. Main-tree only — the trigger is all
/// these tests ever click.
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

/// The event loop applies a queued focus at the top of each redraw; nothing in
/// a dispatch does it, so tests have to turn the frame over too.
fn frame(tree: &mut WidgetTree, ctx: &mut EventContext) {
    tree.flush_pending_focus(ctx);
    layout(tree);
}

/// Labels of the rows whose handler ran, in order.
type Fired = Rc<RefCell<Vec<String>>>;

fn populate(fired: &Fired) -> impl FnOnce(&mut WidgetTree, usize) + 'static {
    let fired = Rc::clone(fired);
    move |tree: &mut WidgetTree, layer: usize| {
        for label in ROWS {
            let fired = Rc::clone(&fired);
            tree.add_child(
                layer,
                MenuItem::new(label, move |_| fired.borrow_mut().push(label.to_string())),
            );
        }
    }
}

/// A window with a plain Button, then a Button whose click opens a three-row
/// menu. Returns (tree, first button, trigger, fired-log).
fn menu_app() -> (WidgetTree, usize, usize, Fired) {
    let fired: Fired = Rc::new(RefCell::new(Vec::new()));
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(400.0));
    let first = tree.add_child(root, Button::new("first"));
    let captured = Rc::clone(&fired);
    let trigger = tree.add_child(
        root,
        Button::new("menu").on_click(move |ctx| {
            ctx.push_layer(
                LayerOptions::popover(),
                Container::column().width(120.0),
                populate(&captured),
            );
        }),
    );
    layout(&mut tree);
    (tree, first, trigger, fired)
}

/// Open the menu and return its rows, in tab order.
fn open(tree: &mut WidgetTree, ctx: &mut EventContext, trigger: usize) -> Vec<usize> {
    click(tree, ctx, trigger);
    frame(tree, ctx);
    assert_eq!(tree.layer_count(), 1, "sanity: the click opened the menu");
    let rows = tree.focusable_in_tab_order();
    assert_eq!(
        rows.len(),
        ROWS.len(),
        "a menu's rows are the tab order of its layer"
    );
    rows
}

#[test]
fn tab_steps_into_an_open_menu() {
    // The headline. Focus is out on the trigger and keyboard events go to the
    // layer, so unless a row can take focus the keystroke has nowhere to land.
    let (mut tree, _first, trigger, _fired) = menu_app();
    let mut ctx = EventContext::new();

    let rows = open(&mut tree, &mut ctx, trigger);
    assert_eq!(
        tree.focused(),
        Some(trigger),
        "sanity: clicking the trigger focused it, and opening left focus there"
    );

    tab(&mut tree, &mut ctx);

    assert_eq!(
        tree.focused(),
        Some(rows[0]),
        "Tab must step into the menu, not die on the trigger behind it"
    );
    assert!(
        tree.focus_visible(),
        "a row reached by keyboard has to show which row Enter would fire"
    );
}

#[test]
fn tab_walks_every_row_and_stays_trapped_in_the_menu() {
    let (mut tree, first, trigger, _fired) = menu_app();
    let mut ctx = EventContext::new();
    let rows = open(&mut tree, &mut ctx, trigger);

    for &row in &rows {
        tab(&mut tree, &mut ctx);
        assert_eq!(tree.focused(), Some(row));
    }
    // Off the last row: the layer traps Tab, so it wraps to the first row
    // rather than escaping to the window behind.
    tab(&mut tree, &mut ctx);
    assert_eq!(tree.focused(), Some(rows[0]), "Tab wraps inside the layer");
    assert_ne!(
        tree.focused(),
        Some(first),
        "and never leaks out to the tree behind"
    );
}

#[test]
fn arrow_keys_walk_the_same_ring_as_tab() {
    // ↓ / ↑ are a menu's native way between rows, and they mean exactly what
    // Tab / Shift+Tab mean here — the rows are the order, so both keys walk it.
    let (mut tree, _first, trigger, _fired) = menu_app();
    let mut ctx = EventContext::new();
    let rows = open(&mut tree, &mut ctx, trigger);

    tab(&mut tree, &mut ctx);
    assert_eq!(tree.focused(), Some(rows[0]));

    press(&mut tree, &mut ctx, NamedKey::ArrowDown);
    assert_eq!(tree.focused(), Some(rows[1]), "ArrowDown steps forward");

    press(&mut tree, &mut ctx, NamedKey::ArrowUp);
    assert_eq!(tree.focused(), Some(rows[0]), "ArrowUp steps back");

    // Wrap, same as Tab's — the arrows delegate to the tree's traversal rather
    // than deriving a sibling index, so they inherit its ring.
    press(&mut tree, &mut ctx, NamedKey::ArrowUp);
    assert_eq!(
        tree.focused(),
        Some(rows[2]),
        "ArrowUp off the first row wraps to the last"
    );
}

#[test]
fn arrow_down_steps_into_an_open_menu() {
    // ↓ is a menu's native way *in*, not just around. It has to be the tree's
    // job: the trigger holds focus but sits outside the layer, so it cannot
    // receive the key, and no row is focused yet to receive it either — left
    // alone the keystroke reaches no listener at all.
    let (mut tree, _first, trigger, _fired) = menu_app();
    let mut ctx = EventContext::new();
    let rows = open(&mut tree, &mut ctx, trigger);

    press(&mut tree, &mut ctx, NamedKey::ArrowDown);

    assert_eq!(
        tree.focused(),
        Some(rows[0]),
        "ArrowDown steps into the menu at the first row"
    );
    assert!(tree.focus_visible(), "and shows where it landed");
}

#[test]
fn arrow_up_steps_into_an_open_menu_at_the_last_row() {
    // The mirror, matching Shift+Tab and a native menu opened with ↑.
    let (mut tree, _first, trigger, _fired) = menu_app();
    let mut ctx = EventContext::new();
    let rows = open(&mut tree, &mut ctx, trigger);

    press(&mut tree, &mut ctx, NamedKey::ArrowUp);

    assert_eq!(tree.focused(), Some(rows[2]));
}

#[test]
fn stepping_in_with_an_arrow_does_not_shadow_a_field_that_wants_it() {
    // The guard on the step-in rule: it fires only while focus is *outside*
    // the layer. A modal's own field must keep the arrows for its caret, and
    // the moment focus is inside, the rule stops applying and the event routes
    // to the widget as usual.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(400.0));
    let trigger = tree.add_child(
        root,
        Button::new("open").on_click(|ctx| {
            ctx.push_layer(
                LayerOptions::modal(),
                Container::column().width(200.0).height(100.0),
                |tree, layer| {
                    tree.add_child(layer, Button::new("a"));
                    tree.add_child(layer, Button::new("b"));
                },
            );
        }),
    );
    layout(&mut tree);
    let mut ctx = EventContext::new();

    click(&mut tree, &mut ctx, trigger);
    frame(&mut tree, &mut ctx);
    let stops = tree.focusable_in_tab_order();

    // Focus is outside → the arrow steps in.
    press(&mut tree, &mut ctx, NamedKey::ArrowDown);
    assert_eq!(
        tree.focused(),
        Some(stops[0]),
        "sanity: the arrow stepped in"
    );

    // Focus is now inside. A `Button` ignores arrows, so nothing moves — the
    // tree must not step focus on its behalf, or a field that *does* want the
    // key would never see it.
    press(&mut tree, &mut ctx, NamedKey::ArrowDown);
    assert_eq!(
        tree.focused(),
        Some(stops[0]),
        "once focus is inside the layer, the arrows belong to the focused widget"
    );
}

#[test]
fn enter_fires_the_focused_row() {
    let (mut tree, _first, trigger, fired) = menu_app();
    let mut ctx = EventContext::new();
    let _rows = open(&mut tree, &mut ctx, trigger);

    tab(&mut tree, &mut ctx);
    press(&mut tree, &mut ctx, NamedKey::ArrowDown);
    press(&mut tree, &mut ctx, NamedKey::Enter);

    assert_eq!(
        fired.borrow().as_slice(),
        ["copy"],
        "Enter fires the row that is focused, and only it"
    );
}

#[test]
fn space_fires_the_focused_row() {
    // Space arrives through the character pipeline, not as a named key —
    // the same split `Button` handles.
    let (mut tree, _first, trigger, fired) = menu_app();
    let mut ctx = EventContext::new();
    let _rows = open(&mut tree, &mut ctx, trigger);

    tab(&mut tree, &mut ctx);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: ' ' }, &mut ctx);

    assert_eq!(fired.borrow().as_slice(), ["cut"]);
}

#[test]
fn an_unfocused_menu_swallows_enter_rather_than_firing_a_row() {
    // Nothing is focused inside the menu until the user steps in, and Enter
    // must not pick a row on its own — the row it picked would be one the user
    // was never shown.
    let (mut tree, _first, trigger, fired) = menu_app();
    let mut ctx = EventContext::new();
    let _rows = open(&mut tree, &mut ctx, trigger);

    press(&mut tree, &mut ctx, NamedKey::Enter);

    assert!(
        fired.borrow().is_empty(),
        "Enter with no row focused fires nothing"
    );
}

#[test]
fn opening_a_menu_does_not_prehighlight_a_row() {
    // Focus stays out on the trigger until the user steps in. Menus on Windows
    // and macOS open with nothing highlighted, and `resync_hover` already makes
    // that promise for a menu popped up under the cursor — a row pre-focused on
    // open would light the same fill and break it from the other side.
    let (mut tree, _first, trigger, _fired) = menu_app();
    let mut ctx = EventContext::new();
    let rows = open(&mut tree, &mut ctx, trigger);

    assert!(
        !rows.contains(&tree.focused().expect("the trigger keeps focus")),
        "opening a menu must not move focus onto a row"
    );
}

#[test]
fn a_disabled_row_is_not_a_tab_stop() {
    // A disabled row cannot fire, so parking focus on it would strand the user
    // on a dead stop. Same contract as `Button`.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(400.0));
    let trigger = tree.add_child(
        root,
        Button::new("menu").on_click(move |ctx| {
            ctx.push_layer(
                LayerOptions::popover(),
                Container::column().width(120.0),
                |tree, layer| {
                    tree.add_child(layer, MenuItem::new("cut", |_| {}));
                    tree.add_child(layer, MenuItem::new("copy", |_| {}).disabled(true));
                    tree.add_child(layer, MenuItem::new("paste", |_| {}));
                },
            );
        }),
    );
    layout(&mut tree);
    let mut ctx = EventContext::new();

    click(&mut tree, &mut ctx, trigger);
    frame(&mut tree, &mut ctx);

    assert_eq!(
        tree.focusable_in_tab_order().len(),
        2,
        "the disabled row drops out of the order, leaving the two live rows"
    );
}

#[test]
fn escaping_a_menu_stepped_into_returns_focus_to_the_trigger() {
    // Rows being focusable is also what arms the tree's return path: it fires
    // only when the pop is what cleared focus, which for a menu could never
    // happen while no row could hold any.
    let (mut tree, _first, trigger, _fired) = menu_app();
    let mut ctx = EventContext::new();
    let rows = open(&mut tree, &mut ctx, trigger);

    tab(&mut tree, &mut ctx);
    assert_eq!(
        tree.focused(),
        Some(rows[0]),
        "sanity: focus is inside the layer"
    );

    press(&mut tree, &mut ctx, NamedKey::Escape);
    frame(&mut tree, &mut ctx);

    assert_eq!(tree.layer_count(), 0, "sanity: Escape dismissed the menu");
    assert_eq!(
        tree.focused(),
        Some(trigger),
        "focus comes back to the trigger the user opened the menu from"
    );
    assert!(
        tree.focus_visible(),
        "they were navigating by keyboard when they dismissed, and still are"
    );
}

#[test]
fn tab_reaches_a_context_menu_whose_trigger_cannot_hold_focus() {
    // The case a trigger-side fix could never reach. A right-click deliberately
    // does not steal focus, so when a context menu opens, focus is still on
    // whatever the user last had — a widget *behind* the menu. There is no
    // focused trigger to route keys to; the rows have to be the answer.
    let fired: Fired = Rc::new(RefCell::new(Vec::new()));
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(400.0));
    let elsewhere = tree.add_child(root, Button::new("elsewhere"));
    let captured = Rc::clone(&fired);
    let surface = tree.add_child(
        root,
        Container::column()
            .width(200.0)
            .height(200.0)
            .on_context_menu(move |_pos, ctx| {
                ctx.push_layer(
                    LayerOptions::popover(),
                    Container::column().width(120.0),
                    populate(&captured),
                );
            }),
    );
    layout(&mut tree);
    let mut ctx = EventContext::new();

    // Put focus somewhere real, then right-click the surface.
    tree.focus(Some(elsewhere), &mut ctx);
    let r = tree.layout_rect(surface);
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(r.origin.x + 2.0, r.origin.y + 2.0),
            button: MouseButton::Right,
        },
        &mut ctx,
    );
    frame(&mut tree, &mut ctx);
    assert_eq!(
        tree.layer_count(),
        1,
        "sanity: the right-click opened a menu"
    );
    assert_eq!(
        tree.focused(),
        Some(elsewhere),
        "sanity: right-click leaves focus where it was — behind the menu"
    );

    let rows = tree.focusable_in_tab_order();
    tab(&mut tree, &mut ctx);
    assert_eq!(
        tree.focused(),
        Some(rows[0]),
        "Tab steps from a widget behind the menu into the menu itself"
    );

    press(&mut tree, &mut ctx, NamedKey::Enter);
    assert_eq!(fired.borrow().as_slice(), ["cut"]);
}
