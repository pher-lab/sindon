//! Clicking a non-focusable widget keeps the user's place in the tab order.
//!
//! Click-to-focus can only focus what is focusable, so a click on a note's
//! background — a container, a label, a padding gap — clears focus. With focus
//! gone the next Tab has nothing to step from and restarts at the top of the
//! window, which is how "Tab sends me back to the beginning" happens even
//! though every widget is reachable. The tree instead remembers *where* the
//! click landed and resumes the walk from there: the web platform's sequential
//! focus navigation starting point.
//!
//! The start point is a position between tab stops rather than a stop itself,
//! which is what lets a widget that can't be focused name a place in an order
//! it never appears in. These tests pin that position from both directions,
//! plus what happens when the anchor stops being resolvable (removed, hidden,
//! or walled off behind a layer) and the walk has to fall back.

use shroud_core::{Point, Theme};
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::layer::LayerOptions;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Button, Container};

const CARD_PAD: f32 = 20.0;

fn layout(tree: &mut WidgetTree) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 600.0, &mut engine, &theme);
}

/// The event loop applies a queued focus at the top of each redraw; nothing in
/// a dispatch does it, so tests have to turn the frame over too.
fn frame(tree: &mut WidgetTree, ctx: &mut EventContext) {
    tree.flush_pending_focus(ctx);
    layout(tree);
}

fn escape(tree: &mut WidgetTree, ctx: &mut EventContext) {
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Escape),
        },
        ctx,
    );
}

fn tab(tree: &mut WidgetTree, ctx: &mut EventContext) {
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Tab),
        },
        ctx,
    );
}

/// Shift+Tab, resetting modifiers after so a following event isn't read as a
/// chord (the event loop keeps them on the context, not on the event).
fn shift_tab(tree: &mut WidgetTree, ctx: &mut EventContext) {
    ctx.modifiers = Modifiers::SHIFT;
    tab(tree, ctx);
    ctx.modifiers = Modifiers::NONE;
}

/// Click a point in viewport coordinates.
fn click_at(tree: &mut WidgetTree, ctx: &mut EventContext, p: Point) {
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

/// Click a widget's own surface, inside its padding so the hit lands on it
/// rather than on a child — the "clicked the row, not the button" gesture this
/// whole feature is about.
fn click_background_of(tree: &mut WidgetTree, ctx: &mut EventContext, idx: usize) {
    let r = tree.layout_rect(idx);
    let p = Point::new(r.origin.x + CARD_PAD / 2.0, r.origin.y + CARD_PAD / 2.0);
    click_at(tree, ctx, p);
}

struct App {
    tree: WidgetTree,
    header: usize,
    /// Per card: (card container, its two buttons).
    cards: Vec<(usize, usize, usize)>,
    footer: usize,
    /// A non-focusable node *after* every tab stop in tree order.
    tail: usize,
}

/// A window shaped like a note list: a header button, two cards that each wrap
/// two buttons in a padded container, a footer button, and a trailing filler.
///
/// Tab order is `[header, a0, b0, a1, b1, footer]`; the cards and the tail are
/// the non-focusable surfaces a user clicks by accident.
fn note_list() -> App {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(600.0));
    let header = tree.add_child(root, Button::new("header"));
    let cards = (0..2)
        .map(|i| {
            let card = tree.add_child(
                root,
                Container::row().padding(CARD_PAD).width(400.0).height(80.0),
            );
            let a = tree.add_child(card, Button::new(format!("open{i}")));
            let b = tree.add_child(card, Button::new(format!("star{i}")));
            (card, a, b)
        })
        .collect();
    let footer = tree.add_child(root, Button::new("footer"));
    let tail = tree.add_child(root, Container::column().width(400.0).height(100.0));
    layout(&mut tree);
    App {
        tree,
        header,
        cards,
        footer,
        tail,
    }
}

