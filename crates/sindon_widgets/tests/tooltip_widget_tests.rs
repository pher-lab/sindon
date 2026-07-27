//! The framework `Tooltip` widget — the delayed bubble on top of the FW-13
//! primitives (`tooltip_tests.rs` covers those primitives themselves).
//!
//! What the widget owes its users, and what these tests pin down:
//!
//! - the tip waits out its delay instead of flashing as the cursor passes,
//!   and the wait needs no app-side periodic tick;
//! - it dismisses on hover exit;
//! - and — the part every app previously had to re-derive — it survives the
//!   teardown paths that produce no `MouseLeave`. A tip stranded by a rebuilt
//!   list or a screen swap used to leave the controller believing a tip was
//!   still up, silently killing *every* later tooltip.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use sindon_core::{Point, Theme};
use sindon_text::TextEngine;
use sindon_widgets::event::{EventContext, MouseButton, WidgetEvent};
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Button, Container, TextWidget, Tooltip};

fn measured_layout(tree: &mut WidgetTree) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);
}

fn mouse_move(tree: &mut WidgetTree, x: f32, y: f32) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(x, y),
        },
        &mut ctx,
    );
}

fn center(tree: &WidgetTree, idx: usize) -> (f32, f32) {
    let r = tree.layout_rect(idx);
    (
        r.origin.x + r.size.width / 2.0,
        r.origin.y + r.size.height / 2.0,
    )
}

/// A root with two side-by-side tooltip triggers, each wrapping a label.
/// Returns (tree, first trigger, second trigger).
fn two_triggers(delay: Duration) -> (WidgetTree, usize, usize) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(300.0));
    let a = tree.add_child(root, Tooltip::new("Alpha tip").delay(delay));
    tree.add_child(a, TextWidget::new("AAAA"));
    let b = tree.add_child(root, Tooltip::new("Beta tip").delay(delay));
    tree.add_child(b, TextWidget::new("BBBB"));
    (tree, a, b)
}

#[test]
fn tip_waits_out_its_delay_then_shows() {
    let (mut tree, a, _b) = two_triggers(Duration::from_millis(60));
    measured_layout(&mut tree);

    let (x, y) = center(&tree, a);
    mouse_move(&mut tree, x, y);

    // Armed, but not yet due: the frame pump must not show it.
    assert!(!tree.sync_tooltips(), "tip must not fire before its delay");
    assert_eq!(tree.layer_count(), 0, "no bubble during the delay");

    std::thread::sleep(Duration::from_millis(80));

    assert!(tree.sync_tooltips(), "tip is due after the delay elapses");
    assert_eq!(tree.layer_count(), 1, "the bubble is up");
    // Idempotent: a second pump on a later frame must not stack bubbles.
    assert!(!tree.sync_tooltips(), "already shown");
    assert_eq!(tree.layer_count(), 1);
}

#[test]
fn tip_dismisses_when_the_cursor_leaves() {
    let (mut tree, a, _b) = two_triggers(Duration::ZERO);
    measured_layout(&mut tree);

    let (x, y) = center(&tree, a);
    mouse_move(&mut tree, x, y);
    assert!(tree.sync_tooltips());
    assert_eq!(tree.layer_count(), 1);

    // Off both triggers — the click-through layer does not swallow the move,
    // so the trigger still gets its MouseLeave.
    mouse_move(&mut tree, 399.0, 299.0);
    assert_eq!(tree.layer_count(), 0, "hover exit dismisses the tip");
}

#[test]
fn moving_between_triggers_swaps_the_tip() {
    let (mut tree, a, b) = two_triggers(Duration::ZERO);
    measured_layout(&mut tree);

    let (ax, ay) = center(&tree, a);
    mouse_move(&mut tree, ax, ay);
    assert!(tree.sync_tooltips());
    assert_eq!(tree.layer_count(), 1);

    let (bx, by) = center(&tree, b);
    mouse_move(&mut tree, bx, by);
    // A's tip is dismissed by its leave; B's is armed and shows on the pump.
    assert_eq!(tree.layer_count(), 0, "the old tip goes down first");
    assert!(tree.sync_tooltips(), "the new trigger's tip is armed");
    assert_eq!(tree.layer_count(), 1, "exactly one tip at a time");
}

#[test]
fn empty_text_never_arms() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(300.0));
    // A reactive tip that resolves to nothing — e.g. a translation key with no
    // entry yet. An empty bubble would be a floating rectangle.
    let t = tree.add_child(root, Tooltip::reactive(String::new).delay(Duration::ZERO));
    tree.add_child(t, TextWidget::new("AAAA"));
    measured_layout(&mut tree);

    let (x, y) = center(&tree, t);
    mouse_move(&mut tree, x, y);
    assert!(!tree.sync_tooltips(), "empty text must not push a bubble");
    assert_eq!(tree.layer_count(), 0);
}

