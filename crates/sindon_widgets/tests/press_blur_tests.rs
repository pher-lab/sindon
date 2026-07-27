//! Tests for the tag-editor follow-up primitives:
//!
//! - `Container::on_press` — left-press handler firing on `MouseDown`
//!   (not release), so a commit survives a focus change queued on the same
//!   click.
//! - `Input::on_blur` — fires on `FocusLost`, for dismissing transient UI.
//! - `Input::on_backspace_empty` — fires on Backspace into an empty buffer,
//!   the signal a chip editor uses to delete the last chip.
//!
//! The headline test, [`suggestion_press_commits_even_when_blur_dismisses`],
//! reproduces the autocomplete race at the framework level: clicking a
//! suggestion blurs the input (which dismisses the list) *and* commits the
//! suggestion, in that order, within a single `MouseDown` dispatch.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sindon_core::{Color, Point, Theme};
use sindon_reactive::Signal;
use sindon_text::TextEngine;
use sindon_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Container, Input};

fn measured_layout(tree: &mut WidgetTree, w: f32, h: f32) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(w, h, &mut engine, &theme);
}

fn dispatch(tree: &mut WidgetTree, ev: WidgetEvent) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(&ev, &mut ctx);
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

fn right_down(tree: &mut WidgetTree, x: f32, y: f32) {
    dispatch(
        tree,
        WidgetEvent::MouseDown {
            position: Point::new(x, y),
            button: MouseButton::Right,
        },
    );
}

fn key(tree: &mut WidgetTree, named: NamedKey) {
    dispatch(
        tree,
        WidgetEvent::KeyDown {
            key: Key::Named(named),
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

// ── Container::on_press ──────────────────────────────────────────────────

#[test]
fn container_on_press_fires_on_left_mouse_down() {
    let fired = Rc::new(Cell::new(0u32));
    let last = Rc::new(Cell::new(Point::new(-1.0, -1.0)));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(120.0).padding(10.0));
    let f = Rc::clone(&fired);
    let l = Rc::clone(&last);
    let panel = tree.add_child(
        root,
        Container::row()
            .width(100.0)
            .height(40.0)
            .background(Color::rgb(0.2, 0.2, 0.2))
            .on_press(move |pos, _ctx| {
                f.set(f.get() + 1);
                l.set(pos);
            }),
    );
    measured_layout(&mut tree, 200.0, 120.0);

    let r = tree.layout_rect(panel);
    // A single MouseDown (no matching MouseUp) is enough — the press is the
    // trigger.
    left_down(&mut tree, r.origin.x + 20.0, r.origin.y + 10.0);

    assert_eq!(fired.get(), 1, "on_press fires once on the press itself");
    let p = last.get();
    assert!(
        (p.x - (r.origin.x + 20.0)).abs() < 0.5,
        "local x (got {})",
        p.x
    );
    assert!(
        (p.y - (r.origin.y + 10.0)).abs() < 0.5,
        "local y (got {})",
        p.y
    );
}

#[test]
fn container_on_press_rect_hands_back_the_trigger_rect() {
    // FW-21 / G5: on_press_rect reports the container's own layout rect (not
    // the cursor point) so a menu button can anchor a popover to itself.
    let fired = Rc::new(Cell::new(0u32));
    let got = Rc::new(Cell::new(sindon_core::Rect::ZERO));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(120.0).padding(10.0));
    let f = Rc::clone(&fired);
    let g = Rc::clone(&got);
    let panel = tree.add_child(
        root,
        Container::row()
            .width(100.0)
            .height(40.0)
            .background(Color::rgb(0.2, 0.2, 0.2))
            .on_press_rect(move |rect, _ctx| {
                f.set(f.get() + 1);
                g.set(rect);
            }),
    );
    measured_layout(&mut tree, 200.0, 120.0);

    let expected = tree.layout_rect(panel);
    // Click anywhere inside the panel — the handler should still get the box.
    left_down(
        &mut tree,
        expected.origin.x + 20.0,
        expected.origin.y + 10.0,
    );

    assert_eq!(fired.get(), 1, "on_press_rect fires once on the press");
    let r = got.get();
    assert!(
        (r.origin.x - expected.origin.x).abs() < 0.5
            && (r.origin.y - expected.origin.y).abs() < 0.5,
        "origin: got {:?}, want {:?}",
        r.origin,
        expected.origin
    );
    assert!(
        (r.size.width - 100.0).abs() < 0.5 && (r.size.height - 40.0).abs() < 0.5,
        "size: got {:?}, want 100x40",
        r.size
    );
}

#[test]
fn container_on_press_ignores_right_click() {
    let fired = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(160.0).height(100.0));
    let f = Rc::clone(&fired);
    let panel = tree.add_child(
        root,
        Container::row()
            .width(100.0)
            .height(40.0)
            .on_press(move |_pos, _ctx| f.set(f.get() + 1)),
    );
    measured_layout(&mut tree, 160.0, 100.0);

    let (x, y) = center(&tree, panel);
    right_down(&mut tree, x, y);
    assert_eq!(fired.get(), 0, "on_press is a left-button hook only");
}