#[test]
fn fixture_puts_the_card_background_under_the_click() {
    // Everything below reads "clicked the card, not its buttons". If padding
    // ever stopped leaving bare surface, those tests would still pass while
    // testing a click on a Button instead.
    let mut app = note_list();
    let mut ctx = EventContext::new();
    let (card1, a1, _) = app.cards[1];

    click_background_of(&mut app.tree, &mut ctx, card1);
    assert_eq!(
        app.tree.focused(),
        None,
        "a card is not focusable, so clicking it must clear focus"
    );
    assert!(
        app.tree.layout_rect(a1).origin.x > app.tree.layout_rect(card1).origin.x,
        "the card's buttons must be inset by its padding"
    );
}

#[test]
fn tab_after_clicking_a_card_enters_that_card() {
    let mut app = note_list();
    let mut ctx = EventContext::new();
    let (card1, a1, _) = app.cards[1];

    click_background_of(&mut app.tree, &mut ctx, card1);
    tab(&mut app.tree, &mut ctx);

    assert_eq!(
        app.tree.focused(),
        Some(a1),
        "Tab should continue from the clicked card, not restart at the header"
    );
}

#[test]
fn shift_tab_after_clicking_a_card_leaves_it_backwards() {
    let mut app = note_list();
    let mut ctx = EventContext::new();
    let (card1, _, _) = app.cards[1];
    let (_, _, b0) = app.cards[0];

    click_background_of(&mut app.tree, &mut ctx, card1);
    shift_tab(&mut app.tree, &mut ctx);

    // The start point sits *before* the card's own buttons, so stepping back
    // from it lands on the stop preceding the card — not inside it.
    assert_eq!(
        app.tree.focused(),
        Some(b0),
        "Shift+Tab should step to the stop before the clicked card"
    );
}

#[test]
fn the_ring_shows_when_tab_resumes_from_a_click() {
    // Resuming is keyboard navigation like any other Tab: the point of the
    // feature is showing the user where they are, which needs the ring.
    let mut app = note_list();
    let mut ctx = EventContext::new();
    let (card1, _, _) = app.cards[1];

    click_background_of(&mut app.tree, &mut ctx, card1);
    assert!(!app.tree.focus_visible(), "nothing focused → no ring");
    tab(&mut app.tree, &mut ctx);
    assert!(app.tree.focus_visible());
}

#[test]
fn a_start_point_past_every_stop_wraps() {
    let mut app = note_list();
    let mut ctx = EventContext::new();
    let tail = app.tail;

    // The tail follows all six stops in tree order, so forward has nowhere
    // left to go and must wrap, while backward reaches the last stop.
    click_background_of(&mut app.tree, &mut ctx, tail);
    tab(&mut app.tree, &mut ctx);
    assert_eq!(
        app.tree.focused(),
        Some(app.header),
        "forward wraps to first"
    );

    click_background_of(&mut app.tree, &mut ctx, tail);
    shift_tab(&mut app.tree, &mut ctx);
    assert_eq!(
        app.tree.focused(),
        Some(app.footer),
        "backward finds the last"
    );
}

#[test]
fn focusing_a_widget_retires_the_start_point() {
    let mut app = note_list();
    let mut ctx = EventContext::new();
    let (card1, _, _) = app.cards[1];
    let (_, a0, b0) = app.cards[0];

    // Click a card (start point at card1), then click a real button: focus is
    // now the place Tab steps from, and the stale anchor must not resurface.
    click_background_of(&mut app.tree, &mut ctx, card1);
    let r = app.tree.layout_rect(a0);
    click_at(
        &mut app.tree,
        &mut ctx,
        Point::new(r.origin.x + 2.0, r.origin.y + 2.0),
    );
    assert_eq!(app.tree.focused(), Some(a0), "click-to-focus still works");

    tab(&mut app.tree, &mut ctx);
    assert_eq!(
        app.tree.focused(),
        Some(b0),
        "Tab must step from the focused button, not the earlier click"
    );
}