#[test]
fn removing_the_trigger_takes_its_tip_with_it() {
    // The teardown that produces no `MouseLeave`: a list rebuild drops the row
    // the cursor is on. Without a cancel at the source the bubble would hang
    // there with nothing left to dismiss it.
    let (mut tree, a, b) = two_triggers(Duration::ZERO);
    measured_layout(&mut tree);

    let (x, y) = center(&tree, a);
    mouse_move(&mut tree, x, y);
    assert!(tree.sync_tooltips());
    assert_eq!(tree.layer_count(), 1);

    tree.remove(a);
    assert_eq!(
        tree.layer_count(),
        0,
        "dropping the hovered trigger dismisses its tip"
    );

    // And the controller is not wedged: the surviving trigger still works.
    measured_layout(&mut tree);
    let (bx, by) = center(&tree, b);
    mouse_move(&mut tree, bx, by);
    assert!(tree.sync_tooltips(), "later tooltips still fire");
    assert_eq!(tree.layer_count(), 1);
}

#[test]
fn a_screen_swap_does_not_wedge_future_tips() {
    // The knot-era trap in its original form: an auto-lock `replace_screen`
    // fires while a tip is up and tears down every layer. An app-side
    // controller that only tracked "a tip is shown" would keep believing one
    // was up and suppress every tooltip for the rest of the session — knot
    // needed an explicit `tooltip::reset()` in each screen's builder to avoid
    // it. Two independent things make that unnecessary here: the removal of
    // the hovered trigger cancels the tip at the source, and re-arming clears
    // the record regardless (dismissal is by node index, and indices are
    // never recycled, so a stale one is a provable no-op).
    let (mut tree, a, _b) = two_triggers(Duration::ZERO);
    measured_layout(&mut tree);

    let (x, y) = center(&tree, a);
    mouse_move(&mut tree, x, y);
    assert!(tree.sync_tooltips());
    assert_eq!(tree.layer_count(), 1);

    let mut ctx = EventContext::new();
    ctx.replace_screen(|tree| {
        let root = tree.set_root(Container::row().width(400.0).height(300.0));
        let t = tree.add_child(root, Tooltip::new("Fresh tip").delay(Duration::ZERO));
        tree.add_child(t, TextWidget::new("CCCC"));
    });
    tree.apply_pending_commands(&mut ctx);
    assert_eq!(tree.layer_count(), 0, "the swap tore down the tip layer");

    measured_layout(&mut tree);
    let fresh = tree.children(tree.root().expect("new root"))[0];
    let (fx, fy) = center(&tree, fresh);
    mouse_move(&mut tree, fx, fy);
    assert!(
        tree.sync_tooltips(),
        "a tooltip on the new screen still fires"
    );
    assert_eq!(tree.layer_count(), 1);
}

#[test]
fn the_tip_does_not_swallow_clicks_meant_for_the_tree() {
    // The bubble is a click-through layer, so a button under it (or anywhere
    // else) still gets its click. This is what lets a tip sit over a toolbar
    // without deadening it.
    let clicked = Rc::new(Cell::new(false));
    let sink = Rc::clone(&clicked);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(300.0));
    let t = tree.add_child(root, Tooltip::new("Bold").delay(Duration::ZERO));
    let btn = tree.add_child(t, Button::new("B").on_click(move |_| sink.set(true)));
    measured_layout(&mut tree);

    let (x, y) = center(&tree, btn);
    mouse_move(&mut tree, x, y);
    assert!(tree.sync_tooltips());
    assert_eq!(tree.layer_count(), 1);

    let mut ctx = EventContext::new();
    for ev in [
        WidgetEvent::MouseDown {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
        WidgetEvent::MouseUp {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
    ] {
        tree.dispatch_event(&ev, &mut ctx);
    }

    assert!(
        clicked.get(),
        "a click under a shown tip must still reach the button"
    );
}

#[test]
fn the_wrapper_paints_nothing_of_its_own() {
    // Carrying a tip must not restyle the widget underneath — no hover fill,
    // no background. Compare against the same tree without the wrapper.
    fn rects_painted(with_tooltip: bool) -> usize {
        let mut tree = WidgetTree::new();
        let root = tree.set_root(Container::row().width(400.0).height(300.0));
        let parent = if with_tooltip {
            tree.add_child(root, Tooltip::new("tip").delay(Duration::ZERO))
        } else {
            root
        };
        tree.add_child(parent, TextWidget::new("AAAA"));
        measured_layout(&mut tree);

        let (x, y) = center(&tree, parent);
        mouse_move(&mut tree, x, y);

        let mut ctx = sindon_widgets::paint::PaintContext::default();
        tree.paint(&mut ctx);
        ctx.rects.len()
    }

    assert_eq!(
        rects_painted(true),
        rects_painted(false),
        "the tooltip wrapper must add no paint of its own while hovered"
    );
}
