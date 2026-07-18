//! B-1 ⑤ editor-viewport spike: widget-level proof.
//!
//! A multi-line `Input` with `height_full` is a fixed viewport that scrolls its
//! content *internally* instead of overflowing past its border. These tests
//! pin the three load-bearing behaviors:
//!
//! 1. the caret is scrolled back into view after an edit (so a long note stays
//!    editable at the bottom),
//! 2. every glyph is clipped to the field's padding box (so overflow never
//!    crosses the border / bleeds below the field), and
//! 3. the mouse wheel moves the viewport.
//!
//! A fourth test guards that single-line inputs are left completely unchanged
//! (no clip, no scroll).

use shroud_core::{Point, Rect, Theme};
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, MouseButton, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Input};
use std::time::Duration;

const W: f32 = 400.0;
const H: f32 = 120.0;

fn rect_close(a: Rect, b: Rect) -> bool {
    (a.origin.x - b.origin.x).abs() < 0.5
        && (a.origin.y - b.origin.y).abs() < 0.5
        && (a.size.width - b.size.width).abs() < 0.5
        && (a.size.height - b.size.height).abs() < 0.5
}

fn paint(tree: &WidgetTree) -> PaintContext {
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
    ctx
}

/// Build a focused, full-height multi-line `Input`, then type `n_lines` short
/// lines so the content overflows the viewport and the caret ends at the bottom.
/// Returns the tree, the input's node index, and its laid-out rect. The caret
/// hit-test from the focusing click is resolved by a throwaway "warm" paint
/// *before* typing, so the typed caret position (end of buffer) is what the next
/// paint sees.
fn build_and_type(n_lines: usize, scroll_transition: Duration) -> (WidgetTree, usize, Rect) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));
    let input_idx = tree.add_child(
        root,
        Input::new()
            .multiline()
            .height_full()
            .scroll_transition(scroll_transition),
    );

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(W, H, &mut engine, &theme);
    let rect = tree.layout_rect(input_idx);

    let mut ev = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ev,
    );
    // Resolve the focusing click (caret -> start) before typing fills the buffer.
    let _ = paint(&tree);

    for i in 0..n_lines {
        for ch in format!("line {i}").chars() {
            tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ev);
        }
        if i + 1 < n_lines {
            tree.dispatch_event(&WidgetEvent::CharInput { ch: '\n' }, &mut ev);
        }
    }
    (tree, input_idx, rect)
}

/// The single caret is the only 2px-wide rect drawn *inside* the clip (the focus
/// ring's strokes are drawn unclipped, so they have `clip_rect == None`).
fn caret_rect(ctx: &PaintContext) -> (f32, f32) {
    let carets: Vec<_> = ctx
        .rects
        .iter()
        .filter(|r| (r.width - 2.0).abs() < 0.01 && r.clip_rect.is_some())
        .collect();
    assert_eq!(carets.len(), 1, "expected exactly one caret rect");
    (carets[0].y, carets[0].height)
}

#[test]
fn caret_scrolls_into_view_for_long_content() {
    let (tree, _idx, rect) = build_and_type(20, Duration::ZERO);
    let ctx = paint(&tree);

    let top = rect.origin.y + 8.0;
    let bottom = rect.origin.y + rect.size.height - 8.0;
    // Sanity: the content really does overflow, or the test proves nothing.
    assert!(
        bottom - top < 19.2 * 20.0,
        "viewport must be shorter than the 20-line content for this test to matter"
    );

    let (caret_y, caret_h) = caret_rect(&ctx);
    assert!(
        caret_y >= top - 1.0 && caret_y + caret_h <= bottom + 1.0,
        "caret at end of a long note must be scrolled into view: \
         caret_y={caret_y} h={caret_h} viewport=[{top}, {bottom}]"
    );
}

