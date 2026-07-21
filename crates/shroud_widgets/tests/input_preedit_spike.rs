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

use shroud_core::Color;
use shroud_core::{Point, Rect, Theme};
use shroud_reactive::Signal;
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, Modifiers, MouseButton, WidgetEvent};
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

/// Build a focused single-line `Input` seeded with `text` (via `with_value`),
/// with a warm paint after the focusing click so the caret geometry resolves.
/// Returns the child index so tests can select before composing.
fn seeded_focused_input(text: &str) -> (WidgetTree, usize, EventContext) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));
    let idx = tree.add_child(root, Input::new().with_value(text));

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
    (tree, idx, ev)
}

/// Select the whole buffer (Ctrl+A), resetting modifiers after so a following
/// preedit / CharInput isn't read as a chord.
fn select_all(tree: &mut WidgetTree, ev: &mut EventContext) {
    ev.modifiers = Modifiers::CTRL;
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Character('a'),
        },
        ev,
    );
    ev.modifiers = Modifiers::NONE;
}

#[test]
fn preedit_replaces_the_selection_in_the_display() {
    // Select the whole buffer, then start composing over it. The composition
    // must show *in place of* the selection — the display collapses to just the
    // preedit rather than the selected text sitting next to it until commit.
    // Proven by glyph count: "hello" fully selected + preedit "X" must render
    // one glyph (X), not six (helloX).
    let (mut tree, idx, mut ev) = seeded_focused_input("hello");
    select_all(&mut tree, &mut ev);
    assert!(
        tree.widget_as::<Input>(idx).unwrap().has_selection(),
        "Ctrl+A should have selected the buffer"
    );

    preedit(&mut tree, &mut ev, "X", None);
    let ctx = paint(&tree);
    assert_eq!(
        ctx.glyphs.len(),
        1,
        "composing over a full selection must display only the preedit, \
         got {} glyphs",
        ctx.glyphs.len()
    );
}

#[test]
fn preedit_over_selection_does_not_touch_the_value_until_commit() {
    // The in-place replacement is display-only: while composing over a
    // selection the bound value is still the *original* text, and the real
    // replacement only lands when the IME commits (clearing preedit + a
    // CharInput burst — exactly what `translate_ime(Ime::Commit)` produces).
    let value = Signal::new("hello".to_string());
    let (mut tree, _rect, mut ev) = focused_input(Some(value));
    select_all(&mut tree, &mut ev);

    preedit(&mut tree, &mut ev, "X", None);
    let _ = paint(&tree);
    assert_eq!(
        value.get_clone(),
        "hello",
        "composing over a selection must not mutate the bound value yet"
    );

    // Commit.
    preedit(&mut tree, &mut ev, "", None);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'X' }, &mut ev);
    assert_eq!(
        value.get_clone(),
        "X",
        "committing over a selection must replace it, leaving just the commit"
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

/// Whether a composition caret (the ≈2px-wide, tall fill rect) was drawn.
fn has_caret(ctx: &PaintContext) -> bool {
    ctx.rects
        .iter()
        .any(|r| (r.width - 2.0).abs() < 0.01 && r.height > 4.0)
}

/// Count of target-clause rule rects: the thick (≈2px tall) underline drawn in
/// the text color under the 注目文節 while converting. Distinct from the thin
/// (≈1px), dimmed whole-preedit underline and from the tall, ≈2px-wide caret
/// (the caret is only 2px *wide*, so the `width > 2.0` guard excludes it).
fn target_rule_count(ctx: &PaintContext, text_color: Color) -> usize {
    ctx.rects
        .iter()
        .filter(|r| (r.height - 2.0).abs() < 0.5 && r.width > 2.0 && r.color == text_color)
        .count()
}

#[test]
fn converting_clause_is_underlined_and_the_caret_is_suppressed() {
    // Space-bar 変換: winit reports the target clause as a non-empty byte range
    // (`cs != ce`), not a caret. The clause must be marked with a *thick
    // underline* (the modern inline-composition affordance, matching Chromium /
    // macOS / Win11) instead of collapsing the range to a stray caret at its
    // head, and the thin caret must not be drawn over it. Contrast the two
    // states off the same field so only the reported cursor changes.
    let text_color = Theme::default().colors.on_surface;
    let (mut tree, _rect, mut ev) = focused_input(None);

    // Typing (no clause converted yet): the IME reports a caret. Baseline — a
    // caret is drawn and no target rule is present.
    preedit(&mut tree, &mut ev, "hello", None);
    let typing = paint(&tree);
    assert!(
        has_caret(&typing),
        "while typing, the composition caret must be drawn"
    );
    assert_eq!(
        target_rule_count(&typing, text_color),
        0,
        "typing (no target clause) must not draw a target-clause rule"
    );

    // Convert: the whole preedit becomes the target clause (range 0..len).
    preedit(&mut tree, &mut ev, "hello", Some((0, "hello".len())));
    let converting = paint(&tree);
    assert!(
        target_rule_count(&converting, text_color) >= 1,
        "the converting target clause must be underlined with a thick rule"
    );
    assert!(
        !has_caret(&converting),
        "the stray caret must be suppressed while the clause is underlined"
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
