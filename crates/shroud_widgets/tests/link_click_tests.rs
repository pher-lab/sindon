//! Integration tests for clickable inline links (`TextWidget::on_link_click`).
//!
//! The flow under test mirrors the real frame loop: lay the tree out (which
//! sizes the rich text), `paint` it once (which is when the widget caches its
//! clickable hit regions from the shaped span geometry), then dispatch mouse
//! events and assert the handler fires with the clicked span's link target.

use std::cell::RefCell;
use std::rc::Rc;

use shroud_core::{Point, Theme};
use shroud_text::{TextEngine, TextSpan};
use shroud_widgets::event::{EventContext, MouseButton, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, TextWidget};

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