#[test]
fn overflowing_glyphs_are_clipped_to_the_field_box() {
    let (tree, _idx, rect) = build_and_type(20, Duration::ZERO);
    let ctx = paint(&tree);

    let box_clip = Rect::new(
        rect.origin.x,
        rect.origin.y + 8.0,
        rect.size.width,
        rect.size.height - 16.0,
    );
    assert!(
        !ctx.glyphs.is_empty(),
        "the body text should produce glyphs"
    );
    for g in &ctx.glyphs {
        match g.clip_rect {
            Some(c) => assert!(
                rect_close(c, box_clip),
                "glyph clip {c:?} should equal the field's padding box {box_clip:?}"
            ),
            None => panic!("a multi-line glyph must be clipped to the field box"),
        }
    }
}

#[test]
fn mouse_wheel_scrolls_the_viewport() {
    let (mut tree, _idx, rect) = build_and_type(20, Duration::ZERO);

    // First paint: reveal scrolls to the caret (bottom), so the top lines sit
    // above the viewport (very negative y once the offset is applied).
    let before = paint(&tree);
    let min_y_before = before
        .glyphs
        .iter()
        .map(|g| g.y)
        .fold(f32::INFINITY, f32::min);

    // Scroll up hard; the wheel does not set the reveal flag, so it sticks.
    let mut ev = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            delta_x: 0.0,
            delta_y: 10_000.0,
        },
        &mut ev,
    );

    let after = paint(&tree);
    let min_y_after = after
        .glyphs
        .iter()
        .map(|g| g.y)
        .fold(f32::INFINITY, f32::min);
    assert!(
        min_y_after > min_y_before,
        "scrolling up must move content down (min glyph y rises): \
         before={min_y_before} after={min_y_after}"
    );
}

#[test]
fn mouse_wheel_glides_with_a_transition() {
    // FW-7b: with a (long) transition the wheel *eases* the viewport instead of
    // teleporting — on the very next frame the content has barely moved. Paired
    // with `mouse_wheel_scrolls_the_viewport` (instant via ZERO) this pins that
    // wheel input uses `set` (eased), not `snap`.
    let (mut tree, _idx, rect) = build_and_type(20, Duration::from_secs(10));

    // First paint snaps the caret-reveal to the bottom (reveal always snaps).
    let before = paint(&tree);
    let min_y_before = before
        .glyphs
        .iter()
        .map(|g| g.y)
        .fold(f32::INFINITY, f32::min);

    let mut ev = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            delta_x: 0.0,
            delta_y: 10_000.0,
        },
        &mut ev,
    );

    let after = paint(&tree);
    let min_y_after = after
        .glyphs
        .iter()
        .map(|g| g.y)
        .fold(f32::INFINITY, f32::min);
    // Scrolling up moves content down, so min glyph y rises — but over a 10s
    // glide the first frame advances only a few px, nowhere near the ~full
    // viewport jump an instant scroll to the top would produce.
    assert!(
        min_y_after >= min_y_before && min_y_after < min_y_before + 100.0,
        "a 10s-transition wheel scroll should barely move the content on the \
         next frame (eased, not instant): before={min_y_before} after={min_y_after}"
    );
}

#[test]
fn scrolled_glyphs_land_on_the_device_pixel_grid() {
    // A fractional scroll offset used to be pushed to the glyphs raw, shifting
    // the whole column off the physical-pixel grid so scrolled text rendered
    // blurry — and worse the further you scrolled, since a settled non-integer
    // offset never realigns. The visual draw offset is now snapped to the
    // device grid (each line is already shape-snapped relative to the buffer
    // top), so scrolled text stays crisp. At this harness's 1.0 scale the
    // device grid is the integer logical grid. Hit-testing keeps the unsnapped
    // offset, so this guards the drawn geometry only.
    let (mut tree, _idx, rect) = build_and_type(20, Duration::ZERO);
    let _ = paint(&tree); // consume the type-driven caret reveal

    let mut ev = EventContext::new();
    // Pin the viewport to the top, then nudge down by a deliberately fractional
    // 3.5 px. The wheel does not set the reveal flag, so this offset sticks and
    // is what the next paint translates the glyphs by.
    for delta_y in [100_000.0, -3.5] {
        tree.dispatch_event(
            &WidgetEvent::Scroll {
                position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
                delta_x: 0.0,
                delta_y,
            },
            &mut ev,
        );
    }

    let ctx = paint(&tree);
    assert!(
        !ctx.glyphs.is_empty(),
        "the body text should produce glyphs"
    );
    for g in &ctx.glyphs {
        let off_grid = (g.y - g.y.round()).abs();
        assert!(
            off_grid < 0.01,
            "a scrolled glyph must land on the device-pixel grid, but y={} is {off_grid} off",
            g.y
        );
    }
}