// ── Input::on_blur ───────────────────────────────────────────────────────

#[test]
fn input_on_blur_fires_when_focus_moves_away() {
    let blurred = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(240.0).height(160.0).gap(8.0));
    let b = Rc::clone(&blurred);
    let a = tree.add_child(root, Input::new().on_blur(move |_ctx| b.set(b.get() + 1)));
    let other = tree.add_child(root, Input::new());
    measured_layout(&mut tree, 240.0, 160.0);

    let (ax, ay) = center(&tree, a);
    left_down(&mut tree, ax, ay);
    assert_eq!(tree.focused(), Some(a), "first input takes focus");
    assert_eq!(blurred.get(), 0, "no blur yet");

    let (ox, oy) = center(&tree, other);
    left_down(&mut tree, ox, oy);
    assert_eq!(
        tree.focused(),
        Some(other),
        "focus moves to the second input"
    );
    assert_eq!(blurred.get(), 1, "on_blur fires exactly once on focus move");
}

// ── Input::on_backspace_empty ──────────────────────────────────────────────

#[test]
fn input_on_backspace_empty_fires_only_on_empty_buffer() {
    let empty_bs = Rc::new(Cell::new(0u32));
    let nonempty_bs = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(240.0).height(160.0).gap(8.0));

    let e = Rc::clone(&empty_bs);
    let empty = tree.add_child(
        root,
        Input::new().on_backspace_empty(move |_ctx| e.set(e.get() + 1)),
    );
    let n = Rc::clone(&nonempty_bs);
    let filled = tree.add_child(
        root,
        Input::new()
            .with_value("abc")
            .on_backspace_empty(move |_ctx| n.set(n.get() + 1)),
    );
    measured_layout(&mut tree, 240.0, 160.0);

    // Empty field: focus then Backspace → fires.
    let (ex, ey) = center(&tree, empty);
    left_down(&mut tree, ex, ey);
    key(&mut tree, NamedKey::Backspace);
    assert_eq!(
        empty_bs.get(),
        1,
        "empty buffer + Backspace hands off to app"
    );

    // Non-empty field with the cursor driven to the start: Backspace there is
    // an inert no-op and must NOT fire the hook (the field isn't empty).
    let (fx, fy) = center(&tree, filled);
    left_down(&mut tree, fx, fy);
    key(&mut tree, NamedKey::Home);
    key(&mut tree, NamedKey::Backspace);
    assert_eq!(
        nonempty_bs.get(),
        0,
        "Backspace at the start of non-empty text must not fire on_backspace_empty"
    );
}

// ── The autocomplete race ──────────────────────────────────────────────────

#[test]
fn suggestion_press_commits_even_when_blur_dismisses() {
    // Mirrors the knot tag editor: an Input whose on_blur tears down a
    // suggestion list, and a suggestion built as a Container::on_press. A
    // single click on the suggestion must (1) commit the value and (2)
    // dismiss the list — the commit must survive the teardown.
    let committed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(260.0).height(200.0).gap(6.0));

    // Suggestion list container — captured so on_blur can rebuild it empty.
    let suggestions = tree.add_child(root, Container::column().width(220.0).gap(2.0));

    let sugg_parent = suggestions;
    let _sig = Signal::new(String::new());
    let input = tree.add_child(
        root,
        Input::new().on_blur(move |ctx| {
            // Dismiss: drop every suggestion row.
            ctx.rebuild_children(sugg_parent, |_tree, _parent| {});
        }),
    );

    // One suggestion row that commits "rust" on press.
    let commit = Rc::clone(&committed);
    let sugg_row = tree.add_child(
        suggestions,
        Container::row()
            .width(200.0)
            .height(24.0)
            .on_press(move |_pos, _ctx| commit.borrow_mut().push("rust".to_string())),
    );
    measured_layout(&mut tree, 260.0, 200.0);

    // Focus the input first (so the suggestion click triggers a real blur).
    let (ix, iy) = center(&tree, input);
    left_down(&mut tree, ix, iy);
    assert_eq!(
        tree.focused(),
        Some(input),
        "input is focused before the click"
    );

    // Click the suggestion: focus(None) → input FocusLost → on_blur queues the
    // list teardown; then the suggestion's on_press commits; then commands
    // drain. The commit (queued earlier on the press) survives.
    let (sx, sy) = center(&tree, sugg_row);
    left_down(&mut tree, sx, sy);

    assert_eq!(
        committed.borrow().as_slice(),
        ["rust"],
        "clicking the suggestion commits even though blur tore the list down"
    );
    assert!(
        !tree.contains(sugg_row),
        "the suggestion list was dismissed on blur"
    );
    assert_eq!(tree.focused(), None, "focus left the input on the click");
}
