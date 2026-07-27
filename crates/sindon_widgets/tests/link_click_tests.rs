//! Integration tests for clickable inline links (`TextWidget::on_link_click`).
//!
//! The flow under test mirrors the real frame loop: lay the tree out (which
//! sizes the rich text), `paint` it once (which is when the widget caches its
//! clickable hit regions from the shaped span geometry), then dispatch mouse
//! events and assert the handler fires with the clicked span's link target.

use std::cell::RefCell;
use std::rc::Rc;

use sindon_core::{Point, Theme};
use sindon_text::{TextEngine, TextSpan};
use sindon_widgets::event::{EventContext, MouseButton, WidgetEvent};
use sindon_widgets::paint::PaintContext;
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Container, TextWidget};

fn layout(tree: &mut WidgetTree, w: f32, h: f32) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(w, h, &mut engine, &theme);
}

/// Paint the tree so each `TextWidget` refreshes its cached link hit-regions.
fn paint(tree: &WidgetTree) {
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
}

fn mouse_down(tree: &mut WidgetTree, x: f32, y: f32) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
}

fn mouse_up(tree: &mut WidgetTree, x: f32, y: f32) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
}

/// Build a rich text whose first span is the link, so the link's hit region
/// starts at the widget's left edge — clicking near `origin.x` lands on it
/// without the test having to predict shaped glyph offsets.
fn link_first_tree(sink: Rc<RefCell<Vec<String>>>) -> (WidgetTree, usize) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let text = tree.add_child(
        root,
        TextWidget::rich(vec![
            TextSpan::new("LINKWORD").link("note://target"),
            TextSpan::new(" and some trailing plain text"),
        ])
        .on_link_click(move |target, _ctx| sink.borrow_mut().push(target.to_string())),
    );
    (tree, text)
}

#[test]
fn clicking_a_link_span_fires_handler_with_its_target() {
    let fired: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let (mut tree, text) = link_first_tree(Rc::clone(&fired));

    layout(&mut tree, 400.0, 200.0);
    paint(&tree);

    let r = tree.layout_rect(text);
    let x = r.origin.x + 4.0; // inside span 0 (the link), near its left edge
    let y = r.origin.y + 8.0;
    mouse_down(&mut tree, x, y);
    mouse_up(&mut tree, x, y);

    assert_eq!(
        fired.borrow().as_slice(),
        ["note://target"],
        "press+release on the link span should fire the handler once with its target"
    );
}

#[test]
fn clicking_outside_any_link_does_not_fire() {
    let fired: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let (mut tree, text) = link_first_tree(Rc::clone(&fired));

    layout(&mut tree, 400.0, 200.0);
    paint(&tree);

    // Far right edge of the widget — past the link span, on the plain tail
    // (or empty stretched space). The widget still receives the event; it just
    // doesn't map to a link.
    let r = tree.layout_rect(text);
    let x = r.origin.x + r.size.width - 4.0;
    let y = r.origin.y + 8.0;
    mouse_down(&mut tree, x, y);
    mouse_up(&mut tree, x, y);

    assert!(
        fired.borrow().is_empty(),
        "a click that misses every link region must not fire the handler"
    );
}

#[test]
fn press_on_link_release_off_link_does_not_fire() {
    // Pressing on the link but releasing elsewhere is a cancelled click — the
    // pressed-link guard must keep the handler from firing.
    let fired: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let (mut tree, text) = link_first_tree(Rc::clone(&fired));

    layout(&mut tree, 400.0, 200.0);
    paint(&tree);

    let r = tree.layout_rect(text);
    let y = r.origin.y + 8.0;
    mouse_down(&mut tree, r.origin.x + 4.0, y); // press on the link
    mouse_up(&mut tree, r.origin.x + r.size.width - 4.0, y); // release off it

    assert!(
        fired.borrow().is_empty(),
        "press-then-release-elsewhere should be a cancelled click"
    );
}

#[test]
fn text_without_link_handler_ignores_clicks() {
    // A rich widget with link spans but no handler installed must not panic
    // and must leave the click unconsumed (so surrounding widgets still get
    // their turn). We assert via "no handler ran" implicitly: there's no
    // handler — the point is the dispatch path stays inert.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let text = tree.add_child(
        root,
        TextWidget::rich(vec![TextSpan::new("LINKWORD").link("note://target")]),
    );
    layout(&mut tree, 400.0, 200.0);
    paint(&tree);

    let r = tree.layout_rect(text);
    // Should simply not panic.
    mouse_down(&mut tree, r.origin.x + 4.0, r.origin.y + 8.0);
    mouse_up(&mut tree, r.origin.x + 4.0, r.origin.y + 8.0);
}

#[test]
fn truncated_away_link_is_not_reachable_through_the_ellipsis() {
    // Truncation rebuilds the span list — surviving spans, then `…` — so a
    // dropped span's index can be re-used by the ellipsis. Mapping a shaped
    // span box back through the *original* list would hand the ellipsis the
    // link that was elided, making a click on "…" navigate somewhere the user
    // can no longer see.
    let fired: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&fired);

    let mut tree = WidgetTree::new();
    // Narrow root so the widget's laid-out width forces a cut inside span 0,
    // dropping the link span entirely.
    let root = tree.set_root(Container::column().width(120.0).height(200.0));
    let text = tree.add_child(
        root,
        TextWidget::rich(vec![
            TextSpan::new("aaaa bbbb cccc dddd eeee ffff gggg"),
            TextSpan::new("LINKWORD").link("note://elided"),
        ])
        .truncate(true)
        .on_link_click(move |target, _ctx| sink.borrow_mut().push(target.to_string())),
    );

    layout(&mut tree, 120.0, 200.0);
    paint(&tree);

    // Sweep the whole row rather than guessing where the ellipsis landed.
    let r = tree.layout_rect(text);
    let y = r.origin.y + 8.0;
    let mut x = r.origin.x + 2.0;
    while x < r.origin.x + r.size.width {
        mouse_down(&mut tree, x, y);
        mouse_up(&mut tree, x, y);
        x += 4.0;
    }

    assert!(
        fired.borrow().is_empty(),
        "no click on a truncated line may fire the elided span's link; fired {:?}",
        fired.borrow()
    );
}

#[test]
fn a_surviving_link_span_still_fires_when_the_line_is_truncated() {
    // The mirror of the test above: truncation must not make links inert, so
    // a link that is still on screen keeps its hit region. Guards against
    // "fix the false positive by dropping all hits".
    let fired: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&fired);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(120.0).height(200.0));
    let text = tree.add_child(
        root,
        TextWidget::rich(vec![
            TextSpan::new("LINK").link("note://kept"),
            TextSpan::new(" aaaa bbbb cccc dddd eeee ffff gggg"),
        ])
        .truncate(true)
        .on_link_click(move |target, _ctx| sink.borrow_mut().push(target.to_string())),
    );

    layout(&mut tree, 120.0, 200.0);
    paint(&tree);

    let r = tree.layout_rect(text);
    let x = r.origin.x + 4.0; // inside the leading link span
    let y = r.origin.y + 8.0;
    mouse_down(&mut tree, x, y);
    mouse_up(&mut tree, x, y);

    assert_eq!(
        fired.borrow().as_slice(),
        ["note://kept"],
        "a link that survives the cut keeps its hit region"
    );
}