#[test]
fn external_caret_jump_scrolls_into_view() {
    // A *programmatic* caret move — the find-replace bar jumping to a match by
    // setting the bound cursor / selection signals — must scroll the match into
    // view, not leave the viewport at the top, even though the field isn't being
    // typed into. This guards the `sync_from_source` reveal extended in B-1 ④.
    use shroud_reactive::Signal;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));

    // 30 short lines so the content overflows the viewport several times over.
    let body: String = (0..30).map(|i| format!("line {i}\n")).collect();
    let value = Signal::new(body.clone());
    let cursor = Signal::new(0usize);
    let selection: Signal<Option<(usize, usize)>> = Signal::new(None);
    let input_idx = tree.add_child(
        root,
        Input::new()
            .multiline()
            .height_full()
            .value(value)
            .cursor_signal(cursor)
            .selection_signal(selection),
    );

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(W, H, &mut engine, &theme);
    let rect = tree.layout_rect(input_idx);

    // Focus the field (so the caret renders and scroll-to-caret runs) and paint
    // once: the caret is at the top, so the content sits at the top.
    let mut ev = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ev,
    );
    let _ = paint(&tree);

    // Jump to a match deep in the buffer via the bound signals, exactly as the
    // find-replace bar does (selection range + caret at its end).
    let lo = body.find("line 28").expect("the body contains line 28");
    let hi = lo + "line 28".len();
    cursor.set(hi);
    selection.set(Some((lo, hi)));

    let ctx = paint(&tree);
    let top = rect.origin.y + 8.0;
    let bottom = rect.origin.y + rect.size.height - 8.0;
    let (caret_y, caret_h) = caret_rect(&ctx);
    assert!(
        caret_y >= top - 1.0 && caret_y + caret_h <= bottom + 1.0,
        "an externally-set caret deep in the buffer must scroll into view: \
         caret_y={caret_y} h={caret_h} viewport=[{top}, {bottom}]"
    );
}

#[test]
fn single_line_input_does_not_clip_or_scroll() {
    // The viewport machinery is multi-line only — a single-line field must push
    // no clip (it intentionally lets text overflow horizontally).
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));
    tree.add_child(root, Input::new().with_value("hello world"));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(W, H, &mut engine, &theme);

    let ctx = paint(&tree);
    assert!(!ctx.glyphs.is_empty(), "the value should produce glyphs");
    for g in &ctx.glyphs {
        assert!(
            g.clip_rect.is_none(),
            "single-line input must not push a clip"
        );
    }
}

// --- Scrollbar indicator -------------------------------------------------
//
// Mirrored from `input.rs` (the consts there are private); the indicator draws
// a `SCROLLBAR_WIDTH`-wide track + thumb at the field's inner right edge.
const SCROLLBAR_WIDTH: f32 = 6.0;
const SCROLLBAR_INSET: f32 = 2.0;
const SCROLLBAR_THUMB_MIN: f32 = 16.0;

/// `(y, height)` of every rect the scrollbar draws: the `SCROLLBAR_WIDTH`-wide
/// fills sitting at the field's inner right edge (`right - 1 - WIDTH - INSET`).
/// Filtering on that x keeps the 2px focus-ring strokes (drawn *outside* the
/// box) and the 1px borders out of the result.
fn scrollbar_bars(ctx: &PaintContext, field: Rect) -> Vec<(f32, f32)> {
    let bar_x = field.right() - 1.0 - SCROLLBAR_WIDTH - SCROLLBAR_INSET;
    ctx.rects
        .iter()
        .filter(|r| (r.width - SCROLLBAR_WIDTH).abs() < 0.01 && (r.x - bar_x).abs() < 0.5)
        .map(|r| (r.y, r.height))
        .collect()
}