#[test]
fn a_click_outside_the_tree_clears_the_start_point() {
    let mut app = note_list();
    let mut ctx = EventContext::new();
    let (card1, _, _) = app.cards[1];

    click_background_of(&mut app.tree, &mut ctx, card1);
    // Nothing under the cursor: this click names no place, and must not leave
    // the previous one standing.
    click_at(&mut app.tree, &mut ctx, Point::new(5_000.0, 5_000.0));
    tab(&mut app.tree, &mut ctx);

    assert_eq!(
        app.tree.focused(),
        Some(app.header),
        "with no anchor Tab starts at the top"
    );
}

#[test]
fn removing_the_clicked_widget_falls_back_to_the_top() {
    let mut app = note_list();
    let mut ctx = EventContext::new();
    let (card1, _, _) = app.cards[1];

    click_background_of(&mut app.tree, &mut ctx, card1);
    // The anchor is stored as a raw index and resolved lazily, so this is the
    // path where a stale one has to be noticed rather than trusted.
    app.tree.remove(card1);
    layout(&mut app.tree);
    tab(&mut app.tree, &mut ctx);

    assert_eq!(
        app.tree.focused(),
        Some(app.header),
        "an anchor that no longer exists must degrade to the old behavior"
    );
}

#[test]
fn a_pointer_only_trigger_anchors_tab_when_its_layer_closes() {
    // Where this meets the layer-dismiss return: focus cannot go back to a
    // `Container::on_press` trigger, because by design it is not a tab stop.
    // The place still can, and it is the same place a click on the trigger
    // would have anchored — so the pop leaves an anchor rather than nothing.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(600.0));
    let _header = tree.add_child(root, Button::new("header"));
    let trigger = tree.add_child(
        root,
        Container::column()
            .width(400.0)
            .height(40.0)
            .on_press(|_, ctx| {
                ctx.push_layer(
                    LayerOptions::modal(),
                    Container::column().width(200.0).height(100.0),
                    |tree, layer| {
                        tree.add_child(layer, Button::new("field"));
                    },
                );
            }),
    );
    let footer = tree.add_child(root, Button::new("footer"));
    layout(&mut tree);

    let mut ctx = EventContext::new();
    let r = tree.layout_rect(trigger);
    click_at(
        &mut tree,
        &mut ctx,
        Point::new(r.origin.x + 2.0, r.origin.y + 2.0),
    );
    frame(&mut tree, &mut ctx);
    assert_eq!(tree.layer_count(), 1, "sanity: the press opened the layer");

    // Tab into the layer first: the field taking focus retires the anchor the
    // click on the trigger left, so only the pop can put one back — otherwise
    // this would pass on the click alone.
    tab(&mut tree, &mut ctx);
    assert!(tree.focused().is_some(), "sanity: Tab entered the layer");
    escape(&mut tree, &mut ctx);
    frame(&mut tree, &mut ctx);

    assert_eq!(tree.layer_count(), 0, "sanity: Escape dismissed the layer");
    assert_eq!(
        tree.focused(),
        None,
        "a pointer-only trigger is not somewhere focus can live"
    );

    tab(&mut tree, &mut ctx);
    assert_eq!(
        tree.focused(),
        Some(footer),
        "Tab should resume past the trigger, not restart at the header"
    );
}

#[test]
fn a_layer_ignores_a_start_point_left_behind_it() {
    let mut app = note_list();
    let mut ctx = EventContext::new();
    let (card1, _, _) = app.cards[1];

    click_background_of(&mut app.tree, &mut ctx, card1);
    let layer = app
        .tree
        .push_layer(LayerOptions::modal(), Container::column().width(200.0));
    let field = app.tree.add_child(layer, Button::new("field"));
    layout(&mut app.tree);

    tab(&mut app.tree, &mut ctx);

    assert_eq!(
        app.tree.focused(),
        Some(field),
        "Tab is trapped in the layer; an anchor outside it names no position"
    );
}
