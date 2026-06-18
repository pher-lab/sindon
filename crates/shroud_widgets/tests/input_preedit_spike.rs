//! FW-1 IME-preedit spike: widget-level proof.
//!
//! Surfaced by dogfooding (`docs/dogfood-log.md` #25): once the IME is
//! app-driven, winit stops drawing an inline composition string and expects the
//! app to render the preedit. `Input` now splices the uncommitted composition
//! into the buffer *for display only* and underlines it. These tests pin:
//!
//! 1. the composition is visible (glyphs are drawn) even into an empty buffer,
//! 2. it is underlined while composing and the underline clears when it ends,
//! 3. the preedit never enters the bound value until the IME commits, and
//! 4. focus loss drops a half-composed preedit.

use shroud_core::{Point, Rect, Theme};
use shroud_reactive::Signal;
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, MouseButton, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Input};

const W: f32 = 400.0;
const H: f32 = 120.0;

fn paint(tree: &WidgetTree) -> PaintContext {
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
    ctx
}

/// Build a single focused `Input` (optionally bound to `value`) inside a sized
/// column, focus it via a click, and resolve the focusing click with a warm
/// paint so the caret sits at the buffer start before the test proceeds.
fn focused_input(value: Option<Signal<String>>) -> (WidgetTree, Rect, EventContext) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));
    let input = match value {
        Some(sig) => Input::new().value(sig),
        None => Input::new(),
    };
    let idx = tree.add_child(root, input);

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(W, H, &mut engine, &theme);
    let rect = tree.layout_rect(idx);

    let mut ev = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ev,
    );
    let _ = paint(&tree);
    (tree, rect, ev)
}

fn preedit(
    tree: &mut WidgetTree,
    ev: &mut EventContext,
    text: &str,
    cursor: Option<(usize, usize)>,
) {
    tree.dispatch_event(
        &WidgetEvent::ImePreedit {
            text: text.to_string(),
            cursor,
        },
        ev,
    );
}

/// Count the composition underline rects: thin (≈1px tall), wider than the 2px
/// caret, and narrower than the field's full-width borders.
fn underline_count(ctx: &PaintContext) -> usize {
    ctx.rects
        .iter()
        .filter(|r| (r.height - 1.0).abs() < 0.5 && r.width > 2.0 && r.width < W / 2.0)
        .count()
}

#[test]
fn preedit_is_rendered_into_an_empty_buffer() {
    // With an empty value, a plain field draws no body glyphs (just a
    // placeholder, here empty). A preedit must still show — that is the whole
    // point: the user can see what they're composing before it commits.
    let (mut tree, _rect, mut ev) = focused_input(None);
    preedit(&mut tree, &mut ev, "abc", None);
    let ctx = paint(&tree);
    assert!(
        !ctx.glyphs.is_empty(),
        "the composing preedit must produce visible glyphs even into an empty buffer"
    );
}

#[test]
fn preedit_is_underlined_and_clears() {
    let (mut tree, _rect, mut ev) = focused_input(None);

    preedit(&mut tree, &mut ev, "abc", None);
    let composing = paint(&tree);
    assert!(
        underline_count(&composing) >= 1,
        "an active composition must be underlined, got {} underline rects",
        underline_count(&composing)
    );

    // An empty preedit ends the composition (commit / cancel): the underline
    // must disappear.
    preedit(&mut tree, &mut ev, "", None);
    let cleared = paint(&tree);
    assert_eq!(
        underline_count(&cleared),
        0,
        "the underline must clear once composition ends"
    );
}

#[test]
fn preedit_caret_tracks_the_composition() {
    // The caret sits after the composed text, not at the (untouched) buffer
    // start — so it visually follows what the user is typing. Measured against
    // the composed string, the caret x must be strictly positive.
    let (mut tree, _rect, mut ev) = focused_input(None);
    preedit(&mut tree, &mut ev, "hello", None);
    let ctx = paint(&tree);
    // The caret is the 2px-wide rect; with an empty buffer it would sit at x=0
    // without the fix.
    let caret = ctx
        .rects
        .iter()
        .find(|r| (r.width - 2.0).abs() < 0.01 && r.height > 4.0)
        .expect("a focused field draws a caret");
    let text_x = _rect.origin.x + 8.0;
    assert!(
        caret.x > text_x + 1.0,
        "caret should follow the composition past the field's left padding: \
         caret_x={} text_x={text_x}",
        caret.x
    );
}

#[test]
fn preedit_never_enters_the_value_until_commit() {
    // The composition is display-only. The bound signal must stay empty while
    // composing, and only pick up the text once the IME commits (modeled as the
    // event loop does it: a clearing preedit followed by a CharInput burst).
    let value = Signal::new(String::new());
    let (mut tree, _rect, mut ev) = focused_input(Some(value));

    preedit(&mut tree, &mut ev, "abc", None);
    let _ = paint(&tree);
    assert_eq!(
        value.get_clone(),
        "",
        "an in-progress preedit must not mutate the bound value"
    );

    // Commit: clear the preedit, then splat the committed chars (this is exactly
    // what `translate_ime(Ime::Commit)` produces).
    preedit(&mut tree, &mut ev, "", None);
    for ch in "abc".chars() {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ev);
    }
    assert_eq!(
        value.get_clone(),
        "abc",
        "the committed text must land in the value"
    );
}

#[test]
fn focus_loss_drops_a_half_composed_preedit() {
    // Losing focus mid-composition must discard the preedit so it can't linger.
    // Proven by re-focusing: if the preedit had survived, the underline would
    // reappear; it must not.
    let (mut tree, rect, mut ev) = focused_input(None);
    preedit(&mut tree, &mut ev, "abc", None);
    assert!(underline_count(&paint(&tree)) >= 1, "composition is active");

    tree.dispatch_event(&WidgetEvent::FocusLost, &mut ev);
    // Re-focus via a fresh click.
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ev,
    );
    let refocused = paint(&tree);
    assert_eq!(
        underline_count(&refocused),
        0,
        "a preedit dropped on FocusLost must not survive into the next focus"
    );
}