#[test]
fn scrollbar_indicator_drawn_when_content_overflows() {
    let (tree, _idx, rect) = build_and_type(20, Duration::ZERO);
    let ctx = paint(&tree);

    let viewport_h = rect.size.height - 16.0;
    let bars = scrollbar_bars(&ctx, rect);
    assert_eq!(
        bars.len(),
        2,
        "overflowing multi-line field draws a track + thumb at the right edge"
    );

    // The track spans the full viewport; the thumb is shorter (content taller
    // than the viewport) but never below the grab-floor, and stays inside the
    // track.
    let track = bars.iter().find(|(_, h)| (h - viewport_h).abs() < 0.5);
    assert!(track.is_some(), "expected a full-height scrollbar track");
    let (thumb_y, thumb_h) = *bars
        .iter()
        .find(|(_, h)| *h < viewport_h - 0.5)
        .expect("expected a scrollbar thumb shorter than the track");
    assert!(
        thumb_h >= SCROLLBAR_THUMB_MIN - 0.01,
        "thumb must respect the minimum grab height: {thumb_h}"
    );
    let top = rect.origin.y + 8.0;
    assert!(
        thumb_y >= top - 0.5 && thumb_y + thumb_h <= top + viewport_h + 0.5,
        "thumb must sit inside the track: y={thumb_y} h={thumb_h} viewport=[{top}, {}]",
        top + viewport_h
    );
}

#[test]
fn no_scrollbar_when_content_fits() {
    // One short line fits well within the full-height viewport, so the field
    // has nothing to scroll and must draw no scrollbar.
    let (tree, _idx, rect) = build_and_type(1, Duration::ZERO);
    let ctx = paint(&tree);
    assert!(
        scrollbar_bars(&ctx, rect).is_empty(),
        "a multi-line field whose content fits draws no scrollbar"
    );
}

#[test]
fn single_line_input_draws_no_scrollbar() {
    // The viewport + scrollbar machinery is multi-line only.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));
    let idx = tree.add_child(root, Input::new().with_value("hello world"));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(W, H, &mut engine, &theme);
    let rect = tree.layout_rect(idx);

    let ctx = paint(&tree);
    assert!(
        scrollbar_bars(&ctx, rect).is_empty(),
        "single-line input must never draw a scrollbar"
    );
}

#[test]
fn text_clears_the_scrollbar_lane() {
    // FW-2 dogfood follow-up (#34): a full row of text used to run under the
    // scrollbar overlay, so a caret at the row end was buried in the bar. The
    // wrap width now reserves the bar's lane, so no glyph is drawn in the bar's
    // column — and since the caret sits at a content boundary (<= the wrap
    // width), it clears the bar too.
    use shroud_reactive::Signal;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));
    // A long unbroken run wraps into full rows; extra lines overflow the
    // viewport so the scrollbar is shown.
    let body = format!("{}\n{}", "W".repeat(120), "tail\n".repeat(12));
    let idx = tree.add_child(
        root,
        Input::new()
            .multiline()
            .height_full()
            .value(Signal::new(body)),
    );

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(W, H, &mut engine, &theme);
    let rect = tree.layout_rect(idx);

    // Focus so the caret renders (the dogfood report was about the caret).
    let mut ev = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ev,
    );
    let ctx = paint(&tree);

    // The test only proves something while the bar is actually up.
    assert!(
        !scrollbar_bars(&ctx, rect).is_empty(),
        "the content must overflow so the scrollbar is visible"
    );

    let track_x = rect.right() - 1.0 - SCROLLBAR_WIDTH - SCROLLBAR_INSET;
    for g in &ctx.glyphs {
        let right = g.x + g.image.width as f32;
        assert!(
            right <= track_x,
            "no glyph may enter the scrollbar lane: right={right} track_x={track_x}"
        );
    }
}
