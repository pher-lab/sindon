//! FW-2 vertical-navigation spike: widget-level proof.
//!
//! Surfaced by dogfooding (`docs/dogfood-log.md` #26/#27/#29): multi-line
//! ArrowUp/Down used to walk `\n`-delimited *paragraphs* by character column,
//! so the caret teleported on soft-wrapped lines and ignored glyph widths.
//! It now walks *visual* rows holding a sticky x, resolved in `paint` (the move
//! needs the text engine). These tests pin:
//!
//! 1. ArrowDown follows a soft wrap *within one paragraph* (the case the old
//!    hard-line model literally could not express),
//! 2. ArrowUp on the first visual row snaps to the buffer start,
//! 3. ArrowDown on the last visual row snaps to the buffer end, and
//! 4. the sticky x column survives passing through a short middle row.

use shroud_core::{Point, Theme};
use shroud_reactive::Signal;
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Input};

// Narrow enough that a long line soft-wraps; tall enough that our short test
// buffers never scroll (so caret y maps straight to a visual row).
const W: f32 = 200.0;
const H: f32 = 300.0;

fn paint(tree: &WidgetTree) -> PaintContext {
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
    ctx
}

/// Focused multi-line `Input` bound to `sig`, with the focusing click resolved
/// by a warm paint so the caret starts at offset 0 (top-left).
fn build(text: &str) -> (WidgetTree, EventContext, Signal<String>) {
    let sig = Signal::new(text.to_string());
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));
    let idx = tree.add_child(root, Input::new().multiline().lines(6).value(sig));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(W, H, &mut engine, &theme);
    let rect = tree.layout_rect(idx);
    let _ = rect;

    let mut ev = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ev,
    );
    let _ = paint(&tree); // resolve the focusing click -> caret at offset 0
    (tree, ev, sig)
}

/// Dispatch a key and paint, so any deferred vertical move resolves — mirroring
/// the real app, which repaints after every event.
fn press(tree: &mut WidgetTree, ev: &mut EventContext, named: NamedKey) -> PaintContext {
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(named),
        },
        ev,
    );
    paint(tree)
}

/// `(x, y)` of the caret: the single 2px-wide, taller-than-tall rect drawn
/// inside the multi-line clip.
fn caret_xy(ctx: &PaintContext) -> (f32, f32) {
    let carets: Vec<_> = ctx
        .rects
        .iter()
        .filter(|r| (r.width - 2.0).abs() < 0.01 && r.height > 4.0 && r.clip_rect.is_some())
        .collect();
    assert_eq!(carets.len(), 1, "expected exactly one caret rect");
    (carets[0].x, carets[0].y)
}

#[test]
fn arrow_down_follows_soft_wrap_within_one_paragraph() {
    // A single long line (no '\n') that wraps into several visual rows. The old
    // hard-line model saw one paragraph, so ArrowDown was a no-op; visual-row
    // nav must move the caret down to the next *wrapped* row.
    let long = "the quick brown fox jumps over the lazy dog and keeps on running";
    let (mut tree, mut ev, sig) = build(long);
    assert!(
        !sig.get_clone().contains('\n'),
        "test buffer must be a single soft-wrapped paragraph"
    );

    let before = paint(&tree);
    let (_x0, y0) = caret_xy(&before);

    let after = press(&mut tree, &mut ev, NamedKey::ArrowDown);
    let (_x1, y1) = caret_xy(&after);

    assert!(
        y1 > y0 + 5.0,
        "ArrowDown must drop the caret to the next wrapped visual row \
         (y0={y0} -> y1={y1}); the old hard-line model would have stayed put"
    );
}

#[test]
fn arrow_up_on_first_row_jumps_to_start() {
    // Caret mid-first-row, ArrowUp must snap to the buffer start (offset 0).
    let (mut tree, mut ev, sig) = build("hello");
    for _ in 0..2 {
        press(&mut tree, &mut ev, NamedKey::ArrowRight); // -> offset 2 (synchronous)
    }
    press(&mut tree, &mut ev, NamedKey::ArrowUp);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'M' }, &mut ev);
    assert_eq!(
        sig.get_clone(),
        "Mhello",
        "ArrowUp on the first visual row must land the caret at offset 0"
    );
}

#[test]
fn arrow_down_on_last_row_jumps_to_end() {
    // Caret at the start of the (only) row, ArrowDown must snap to the buffer
    // end — `offset_at_point` returns the end for a y past the last row.
    let (mut tree, mut ev, sig) = build("hello");
    press(&mut tree, &mut ev, NamedKey::ArrowDown);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'M' }, &mut ev);
    assert_eq!(
        sig.get_clone(),
        "helloM",
        "ArrowDown on the last visual row must land the caret at the buffer end"
    );
}

#[test]
fn sticky_x_survives_a_short_middle_row() {
    // Long / short / long hard lines (none wrap at this width). Park the caret
    // mid-first-row, drop through the short "x" row, and continue down: the
    // sticky x must restore the original column on the long bottom row instead
    // of sticking at the short row's end.
    let (mut tree, mut ev, _sig) = build("aaaaaaaa\nx\nbbbbbbbb");
    for _ in 0..4 {
        press(&mut tree, &mut ev, NamedKey::ArrowRight); // mid first row, col ~4
    }
    let start = paint(&tree);
    let (x_start, _) = caret_xy(&start);

    press(&mut tree, &mut ev, NamedKey::ArrowDown); // into short "x" row
    let mid = press(&mut tree, &mut ev, NamedKey::ArrowDown); // into long bottom row
    let (x_end, _) = caret_xy(&mid);

    // Within ~one glyph width: the sticky x is preserved exactly, but
    // `offset_at_point` snaps to a char boundary whose x differs slightly
    // between the 'a' row and the 'b' row under a proportional font.
    assert!(
        (x_end - x_start).abs() < 8.0,
        "sticky x must restore the column after a short middle row \
         (x_start={x_start} -> x_end={x_end})"
    );
}
