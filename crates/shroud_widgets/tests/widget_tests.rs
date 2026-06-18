use shroud_core::{Color, Point, Theme};
use shroud_reactive::Signal;
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// ── Widget tree basics ────────────────────────────────────────────

#[test]
fn empty_tree() {
    let tree = WidgetTree::new();
    assert!(tree.is_empty());
    assert_eq!(tree.len(), 0);
}

#[test]
fn add_root_and_children() {
    let mut tree = WidgetTree::new();

    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    assert_eq!(tree.len(), 1);

    let _child1 = tree.add_child(root, Container::row().height(50.0));
    let _child2 = tree.add_child(root, Container::row().height(100.0));
    assert_eq!(tree.len(), 3);
}

#[test]
fn layout_computes_positions() {
    let mut tree = WidgetTree::new();

    let root = tree.set_root(
        Container::column()
            .width(400.0)
            .height(300.0)
            .background(Color::BLACK),
    );

    let c1 = tree.add_child(root, Container::row().width(400.0).height(50.0));
    let c2 = tree.add_child(root, Container::row().width(400.0).height(100.0));

    tree.compute_layout(800.0, 600.0);

    let r1 = tree.layout_rect(c1);
    let r2 = tree.layout_rect(c2);

    assert_eq!(r1.origin.y, 0.0);
    assert_eq!(r1.size.height, 50.0);
    assert_eq!(r2.origin.y, 50.0);
    assert_eq!(r2.size.height, 100.0);
}

// ── Paint ─────────────────────────────────────────────────────────

#[test]
fn paint_produces_rect_commands() {
    let mut tree = WidgetTree::new();

    let root = tree.set_root(
        Container::column()
            .width(400.0)
            .height(300.0)
            .background(Color::rgb(0.1, 0.2, 0.3)),
    );

    let _child = tree.add_child(
        root,
        Container::row()
            .width(200.0)
            .height(100.0)
            .background(Color::rgb(0.5, 0.5, 0.5)),
    );

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert_eq!(ctx.rects.len(), 2, "should have 2 rect draw commands");
}

#[test]
fn text_widget_produces_glyphs() {
    let mut tree = WidgetTree::new();

    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _text = tree.add_child(root, TextWidget::new("Hello").font_size(24.0));

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert!(
        !ctx.glyphs.is_empty(),
        "text widget should produce glyph draw commands"
    );
}

#[test]
fn text_widget_rotation_tags_glyphs_with_a_shared_pivot() {
    // A rotated chevron must stamp every glyph with the same rotation so the
    // renderer spins them as a rigid group. The angle is the requested degrees
    // in radians; the pivot is the widget's layout center.
    use shroud_text::TextEngine;
    let theme = Theme::default();
    let mut engine = TextEngine::new();
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    // Use an ASCII glyph the test font is guaranteed to cover — the rotation
    // path is glyph-agnostic, and a real chevron char may be absent here.
    let _chevron = tree.add_child(root, TextWidget::new(">").font_size(24.0).rotation(90.0));

    // Use the measure-based layout so the text node gets a real height (the
    // pivot is its layout center, so a zero-height box would put pivot.y at 0).
    tree.compute_layout_with_measure(800.0, 600.0, &mut engine, &theme);
    let mut ctx = PaintContext::new(theme.clone());
    tree.paint(&mut ctx);

    assert!(
        !ctx.glyphs.is_empty(),
        "chevron should paint at least one glyph"
    );
    let first = ctx.glyphs[0]
        .rotation
        .expect("rotated text widget should tag glyphs with a rotation");
    assert!(
        (first.angle - 90.0_f32.to_radians()).abs() < 1e-4,
        "angle should be 90° in radians, got {}",
        first.angle
    );
    assert!(
        first.pivot_x > 0.0 && first.pivot_y > 0.0,
        "pivot should land inside the laid-out container, got ({}, {})",
        first.pivot_x,
        first.pivot_y
    );
    // Every glyph shares one pivot so the group rotates rigidly.
    for g in &ctx.glyphs {
        assert_eq!(g.rotation, Some(first));
    }
}

#[test]
fn text_widget_without_rotation_leaves_glyphs_upright() {
    // The default (and an explicit 0°) must take the upright fast path so the
    // renderer skips the per-vertex rotation entirely.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _a = tree.add_child(root, TextWidget::new("Hi").font_size(24.0));
    let _b = tree.add_child(root, TextWidget::new("Yo").font_size(24.0).rotation(0.0));

    tree.compute_layout(800.0, 600.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert!(!ctx.glyphs.is_empty());
    assert!(
        ctx.glyphs.iter().all(|g| g.rotation.is_none()),
        "no rotation (or 0°) should leave glyphs axis-aligned"
    );
}

#[test]
fn text_widget_skips_paint_when_layout_width_is_zero() {
    // Defense-in-depth: when an enclosing flex container squeezes the text
    // widget's layout main-axis to zero (e.g. an inner column inside a row
    // with no flex_basis / grow), cosmic-text treats `Some(0.0)` as
    // unconstrained and emits a natural-width single-line layout that bleeds
    // outside the layout rect. The paint should bail before producing
    // glyphs.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    // height has to be non-zero so the text widget actually paints; we shrink
    // only the width to recreate the overflow scenario.
    let _text = tree.add_child(
        root,
        TextWidget::new("This long string would overflow if painted unwrapped.").font_size(16.0),
    );
    // Pre-flight: a normal layout should produce glyphs.
    tree.compute_layout(400.0, 300.0);
    let mut ctx_baseline = PaintContext::default();
    tree.paint(&mut ctx_baseline);
    assert!(
        !ctx_baseline.glyphs.is_empty(),
        "baseline sanity check: text paints when layout width > 0",
    );

    // Now reproduce the zero-width condition by laying out the root into a
    // viewport whose width forces children to zero. compute_layout sizes the
    // root to its declared 400x300; instead, force the squeeze by setting
    // root width to 0 directly via a fresh tree.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(0.0).height(300.0));
    let _text = tree.add_child(
        root,
        TextWidget::new("This long string would overflow if painted unwrapped.").font_size(16.0),
    );
    tree.compute_layout(400.0, 300.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert!(
        ctx.glyphs.is_empty(),
        "text widget with layout width = 0 should not emit glyphs (got {})",
        ctx.glyphs.len(),
    );
}

// Phase 32 — verify the markdown_demo blockquote shape lays out correctly
// at the event-loop path (compute_layout_with_measure). The body column
// has `flex_basis(0).grow(1.0)`, the body should take row leftover and
// long text inside should wrap.
#[test]
fn blockquote_body_flex_basis_zero_grow_one_wraps_long_text() {
    use shroud_text::TextEngine;
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(720.0).height(600.0).padding(24.0));
    let body_col = tree.add_child(root, Container::column().width_full().gap(12.0));
    let row = tree.add_child(body_col, Container::row().gap(12.0));
    let bar = tree.add_child(row, Container::column().width(4.0).background(Color::WHITE));
    let body = tree.add_child(row, Container::column().gap(8.0).flex_basis(0.0).grow(1.0));
    let text = tree.add_child(
        body,
        TextWidget::new(
            "Knot is a privacy-first notes app whose source of truth is an encrypted \
             SQLCipher database. The spike doesn't open the DB; it keeps a single note \
             encrypted in memory.",
        ),
    );

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(720.0, 600.0, &mut engine, &theme);

    let row_rect = tree.layout_rect(row);
    let body_rect = tree.layout_rect(body);
    let bar_rect = tree.layout_rect(bar);
    let text_rect = tree.layout_rect(text);
    println!(
        "row={:?} bar={:?} body={:?} text={:?}",
        row_rect, bar_rect, body_rect, text_rect
    );

    assert_eq!(bar_rect.size.width, 4.0, "bar keeps explicit width");
    assert!(
        body_rect.size.width > 600.0,
        "body should take row leftover (~656), got {}",
        body_rect.size.width,
    );
    assert!(
        body_rect.size.height > 22.0,
        "long text must wrap (body height > 1 line), got {}",
        body_rect.size.height,
    );
    assert!(
        bar_rect.size.height >= body_rect.size.height,
        "bar height ({}) must reach body height ({}) via cross-axis stretch",
        bar_rect.size.height,
        body_rect.size.height,
    );
}

// Regression guard for the M3 setup-card wrap-overlap bug + the
// `margin_x_auto` centering primitive that fixes it.
//
// Original bug: a centered card built as `width_full().max_width(448)` under
// an `align_center()` parent resolves its width as a *percentage* of the
// parent, so Taffy measures the card's text content at the un-clamped width
// (one line), locks the card height to that, then clamps the width to 448 —
// the text now wraps to two lines at paint time but the box only allocated
// one, and the wrapped tail overlaps the next field. The supported idiom is
// a definite width + `margin_x_auto()`, which avoids the percentage so the
// height is measured at the real (clamped) width.
const HINT: &str = "At least 8 characters. No recovery yet \u{2014} don't forget it, seriously.";

#[test]
fn margin_x_auto_card_allocates_wrapped_text_height_and_centers() {
    use shroud_text::TextEngine;
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .padding(24.0)
            .justify_center(),
    );
    let card = tree.add_child(
        root,
        Container::column()
            .width(448.0)
            .margin_x_auto()
            .padding(32.0)
            .gap(16.0),
    );
    // Card content width = 448 - 2*32 = 384. HINT shapes wider than 384 at
    // the default body font, so it must wrap to two lines.
    let text = tree.add_child(card, TextWidget::new(HINT));
    // Sibling below — stands in for the SecureInput the wrapped tail
    // overlapped on real hardware.
    let sibling = tree.add_child(card, Container::column().height(40.0));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(1080.0, 720.0, &mut engine, &theme);

    let card_rect = tree.layout_rect(card);
    let text_rect = tree.layout_rect(text);
    let sibling_rect = tree.layout_rect(sibling);

    // What paint will actually do: shape at the laid-out width. The layout
    // must allocate at least that much height.
    let painted = engine.shape_text_attrs(
        HINT,
        theme.typography.body.font_size,
        theme.typography.body.line_height,
        Some(text_rect.size.width),
        &Default::default(),
    );
    assert!(
        painted.height > theme.typography.body.line_height,
        "test precondition: HINT must wrap to >1 line at width {}",
        text_rect.size.width,
    );
    assert!(
        text_rect.size.height >= painted.height,
        "layout under-allocated: text box h={} but wrapped paint needs h={}",
        text_rect.size.height,
        painted.height,
    );
    assert!(
        sibling_rect.origin.y >= text_rect.origin.y + text_rect.size.height,
        "sibling (y={}) overlaps the text box (bottom={})",
        sibling_rect.origin.y,
        text_rect.origin.y + text_rect.size.height,
    );

    // margin_x_auto centers the card: left gap ≈ right gap within the
    // 1080-wide viewport (24px root padding on each side).
    let left = card_rect.origin.x;
    let right = 1080.0 - (card_rect.origin.x + card_rect.size.width);
    assert!(
        (left - right).abs() < 1.0,
        "card not centered: left={} right={}",
        left,
        right,
    );
}

// A vertically-centered card holding text + a Button used to render taller
// than its content, leaving dead space below the last child (the knot lock-
// screen gap). Root cause: the card is a non-root flex item that hugs its
// content, and `Button`/`TextWidget` are measured leaves that *also* carried a
// style `min_size`. When that min diverges from the measured size, Taffy over-
// counts the card's content height. The fix drops `min_size` from both leaves
// (their minimum lives in `measure`), so this card must track its laid-out
// children exactly. `SecureInput` — same padding + min height, but no
// `measure` — never triggered it and is the control here.
//
// Critically this is checked across font scales: at scale 1.0 the text widgets
// happened to measure exactly their old hardcoded `min_height(22)`, hiding the
// bug; at "Large" (1.15) the measured line height diverged and each text added
// ~one line of phantom slack (the user's vault was set to Large).
#[test]
fn centered_card_with_button_hugs_content_height() {
    use shroud_text::TextEngine;

    fn card_and_button_at(scale: f32) -> (shroud_core::Rect, shroud_core::Rect, f32) {
        let mut tree = WidgetTree::new();
        let root = tree.set_root(
            Container::column()
                .width_full()
                .height_full()
                .padding(24.0)
                .justify_center(),
        );
        let card = tree.add_child(
            root,
            Container::column()
                .width(448.0)
                .margin_x_auto()
                .padding(32.0)
                .gap(16.0),
        );
        tree.add_child(card, TextWidget::new("Knot").font_size(40.0));
        tree.add_child(card, TextWidget::new("A knot only you can untie."));
        tree.add_child(card, TextWidget::new("Master password:"));
        tree.add_child(card, SecureInput::new().placeholder("pw"));
        tree.add_child(card, TextWidget::new("Locked."));
        let button = tree.add_child(
            card,
            Button::new("Forgot password? Use your recovery key").radius(8.0),
        );

        let mut engine = TextEngine::new();
        let theme = Theme::default().with_font_scale(scale);
        tree.compute_layout_with_measure(1082.0, 753.0, &mut engine, &theme);
        (
            tree.layout_rect(card),
            tree.layout_rect(button),
            theme.typography.body.font_size,
        )
    }

    // Small / Medium / Large font scales (knot's settings map to these).
    for scale in [0.875_f32, 1.0, 1.15] {
        let (card_rect, button_rect, body_font) = card_and_button_at(scale);

        // Dead space between the button's bottom and the card's bottom, beyond
        // the card's 32px bottom padding. Must be ~0 — the card hugs content.
        let slack = (card_rect.origin.y + card_rect.size.height)
            - (button_rect.origin.y + button_rect.size.height)
            - 32.0;
        assert!(
            slack.abs() < 1.0,
            "scale {scale}: card left {slack}px of dead space below the button",
        );

        // The button keeps its minimum visual height (font + 2*8 padding),
        // proving the minimum survived the move from `min_height` to `measure`.
        assert!(
            button_rect.size.height >= body_font + 16.0,
            "scale {scale}: button shorter than its minimum: {}",
            button_rect.size.height,
        );
    }
}

// Same dead-space invariant as `centered_card_with_button_hugs_content_height`,
// for the other two measured leaves that used to carry a style `min_size`:
// `Dropdown`'s trigger and `MenuItem`. Both moved their minimum height out of
// `style().min_size` and into `measure`, so a vertically-centered card must
// now hug each one's laid-out height exactly (no phantom slack below it) while
// the minimum visual height still survives. Checked across font scales because
// the over-count only diverges visibly once the measured height and the old
// `min_height` disagree (the lesson from the button bug).
#[test]
fn centered_card_with_dropdown_and_menu_item_hug_content_height() {
    use shroud_text::TextEngine;

    // Build a centered, content-hugging card whose last child is supplied by
    // `add_last`. Returns (card rect, last-child rect, scaled body font size).
    fn card_and_last_child<F>(
        scale: f32,
        add_last: F,
    ) -> (shroud_core::Rect, shroud_core::Rect, f32)
    where
        F: FnOnce(&mut WidgetTree, usize) -> usize,
    {
        let mut tree = WidgetTree::new();
        let root = tree.set_root(
            Container::column()
                .width_full()
                .height_full()
                .padding(24.0)
                .justify_center(),
        );
        let card = tree.add_child(
            root,
            Container::column()
                .width(448.0)
                .margin_x_auto()
                .padding(32.0)
                .gap(16.0),
        );
        tree.add_child(card, TextWidget::new("Settings"));
        tree.add_child(card, TextWidget::new("Pick a value:"));
        let last = add_last(&mut tree, card);

        let mut engine = TextEngine::new();
        let theme = Theme::default().with_font_scale(scale);
        tree.compute_layout_with_measure(1082.0, 753.0, &mut engine, &theme);
        (
            tree.layout_rect(card),
            tree.layout_rect(last),
            theme.typography.body.font_size,
        )
    }

    for scale in [0.875_f32, 1.0, 1.15] {
        // ── Dropdown trigger ──
        let (card_rect, dd_rect, body_font) = card_and_last_child(scale, |tree, card| {
            let selected = Signal::new(0_usize);
            tree.add_child(
                card,
                Dropdown::new(
                    vec!["Light".into(), "Dark".into(), "System".into()],
                    selected,
                ),
            )
        });
        let slack = (card_rect.origin.y + card_rect.size.height)
            - (dd_rect.origin.y + dd_rect.size.height)
            - 32.0;
        assert!(
            slack.abs() < 1.0,
            "scale {scale}: dropdown card left {slack}px of dead space below the trigger",
        );
        // Trigger keeps at least its old border-box minimum (`font + 16` from
        // the measure floor plus the 16px vertical padding Taffy adds).
        assert!(
            dd_rect.size.height >= body_font + 16.0,
            "scale {scale}: dropdown shorter than its minimum: {}",
            dd_rect.size.height,
        );

        // ── MenuItem row ──
        let (card_rect, mi_rect, _body) = card_and_last_child(scale, |tree, card| {
            tree.add_child(card, MenuItem::new("Delete", |_ctx| {}))
        });
        let slack = (card_rect.origin.y + card_rect.size.height)
            - (mi_rect.origin.y + mi_rect.size.height)
            - 32.0;
        assert!(
            slack.abs() < 1.0,
            "scale {scale}: menu-item card left {slack}px of dead space below the row",
        );
        // Row keeps at least its old `min_height(28)` border box.
        assert!(
            mi_rect.size.height >= 28.0,
            "scale {scale}: menu item shorter than its 28px minimum: {}",
            mi_rect.size.height,
        );
    }
}

// ── Events ────────────────────────────────────────────────────────

#[test]
fn button_click_fires_handler() {
    let clicked = Rc::new(Cell::new(false));
    let clicked2 = clicked.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _btn = tree.add_child(
        root,
        Button::new("Click me").on_click(move |_ctx| {
            clicked2.set(true);
        }),
    );

    tree.compute_layout(800.0, 600.0);

    let mut event_ctx = EventContext::new();

    // Mouse down inside the button area
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(50.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // Mouse up inside the button area
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(50.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(clicked.get(), "button click handler should have fired");
}

#[test]
fn event_outside_widget_is_ignored() {
    let clicked = Rc::new(Cell::new(false));
    let clicked2 = clicked.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(100.0).height(100.0));
    let _btn = tree.add_child(
        root,
        Button::new("Click").on_click(move |_ctx| {
            clicked2.set(true);
        }),
    );

    tree.compute_layout(800.0, 600.0);

    let mut event_ctx = EventContext::new();

    // Click way outside the widget bounds
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(500.0, 500.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(500.0, 500.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(!clicked.get(), "click outside should not fire handler");
}

// ── Container builder ─────────────────────────────────────────────

#[test]
fn container_builder_variants() {
    let _col = Container::column();
    let _row = Container::row();
    let _styled = Container::column()
        .background(Color::WHITE)
        .padding(10.0)
        .gap(5.0)
        .width(200.0)
        .height(100.0)
        .center()
        .grow(1.0);
}

#[test]
fn button_builder() {
    let _btn = Button::new("Test")
        .font_size(20.0)
        .background(Color::rgb(0.3, 0.3, 0.3))
        .text_color(Color::WHITE);
}

// ── Rounded corners ───────────────────────────────────────────────

#[test]
fn fill_rect_emits_zero_radius() {
    use shroud_core::Rect;
    let mut ctx = PaintContext::default();
    ctx.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::WHITE);
    assert_eq!(ctx.rects.len(), 1);
    assert_eq!(
        ctx.rects[0].radius, 0.0,
        "fill_rect must default to sharp corners (radius=0)"
    );
}

#[test]
fn fill_rect_rounded_carries_radius() {
    use shroud_core::Rect;
    let mut ctx = PaintContext::default();
    ctx.fill_rect_rounded(Rect::new(0.0, 0.0, 100.0, 50.0), Color::WHITE, 8.0);
    assert_eq!(ctx.rects.len(), 1);
    assert_eq!(ctx.rects[0].radius, 8.0);
}

#[test]
fn container_radius_propagates_to_drawrect() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(120.0)
            .height(80.0)
            .background(Color::rgb(0.2, 0.4, 0.8))
            .radius(12.0),
    );
    let _ = root;
    tree.compute_layout(400.0, 300.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(ctx.rects.len(), 1);
    assert_eq!(ctx.rects[0].radius, 12.0);
}

#[test]
fn container_default_radius_is_zero() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(50.0)
            .height(50.0)
            .background(Color::WHITE),
    );
    let _ = root;
    tree.compute_layout(400.0, 300.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(ctx.rects.len(), 1);
    assert_eq!(ctx.rects[0].radius, 0.0);
}

#[test]
fn container_negative_radius_clamps_to_zero() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(40.0)
            .height(40.0)
            .background(Color::WHITE)
            .radius(-5.0),
    );
    let _ = root;
    tree.compute_layout(400.0, 300.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(ctx.rects[0].radius, 0.0);
}

#[test]
fn button_radius_propagates_to_drawrect() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(80.0));
    tree.add_child(root, Button::new("Click").radius(6.0));
    tree.compute_layout(400.0, 300.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    // Button paints exactly one bg rect (focus ring is 4 rects, but the
    // button isn't focused here).
    assert_eq!(ctx.rects.len(), 1);
    assert_eq!(ctx.rects[0].radius, 6.0);
}

#[test]
fn button_default_radius_is_zero() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(80.0));
    tree.add_child(root, Button::new("Plain"));
    tree.compute_layout(400.0, 300.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(ctx.rects.len(), 1);
    assert_eq!(ctx.rects[0].radius, 0.0);
}

// ── Theme ────────────────────────────────────────────────────────

#[test]
fn dark_and_light_themes_differ() {
    let dark = Theme::dark();
    let light = Theme::light();
    assert_ne!(dark.colors.background, light.colors.background);
    assert_ne!(dark.colors.on_background, light.colors.on_background);
    assert_ne!(dark.colors.surface, light.colors.surface);
}

#[test]
fn default_theme_is_dark() {
    let def = Theme::default();
    let dark = Theme::dark();
    assert_eq!(def, dark);
}

#[test]
fn theme_typography_scale() {
    let theme = Theme::dark();
    assert!(theme.typography.heading.font_size > theme.typography.body.font_size);
    assert!(theme.typography.body.font_size > theme.typography.label.font_size);
    assert!(theme.typography.label.font_size > theme.typography.small.font_size);
}

#[test]
fn widget_uses_theme_colors_when_no_override() {
    let theme = Theme::dark();
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, Button::new("Themed"));

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::new(theme.clone());
    tree.paint(&mut ctx);

    // Button should paint a rect with the theme's primary color
    assert!(!ctx.rects.is_empty(), "button should produce rect commands");
    let bg_rect = &ctx.rects[0];
    assert_eq!(bg_rect.color, theme.colors.primary);
}

#[test]
fn widget_override_takes_precedence_over_theme() {
    let custom = Color::rgb(1.0, 0.0, 0.0);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, Button::new("Custom").background(custom));

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let bg_rect = &ctx.rects[0];
    assert_eq!(bg_rect.color, custom);
}

#[test]
fn text_widget_uses_theme_defaults() {
    let theme = Theme::dark();
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, TextWidget::new("Hello"));

    tree.compute_layout(800.0, 600.0);

    let mut ctx = PaintContext::new(theme.clone());
    tree.paint(&mut ctx);

    // Text glyphs should use theme's on_background color
    assert!(!ctx.glyphs.is_empty());
    assert_eq!(ctx.glyphs[0].color, theme.colors.on_background);
}

#[test]
fn light_theme_produces_different_colors() {
    let dark = Theme::dark();
    let light = Theme::light();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, TextWidget::new("Test"));
    tree.compute_layout(800.0, 600.0);

    let mut dark_ctx = PaintContext::new(dark.clone());
    tree.paint(&mut dark_ctx);

    let mut light_ctx = PaintContext::new(light.clone());
    tree.paint(&mut light_ctx);

    assert_ne!(
        dark_ctx.glyphs[0].color, light_ctx.glyphs[0].color,
        "dark and light themes should produce different text colors"
    );
}

// ── Hover tracking ────────────────────────────────────────────────

#[test]
fn hover_generates_enter_leave() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(100.0));
    let btn = tree.add_child(root, Button::new("A").background(Color::rgb(1.0, 0.0, 0.0)));
    tree.compute_layout(400.0, 100.0);

    let btn_rect = tree.layout_rect(btn);
    let mut event_ctx = EventContext::new();

    // Move into button bounds
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(btn_rect.origin.x + 5.0, btn_rect.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), Some(btn));

    // Move outside all widgets
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(999.0, 999.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), None);
}

#[test]
fn hover_switches_between_widgets() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let a = tree.add_child(root, Button::new("A"));
    let b = tree.add_child(root, Button::new("B"));
    tree.compute_layout(400.0, 200.0);

    let rect_a = tree.layout_rect(a);
    let rect_b = tree.layout_rect(b);
    let mut event_ctx = EventContext::new();

    // Hover A
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(rect_a.origin.x + 5.0, rect_a.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), Some(a));

    // Hover B
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(rect_b.origin.x + 5.0, rect_b.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), Some(b));
}

// ── Input ─────────────────────────────────────────────────────────

#[test]
fn input_accepts_char_input() {
    let last_value = Rc::new(std::cell::RefCell::new(String::new()));
    let last_value2 = last_value.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let input_idx = tree.add_child(
        root,
        Input::new().on_change(move |text, _ctx| {
            *last_value2.borrow_mut() = text.to_string();
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let input_rect = tree.layout_rect(input_idx);
    let mut event_ctx = EventContext::new();

    // Focus by clicking inside the input
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(input_rect.origin.x + 5.0, input_rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // Type "abc"
    for ch in ['a', 'b', 'c'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    assert_eq!(*last_value.borrow(), "abc");
}

#[test]
fn multiline_input_keeps_pasted_newlines() {
    // Paste arrives as a burst of CharInput events (event_loop dispatch_paste),
    // including the `\n` between lines. A textarea must keep those newlines or a
    // multi-line paste collapses onto one line.
    let last_value = Rc::new(std::cell::RefCell::new(String::new()));
    let last_value2 = last_value.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let input_idx = tree.add_child(
        root,
        Input::new().multiline().on_change(move |text, _ctx| {
            *last_value2.borrow_mut() = text.to_string();
        }),
    );
    tree.compute_layout(400.0, 200.0);

    let rect = tree.layout_rect(input_idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    for ch in ['a', '\n', 'b'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    assert_eq!(*last_value.borrow(), "a\nb");
}

#[test]
fn single_line_input_drops_pasted_newlines() {
    // The complement of the multiline case: a single-line field flattens a
    // multi-line paste, dropping `\n` so a pasted title can't smuggle in a
    // newline.
    let last_value = Rc::new(std::cell::RefCell::new(String::new()));
    let last_value2 = last_value.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let input_idx = tree.add_child(
        root,
        Input::new().on_change(move |text, _ctx| {
            *last_value2.borrow_mut() = text.to_string();
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(input_idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    for ch in ['a', '\n', 'b'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    assert_eq!(*last_value.borrow(), "ab");
}

#[test]
fn input_on_change_fires() {
    let changed = Rc::new(Cell::new(false));
    let changed2 = changed.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(
        root,
        Input::new().on_change(move |_text, _ctx| {
            changed2.set(true);
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();

    // Focus
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // Type a character
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'x' }, &mut event_ctx);

    assert!(changed.get(), "on_change should fire when text changes");
}

#[test]
fn focused_input_does_not_suppress_ime() {
    // Tier 2 regression: only `SecureInput` is allowed to disconnect the
    // OS IME (see `secure_widget_tests::focused_secure_input_suppresses_ime`).
    // Plain `Input` must leave IME alive so CJK users can keep typing
    // composed characters into note bodies, search fields, and similar
    // plain-text inputs. If this test ever fails, Japanese / Chinese /
    // Korean input is silently broken anywhere a regular Input has focus.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let input_idx = tree.add_child(root, Input::new());
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();
    let rect = tree.layout_rect(input_idx);
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    let mut paint_ctx = PaintContext::default();
    tree.paint(&mut paint_ctx);
    assert!(
        !paint_ctx.ime_suppressed(),
        "focused Input must not suppress IME (would break CJK typing)"
    );
}

#[test]
fn input_on_submit_fires() {
    let submitted = Rc::new(Cell::new(false));
    let submitted2 = submitted.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(
        root,
        Input::new().on_submit(move |_text, _ctx| {
            submitted2.set(true);
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();

    // Focus
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // Press Enter
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Enter),
        },
        &mut event_ctx,
    );

    assert!(submitted.get(), "on_submit should fire on Enter");
}

#[test]
fn input_click_outside_unfocuses() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let idx = tree.add_child(root, Input::new());
    tree.compute_layout(400.0, 200.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();

    // Focus via click-in
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'a' }, &mut event_ctx);

    // Click outside the input (above it, well inside the root container)
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.bottom() + 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // After click-outside, char input should be ignored
    let result = tree.dispatch_event(&WidgetEvent::CharInput { ch: 'b' }, &mut event_ctx);
    assert_eq!(
        result,
        EventResult::Ignored,
        "click outside must drop focus"
    );
}

#[test]
fn secure_input_click_outside_unfocuses() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let idx = tree.add_child(root, SecureInput::new().placeholder("pw"));
    tree.compute_layout(400.0, 200.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'x' }, &mut event_ctx);

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.bottom() + 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    let result = tree.dispatch_event(&WidgetEvent::CharInput { ch: 'y' }, &mut event_ctx);
    assert_eq!(
        result,
        EventResult::Ignored,
        "click outside must drop focus"
    );
}

#[test]
fn input_renders_text() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(root, Input::new().with_value("hello"));
    tree.compute_layout(400.0, 100.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert!(!ctx.glyphs.is_empty(), "input should render text glyphs");
}

#[test]
fn input_builder() {
    let _input = Input::new()
        .with_value("initial")
        .placeholder("Enter text")
        .font_size(18.0)
        .background(Color::BLACK)
        .text_color(Color::WHITE);
}

// ── Checkbox ──────────────────────────────────────────────────────

#[test]
fn checkbox_toggle() {
    let toggled = Rc::new(Cell::new(false));
    let toggled2 = toggled.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(
        root,
        Checkbox::new("Accept terms").on_change(move |checked, _ctx| {
            toggled2.set(checked);
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();

    // Click to check
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(10.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    assert!(toggled.get(), "checkbox should be checked after click");

    // Click again to uncheck
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(10.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    assert!(
        !toggled.get(),
        "checkbox should be unchecked after second click"
    );
}

#[test]
fn checkbox_initial_state() {
    let _cb_unchecked = Checkbox::new("Off");
    let _cb_checked = Checkbox::new("On").checked(true);
}

#[test]
fn checkbox_renders_label() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(root, Checkbox::new("Remember me"));
    tree.compute_layout(400.0, 100.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // Should produce rects (checkbox box) and glyphs (label text)
    assert!(!ctx.rects.is_empty(), "checkbox should render box rects");
    assert!(
        !ctx.glyphs.is_empty(),
        "checkbox should render label glyphs"
    );
}

#[test]
fn checkbox_builder() {
    let _cb = Checkbox::new("Test")
        .checked(true)
        .font_size(14.0)
        .check_color(Color::rgb(0.0, 1.0, 0.0))
        .label_color(Color::WHITE);
}

// ── PaintContext clip / offset stack ──────────────────────────────

#[test]
fn paint_context_offset_applied() {
    use shroud_core::Rect as CoreRect;
    let mut ctx = PaintContext::default();
    ctx.push_offset(0.0, -50.0);
    ctx.fill_rect(CoreRect::new(10.0, 100.0, 20.0, 20.0), Color::WHITE);
    ctx.pop_offset();

    assert_eq!(ctx.rects.len(), 1);
    assert_eq!(ctx.rects[0].x, 10.0);
    assert_eq!(ctx.rects[0].y, 50.0, "offset should shift y by -50");
}

#[test]
fn paint_context_clip_intersects() {
    use shroud_core::Rect as CoreRect;
    let mut ctx = PaintContext::default();
    ctx.push_clip(CoreRect::new(0.0, 0.0, 100.0, 100.0));
    ctx.push_clip(CoreRect::new(50.0, 50.0, 100.0, 100.0));

    let clip = ctx.current_clip().unwrap();
    assert_eq!(clip, CoreRect::new(50.0, 50.0, 50.0, 50.0));
}

#[test]
fn paint_context_fill_rect_records_clip() {
    use shroud_core::Rect as CoreRect;
    let mut ctx = PaintContext::default();
    let clip = CoreRect::new(10.0, 10.0, 80.0, 80.0);
    ctx.push_clip(clip);
    ctx.fill_rect(CoreRect::new(0.0, 0.0, 100.0, 100.0), Color::WHITE);

    assert_eq!(ctx.rects[0].clip_rect, Some(clip));
}

#[test]
fn paint_context_offset_pop_restores_state() {
    let mut ctx = PaintContext::default();
    ctx.push_offset(10.0, 20.0);
    ctx.push_offset(5.0, 5.0);
    assert_eq!(ctx.current_offset(), (15.0, 25.0));
    ctx.pop_offset();
    assert_eq!(ctx.current_offset(), (10.0, 20.0));
    ctx.pop_offset();
    assert_eq!(ctx.current_offset(), (0.0, 0.0));
}

// ── ScrollView ────────────────────────────────────────────────────

#[test]
fn scroll_view_builder() {
    let _sv = ScrollView::new()
        .height(300.0)
        .width_full()
        .content_height(1200.0)
        .show_scrollbar(true)
        .padding(8.0)
        .gap(4.0);
}

#[test]
fn scroll_view_initial_offset_is_zero() {
    let sv = ScrollView::new().content_height(1000.0);
    assert_eq!(sv.scroll_y(), 0.0);
    assert_eq!(Widget::scroll_offset(&sv), (0.0, 0.0));
}

#[test]
fn scroll_view_scroll_updates_offset() {
    let mut sv = ScrollView::new().content_height(1000.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0);
    let mut ectx = EventContext::new();

    let event = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: -40.0, // wheel "down" -> scroll down
    };
    assert_eq!(sv.event(&event, layout, &mut ectx), EventResult::Consumed);
    assert_eq!(sv.scroll_y(), 40.0);
}

#[test]
fn scroll_view_clamps_to_max() {
    let mut sv = ScrollView::new().content_height(500.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0);
    // max_scroll = 500 - 300 = 200
    let mut ectx = EventContext::new();
    let huge = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: -9999.0,
    };
    sv.event(&huge, layout, &mut ectx);
    assert_eq!(sv.scroll_y(), 200.0, "should clamp to max");

    let reverse = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: 9999.0,
    };
    sv.event(&reverse, layout, &mut ectx);
    assert_eq!(sv.scroll_y(), 0.0, "should clamp to 0");
}

#[test]
fn scroll_view_ignores_scroll_outside_viewport() {
    let mut sv = ScrollView::new().content_height(1000.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0);
    let mut ectx = EventContext::new();
    let outside = WidgetEvent::Scroll {
        position: Point::new(500.0, 500.0), // not in layout
        delta_x: 0.0,
        delta_y: -40.0,
    };
    assert_eq!(sv.event(&outside, layout, &mut ectx), EventResult::Ignored);
    assert_eq!(sv.scroll_y(), 0.0);
}

#[test]
fn scroll_view_no_scroll_when_content_fits() {
    let mut sv = ScrollView::new().content_height(100.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0); // viewport bigger
    let mut ectx = EventContext::new();
    let event = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: -40.0,
    };
    // Consumed (cursor in layout) but scroll_y stays 0 because max is 0.
    sv.event(&event, layout, &mut ectx);
    assert_eq!(sv.scroll_y(), 0.0);
}

#[test]
fn scroll_view_scroll_offset_matches_scroll_y() {
    let mut sv = ScrollView::new().content_height(1000.0);
    let layout = shroud_core::Rect::new(0.0, 0.0, 200.0, 300.0);
    let mut ectx = EventContext::new();
    let event = WidgetEvent::Scroll {
        position: Point::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: -75.0,
    };
    sv.event(&event, layout, &mut ectx);
    assert_eq!(Widget::scroll_offset(&sv), (0.0, 75.0));
}

#[test]
fn scroll_view_paints_background_and_scrollbar() {
    // When content overflows the viewport, paint should produce background +
    // scrollbar track + thumb (3 rects) once hooks run via the tree.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(1000.0),
    );
    tree.add_child(root, Container::row().width(200.0).height(1000.0));
    tree.compute_layout(400.0, 400.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // ScrollView bg + track + thumb = 3, plus child container = 4
    assert!(
        ctx.rects.len() >= 3,
        "expected >=3 rects, got {}",
        ctx.rects.len()
    );
}

#[test]
fn scroll_view_child_paint_uses_offset_and_clip() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(1000.0),
    );
    let child = tree.add_child(
        root,
        Container::row()
            .width(200.0)
            .height(50.0)
            .background(Color::rgb(1.0, 0.0, 0.0)),
    );
    tree.compute_layout(400.0, 400.0);

    // Manually scroll the ScrollView by sending it an event.
    let mut ectx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(50.0, 50.0),
            delta_x: 0.0,
            delta_y: -100.0,
        },
        &mut ectx,
    );

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // Find the child's rect (red color) and verify it was shifted up by 100.
    let child_layout = tree.layout_rect(child);
    let red = Color::rgb(1.0, 0.0, 0.0);
    let child_rect = ctx
        .rects
        .iter()
        .find(|r| r.color == red)
        .expect("child rect should be painted");
    assert_eq!(child_rect.y, child_layout.origin.y - 100.0);
    assert!(
        child_rect.clip_rect.is_some(),
        "child rect should carry a clip"
    );
}

#[test]
fn scroll_view_hit_test_respects_scroll_offset() {
    // When scrolled, a mouse click at screen y=10 should reach a button whose
    // logical rect sits at y=100+ in content space.
    let clicked = Rc::new(Cell::new(false));
    let clicked_clone = clicked.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(1000.0),
    );
    // Spacer to push the button down to logical y=100.
    tree.add_child(root, Container::column().width(200.0).height(100.0));
    let btn = tree.add_child(
        root,
        Button::new("btn").on_click(move |_ctx| clicked_clone.set(true)),
    );
    tree.compute_layout(400.0, 400.0);

    // Sanity: button's logical rect starts at y=100.
    let btn_rect = tree.layout_rect(btn);
    assert!(btn_rect.origin.y >= 100.0);

    let mut ectx = EventContext::new();
    // Scroll content up so the button is within the viewport at screen y~0.
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(50.0, 50.0),
            delta_x: 0.0,
            delta_y: -100.0,
        },
        &mut ectx,
    );

    // Click at screen (50, 5) — logically (50, 105) after the scroll offset,
    // which should fall inside the button's layout_rect (y >= 100).
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(50.0, 5.0),
            button: MouseButton::Left,
        },
        &mut ectx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(50.0, 5.0),
            button: MouseButton::Left,
        },
        &mut ectx,
    );
    assert!(clicked.get(), "scrolled button should receive click");
}

#[test]
fn scroll_view_reserves_gutter_for_scrollbar() {
    // With `show_scrollbar` on, the ScrollView's content area must stop short
    // of the right edge so children (especially `width_full` ones with
    // unbreakable text) do not draw under the scrollbar track. Concretely,
    // a 200 px wide ScrollView with a `width_full` child should give the
    // child a width strictly less than 200.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(1000.0),
    );
    let child = tree.add_child(root, Container::column().width_full().height(50.0));
    tree.compute_layout(400.0, 400.0);

    let child_rect = tree.layout_rect(child);
    assert!(
        child_rect.size.width < 200.0,
        "child should not span full ScrollView width when scrollbar is reserved (got {})",
        child_rect.size.width
    );
    // Sanity bound: gutter is small (≤ ~16 px), so the child should still
    // claim most of the viewport.
    assert!(child_rect.size.width >= 180.0);
}

#[test]
fn scroll_view_no_gutter_when_scrollbar_hidden() {
    // With `show_scrollbar(false)` the gutter is not reserved; a `width_full`
    // child should reach the full viewport width.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(1000.0)
            .show_scrollbar(false),
    );
    let child = tree.add_child(root, Container::column().width_full().height(50.0));
    tree.compute_layout(400.0, 400.0);

    assert_eq!(tree.layout_rect(child).size.width, 200.0);
}

#[test]
fn scroll_view_gutter_composes_with_user_padding() {
    // Caller-supplied padding still applies on the left, top, bottom; the
    // right side gets `padding + gutter`. A `width_full` child in a 200 px
    // viewport with 10 px user padding sees `200 - 10 (left) - (10 + gutter)
    // (right)` of horizontal space — strictly less than `200 - 20`.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(1000.0)
            .padding(10.0),
    );
    let child = tree.add_child(root, Container::column().width_full().height(50.0));
    tree.compute_layout(400.0, 400.0);

    let child_rect = tree.layout_rect(child);
    assert!(
        child_rect.size.width < 180.0,
        "user padding on left+right plus right-side gutter should make child width < 180, got {}",
        child_rect.size.width
    );
    // Left edge sits at user padding (10 px), unchanged by the gutter.
    let root_rect = tree.layout_rect(root);
    assert_eq!(child_rect.origin.x, root_rect.origin.x + 10.0);
}

#[test]
fn scroll_view_auto_content_height_sums_children() {
    // Without an explicit `content_height`, ScrollView should pick up the
    // total height of its laid-out children after `compute_layout`.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(ScrollView::new().width(200.0).height(300.0));
    tree.add_child(root, Container::column().width(200.0).height(500.0));
    tree.add_child(root, Container::column().width(200.0).height(400.0));
    tree.compute_layout(400.0, 400.0);

    // 500 + 400 = 900 of content; viewport is 300 → 600 px of scroll.
    let sv = tree.widget_as::<ScrollView>(root).expect("ScrollView root");
    assert_eq!(sv.max_scroll_y(300.0), 600.0);
}

#[test]
fn scroll_view_explicit_content_height_overrides_auto() {
    // An explicit `.content_height(h)` pins the value even when children
    // measure differently — virtualized lists rely on this.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        ScrollView::new()
            .width(200.0)
            .height(300.0)
            .content_height(2000.0),
    );
    // Children measuring 100 px total: auto would give 100, explicit wins.
    tree.add_child(root, Container::column().width(200.0).height(100.0));
    tree.compute_layout(400.0, 400.0);

    let sv = tree.widget_as::<ScrollView>(root).expect("ScrollView root");
    // max_scroll = 2000 - 300 = 1700 (the explicit value, not 0 from auto).
    assert_eq!(sv.max_scroll_y(300.0), 1700.0);
}

#[test]
fn scroll_view_auto_content_height_includes_bottom_padding() {
    // Top + bottom padding should both appear in the scrollable extent so a
    // fully scrolled viewport leaves the bottom padding flush with the last
    // child (matching what a static `content_height` caller used to hand-tune).
    let mut tree = WidgetTree::new();
    let root = tree.set_root(ScrollView::new().width(200.0).height(300.0).padding(20.0));
    tree.add_child(root, Container::column().width(50.0).height(500.0));
    tree.compute_layout(400.0, 400.0);

    let sv = tree.widget_as::<ScrollView>(root).expect("ScrollView root");
    // Child y starts at top-padding (20), height 500 → bottom = 520.
    // Auto height adds bottom padding (20) → 540. max_scroll = 540 - 300 = 240.
    assert_eq!(sv.max_scroll_y(300.0), 240.0);
}

#[test]
fn scroll_view_auto_content_height_recomputes_on_relayout() {
    // Re-running compute_layout after adding a child should grow the
    // measured content_height — the value tracks the current tree, not a
    // one-shot snapshot.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(ScrollView::new().width(200.0).height(300.0));
    tree.add_child(root, Container::column().width(200.0).height(400.0));
    tree.compute_layout(400.0, 400.0);

    {
        let sv = tree.widget_as::<ScrollView>(root).expect("ScrollView root");
        assert_eq!(sv.max_scroll_y(300.0), 100.0);
    }

    tree.add_child(root, Container::column().width(200.0).height(600.0));
    tree.compute_layout(400.0, 400.0);

    let sv = tree.widget_as::<ScrollView>(root).expect("ScrollView root");
    // 400 + 600 = 1000 content; viewport 300 → 700 scroll.
    assert_eq!(sv.max_scroll_y(300.0), 700.0);
}

#[test]
fn scroll_view_auto_content_height_skips_invisible_children() {
    let visible_sig = Signal::new(true);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(ScrollView::new().width(200.0).height(300.0));
    tree.add_child(root, Container::column().width(200.0).height(400.0));
    tree.add_child(
        root,
        Container::column()
            .width(200.0)
            .height(500.0)
            .visible(visible_sig),
    );
    tree.compute_layout(400.0, 400.0);

    {
        let sv = tree.widget_as::<ScrollView>(root).expect("ScrollView root");
        // 400 + 500 = 900 visible content.
        assert_eq!(sv.max_scroll_y(300.0), 600.0);
    }

    visible_sig.set(false);
    tree.compute_layout(400.0, 400.0);

    let sv = tree.widget_as::<ScrollView>(root).expect("ScrollView root");
    // Hidden child drops out → only 400 of content, viewport 300, scroll 100.
    assert_eq!(sv.max_scroll_y(300.0), 100.0);
}

#[test]
fn scroll_view_grow_in_fixed_height_parent_clamps_to_viewport() {
    // The sidebar pattern: a `grow(1.0)` ScrollView is a *direct* child of a
    // fixed-height column, with tall content. Before the ScrollView declared
    // `overflow: hidden`, its automatic minimum was its content height, so a
    // `grow` item ballooned to the content (overflowing the parent) and there
    // was nothing to scroll. It must instead clamp to the leftover space and
    // scroll its content. No wrapper fix is needed here because the parent's
    // height is definite (unlike the preview, whose grow wrapper also needs
    // `overflow_hidden`).
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(240.0).height(400.0));
    // A fixed header eats some of the column, the scroll view takes the rest.
    tree.add_child(root, Container::row().width_full().height(40.0));
    let scroll = tree.add_child(root, ScrollView::new().width_full().grow(1.0));
    let list = tree.add_child(scroll, Container::column().width_full());
    // Populate a list taller than the viewport (20 * 50 = 1000px).
    for _ in 0..20 {
        tree.add_child(list, Container::row().width_full().height(50.0));
    }
    let mut engine = shroud_text::TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(240.0, 400.0, &mut engine, &theme);

    // Viewport clamps to the column's leftover (~360), not the 1000px list.
    let viewport_h = tree.layout_rect(scroll).size.height;
    assert!(
        viewport_h < 400.0,
        "scroll viewport should clamp below the parent height, got {viewport_h}"
    );
    let sv = tree
        .widget_as::<ScrollView>(scroll)
        .expect("ScrollView node");
    assert!(
        sv.max_scroll_y(viewport_h) > 0.0,
        "tall list must be scrollable (viewport {viewport_h}, max_scroll {})",
        sv.max_scroll_y(viewport_h)
    );
}

// Build a `w x h` solid-red PNG for image layout tests.
fn solid_png(w: u32, h: u32) -> Vec<u8> {
    let mut img = image::RgbaImage::new(w, h);
    for px in img.pixels_mut() {
        *px = image::Rgba([255, 0, 0, 255]);
    }
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

#[test]
fn scroll_view_auto_content_height_includes_nested_tall_image() {
    // Mirrors knot's markdown preview: ScrollView → Container::column →
    // [tall Image (width-only, aspect-derived height), text]. The image is a
    // *grandchild* of the ScrollView, so it does not get the `shrink(0)`
    // override the tree applies to direct children — it must keep its full
    // aspect-derived height anyway so the auto content_height (and thus the
    // scrollable extent) reaches past the image to the content below it.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(ScrollView::new().width(200.0).height(300.0));
    let column = tree.add_child(root, Container::column().width_full().gap(12.0));
    // 10x40 decoded, pinned to width 480 (wider than the 200px viewport) →
    // aspect-derived height 1920. Mirrors knot capping a large image to
    // MAX_PREVIEW_IMAGE_WIDTH while the preview pane is narrower than the cap.
    let png = solid_png(10, 40);
    let image_idx = tree.add_child(column, Image::from_bytes(&png).unwrap().width(480.0));
    let text_idx = tree.add_child(column, TextWidget::new("below the image"));
    let mut engine = shroud_text::TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 400.0, &mut engine, &theme);

    // The image must keep its tall aspect-derived height, not be squashed to
    // fit the 300px viewport.
    let img_rect = tree.layout_rect(image_idx);
    assert!(
        (img_rect.size.height - 1920.0).abs() < 1.0,
        "image should keep aspect-derived height 1920, got {}",
        img_rect.size.height
    );

    // The text block must sit below the full image height, not on top of it.
    let text_rect = tree.layout_rect(text_idx);
    assert!(
        text_rect.origin.y >= img_rect.origin.y + img_rect.size.height,
        "text (y={}) should start below the image bottom (y={})",
        text_rect.origin.y,
        img_rect.origin.y + img_rect.size.height
    );

    // Auto content_height must reach past the image so you can scroll down to
    // the text. Viewport is 300; content is at least the image's 720.
    let sv = tree.widget_as::<ScrollView>(root).expect("ScrollView root");
    assert!(
        sv.max_scroll_y(300.0) >= 1620.0,
        "should be able to scroll past the tall image, max_scroll_y={}",
        sv.max_scroll_y(300.0)
    );

    // A wheel event with the cursor over the (overflowing) image must reach the
    // ScrollView and move the offset — the image does not swallow the scroll.
    let mut ectx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(50.0, 150.0),
            delta_x: 0.0,
            delta_y: -120.0,
        },
        &mut ectx,
    );
    let sv = tree.widget_as::<ScrollView>(root).expect("ScrollView root");
    assert_eq!(
        sv.scroll_y(),
        120.0,
        "wheel over the tall image should scroll the viewport"
    );
}

#[test]
fn knot_preview_full_pane_scrolls_past_tall_image() {
    // Faithful reproduction of knot's editor pane: a root row holding a
    // fixed-width sidebar and a `flex: 1 1 0` pane; the pane stacks a header
    // and two grow(1) siblings (editor hidden via display:none, preview
    // visible); the preview's grow(1) ScrollView holds a width_full content
    // column whose grandchildren are a tall capped image + trailing text.
    //
    // The point is the full grow/flex-basis/height_full chain above the
    // ScrollView, which my simpler repro doesn't exercise: the ScrollView gets
    // a *definite* viewport height from grow, but its content column must still
    // be content-sized (unshrunk) so the auto content_height reaches past the
    // image.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width_full().height_full());
    // Fixed sidebar.
    tree.add_child(root, Container::column().width(240.0).height_full());
    // Pane: flex 1 1 0, fills height, padded.
    let pane = tree.add_child(
        root,
        Container::column()
            .flex_basis(0.0)
            .grow(1.0)
            .height_full()
            .padding(24.0)
            .gap(12.0),
    );
    // Header (fixed-ish height via a child).
    let header = tree.add_child(pane, Container::row().gap(12.0));
    tree.add_child(header, TextWidget::new("Editing: note"));
    // Editor area sibling — hidden while previewing (display:none).
    tree.add_child(
        pane,
        Container::column()
            .width_full()
            .grow(1.0)
            .visible(Signal::new(false)),
    );
    // Preview area — visible.
    let preview_area = tree.add_child(
        pane,
        Container::column()
            .width_full()
            .grow(1.0)
            .overflow_hidden()
            .visible(Signal::new(true)),
    );
    let preview_scroll = tree.add_child(preview_area, ScrollView::new().width_full().grow(1.0));
    let preview_content =
        tree.add_child(preview_scroll, Container::column().width_full().gap(12.0));
    // Image wider than the pane (480 cap) over a narrow root → overflows
    // horizontally; 10x40 decoded → aspect-derived height 1920.
    let png = solid_png(10, 40);
    let image_idx = tree.add_child(
        preview_content,
        Image::from_bytes(&png).unwrap().width(480.0),
    );
    let text_idx = tree.add_child(preview_content, TextWidget::new("below the image"));

    let mut engine = shroud_text::TextEngine::new();
    let theme = Theme::default();
    // Root viewport: 700 wide (pane ~ 700-240-48 = 412 < 480 image), 600 tall.
    tree.compute_layout_with_measure(700.0, 600.0, &mut engine, &theme);

    // Image keeps full aspect-derived height despite the grow-chain above it.
    let img_rect = tree.layout_rect(image_idx);
    assert!(
        (img_rect.size.height - 1920.0).abs() < 1.0,
        "image should keep aspect-derived height 1920, got {}",
        img_rect.size.height
    );
    // Trailing text sits below the image.
    let text_rect = tree.layout_rect(text_idx);
    assert!(
        text_rect.origin.y >= img_rect.origin.y + img_rect.size.height,
        "text (y={}) should start below the image bottom (y={})",
        text_rect.origin.y,
        img_rect.origin.y + img_rect.size.height
    );
    // The preview scroll's content extent must reach past the image. Viewport
    // is well under the image's 1920 height.
    let sv = tree
        .widget_as::<ScrollView>(preview_scroll)
        .expect("preview ScrollView");
    let viewport_h = tree.layout_rect(preview_scroll).size.height;
    assert!(
        sv.max_scroll_y(viewport_h) > 0.0,
        "preview must be scrollable past the tall image (viewport {viewport_h}, \
         max_scroll {})",
        sv.max_scroll_y(viewport_h)
    );

    // A wheel event over the image scrolls the preview.
    let mut ectx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(200.0, 200.0),
            delta_x: 0.0,
            delta_y: -90.0,
        },
        &mut ectx,
    );
    let sv = tree
        .widget_as::<ScrollView>(preview_scroll)
        .expect("preview ScrollView");
    assert_eq!(
        sv.scroll_y(),
        90.0,
        "wheel over the preview image should scroll"
    );
}

#[test]
fn knot_preview_responsive_image_fits_column_without_phantom_height() {
    // Regression: in knot's markdown preview a too-wide embedded image used to
    // be pinned with a fixed `.width(cap)`. When the preview column is narrower
    // than that cap, the pinned image overflows horizontally AND inflates the
    // content column's reported *height* by a few hundred px (a Taffy
    // min-content-width interaction that only fires under the pane's
    // `flex_basis(0)`), leaving phantom empty scroll space below the content.
    //
    // `Image::max_width` fixes this: it scales the image down to the available
    // column width with an aspect-derived height, so the column height is
    // exactly image_height + gap + text_height — no phantom inflation. Same
    // full pane chain as `knot_preview_full_pane_scrolls_past_tall_image`.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width_full().height_full());
    tree.add_child(root, Container::column().width(240.0).height_full());
    let pane = tree.add_child(
        root,
        Container::column()
            .flex_basis(0.0)
            .grow(1.0)
            .height_full()
            .padding(24.0)
            .gap(12.0),
    );
    let header = tree.add_child(pane, Container::row().gap(12.0));
    tree.add_child(header, TextWidget::new("Editing: note"));
    tree.add_child(
        pane,
        Container::column()
            .width_full()
            .grow(1.0)
            .visible(Signal::new(false)),
    );
    let preview_area = tree.add_child(
        pane,
        Container::column()
            .width_full()
            .grow(1.0)
            .overflow_hidden()
            .visible(Signal::new(true)),
    );
    let preview_scroll = tree.add_child(preview_area, ScrollView::new().width_full().grow(1.0));
    let content_gap = 12.0;
    let preview_content = tree.add_child(
        preview_scroll,
        Container::column().width_full().gap(content_gap),
    );
    // Intrinsic 480x1920 (aspect 0.25): the 480 width cap exceeds the ~400px
    // preview column, so a fixed `.width(480)` pin would overflow it and
    // inflate the column. Responsive `.max_width` scales it to the column.
    let png = solid_png(480, 1920);
    let image_idx = tree.add_child(
        preview_content,
        Image::from_bytes(&png).unwrap().max_width(480.0),
    );
    let text_idx = tree.add_child(preview_content, TextWidget::new("below the image"));

    let mut engine = shroud_text::TextEngine::new();
    let theme = Theme::default();
    // Pane content width ≈ 700 - 240 - 48(padding) = 412, narrower than 480.
    tree.compute_layout_with_measure(700.0, 600.0, &mut engine, &theme);

    let img_rect = tree.layout_rect(image_idx);
    let text_rect = tree.layout_rect(text_idx);
    let col_rect = tree.layout_rect(preview_content);

    // The image fits within the column width — no horizontal overflow.
    assert!(
        img_rect.size.width <= col_rect.size.width + 0.5,
        "responsive image width {} should fit the column width {}",
        img_rect.size.width,
        col_rect.size.width
    );
    // Its height tracks the resolved width via the 0.25 aspect ratio (height =
    // width / 0.25 = 4x), so it shrank with the column instead of staying at
    // the cap's 1920.
    assert!(
        (img_rect.size.height - img_rect.size.width * 4.0).abs() < 1.0,
        "image height {} should be 4x its width {} (aspect-preserved)",
        img_rect.size.height,
        img_rect.size.width
    );
    assert!(
        img_rect.size.height < 1920.0,
        "image should have scaled down below the 480-cap height of 1920, got {}",
        img_rect.size.height
    );
    // The column height is exactly image + gap + text — the phantom is gone.
    let expected = img_rect.size.height + content_gap + text_rect.size.height;
    assert!(
        (col_rect.size.height - expected).abs() < 1.0,
        "column height {} should equal image({}) + gap({}) + text({}) = {} \
         (phantom inflation = {})",
        col_rect.size.height,
        img_rect.size.height,
        content_gap,
        text_rect.size.height,
        expected,
        col_rect.size.height - expected
    );
}

// ── Reactive TextWidget ───────────────────────────────────────────

#[test]
fn reactive_text_reflects_signal_updates() {
    let count = Signal::new(0i32);
    let widget = TextWidget::reactive(move || format!("Count: {}", count.get()));

    assert_eq!(widget.text(), "Count: 0");

    count.set(7);
    assert_eq!(widget.text(), "Count: 7");

    count.update(|n| *n += 1);
    assert_eq!(widget.text(), "Count: 8");
}

#[test]
fn reactive_text_paints_current_value() {
    let count = Signal::new(0i32);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(
        root,
        TextWidget::reactive(move || format!("{}", count.get())).font_size(24.0),
    );
    tree.compute_layout(800.0, 600.0);

    // Paint with count = 0 ("0" → 1 glyph).
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    let glyphs_zero = ctx.glyphs.len();
    assert!(glyphs_zero >= 1, "expected at least 1 glyph for '0'");

    // Update the signal. Next paint should see the new value and produce
    // more glyphs for the longer string.
    count.set(123456);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    let glyphs_six = ctx2.glyphs.len();

    assert!(
        glyphs_six > glyphs_zero,
        "reactive text should repaint with the current signal value \
         (got {} glyphs for '0', {} for '123456')",
        glyphs_zero,
        glyphs_six,
    );
}

#[test]
fn text_color_accepts_literal_via_reactive() {
    // Literal `Color` should still work — proves the `impl<T> From<T>`
    // path for `Reactive<T>` is wired through the widget builder and the
    // Phase 14a migration didn't regress static callers.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(50.0));
    tree.add_child(root, TextWidget::new("X").font_size(24.0).color(red));
    tree.compute_layout(200.0, 50.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert!(!ctx.glyphs.is_empty(), "expected glyphs for 'X'");
    for g in &ctx.glyphs {
        assert_eq!(g.color, red, "static color should flow through Reactive");
    }
}

#[test]
fn text_color_tracks_signal_updates() {
    // Signal<Color> → Reactive<Color> via the `From<Signal<T>>` impl.
    // After flipping the signal, the next paint must emit glyphs with the
    // new color — this is the whole point of making `color` reactive.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let green = Color::rgb(0.0, 1.0, 0.0);
    let color = Signal::new(red);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(50.0));
    tree.add_child(root, TextWidget::new("Y").font_size(24.0).color(color));
    tree.compute_layout(200.0, 50.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    assert!(!ctx1.glyphs.is_empty());
    for g in &ctx1.glyphs {
        assert_eq!(g.color, red);
    }

    color.set(green);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(!ctx2.glyphs.is_empty());
    for g in &ctx2.glyphs {
        assert_eq!(
            g.color, green,
            "reactive color should reflect the updated Signal on next paint"
        );
    }
}

#[test]
fn text_color_accepts_reactive_derive_closure() {
    // `Reactive::derive` — the escape hatch when neither literal nor
    // `Signal`/`Memo` conversions apply. Here we combine two signals
    // (enabled toggle + theme color) into a single derived color.
    use shroud_reactive::Reactive;

    let enabled = Signal::new(true);
    let on_color = Color::rgb(0.2, 0.7, 0.2);
    let off_color = Color::rgb(0.5, 0.5, 0.5);
    let derived = Reactive::derive(move || if enabled.get() { on_color } else { off_color });

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(50.0));
    tree.add_child(root, TextWidget::new("Z").font_size(24.0).color(derived));
    tree.compute_layout(200.0, 50.0);

    let mut ctx_on = PaintContext::default();
    tree.paint(&mut ctx_on);
    for g in &ctx_on.glyphs {
        assert_eq!(g.color, on_color);
    }

    enabled.set(false);
    let mut ctx_off = PaintContext::default();
    tree.paint(&mut ctx_off);
    for g in &ctx_off.glyphs {
        assert_eq!(g.color, off_color);
    }
}

// ── Measure (Widget::measure + compute_layout_with_measure) ──────

#[test]
fn text_widget_measures_to_shaped_size() {
    // TextWidget::measure should return the shaped size of its text so Taffy
    // can lay it out without a fixed-width wrapper.
    use shroud_text::TextEngine;
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let widget = TextWidget::new("Hello").font_size(16.0);

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let size = <TextWidget as Widget>::measure(&widget, None, &mut ctx)
        .expect("TextWidget should report a measured size");
    assert!(
        size.width > 0.0,
        "shaped width should be > 0, got {}",
        size.width
    );
    assert!(
        size.height > 0.0,
        "shaped height should be > 0, got {}",
        size.height
    );
}

#[test]
fn text_widget_monospace_normalizes_iii_and_mmm_widths() {
    // End-to-end: `.monospace()` on the widget builder must reach
    // `shape_text_attrs` in the engine and select a monospace face. The
    // ratio test mirrors `shroud_text`'s `monospace_family_makes_iii_and_
    // mmm_equal_width` but exercises the widget → MeasureContext → engine
    // path, which is what regressed in past phases when a refactor of
    // `measure` forgot to plumb attrs.
    use shroud_text::TextEngine;
    let mut engine = TextEngine::new();
    let theme = Theme::default();

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let iii_mono = <TextWidget as Widget>::measure(
        &TextWidget::new("iii").font_size(16.0).monospace(),
        None,
        &mut ctx,
    )
    .unwrap();
    let mmm_mono = <TextWidget as Widget>::measure(
        &TextWidget::new("mmm").font_size(16.0).monospace(),
        None,
        &mut ctx,
    )
    .unwrap();

    assert!(iii_mono.width > 0.0 && mmm_mono.width > 0.0);
    let ratio = iii_mono.width / mmm_mono.width;
    assert!(
        ratio > 0.95 && ratio < 1.05,
        "monospace widget iii / mmm width ratio = {} (expected ~1.0)",
        ratio
    );

    // Contrast: without `.monospace()` the same chars shape proportionally.
    let iii_prop =
        <TextWidget as Widget>::measure(&TextWidget::new("iii").font_size(16.0), None, &mut ctx)
            .unwrap();
    let mmm_prop =
        <TextWidget as Widget>::measure(&TextWidget::new("mmm").font_size(16.0), None, &mut ctx)
            .unwrap();
    assert!(
        iii_prop.width < mmm_prop.width * 0.6,
        "proportional widget iii ({}) should be much narrower than mmm ({})",
        iii_prop.width,
        mmm_prop.width
    );
}

#[test]
fn empty_text_widget_measures_to_zero() {
    use shroud_text::TextEngine;
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let widget = TextWidget::new("");

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let size = <TextWidget as Widget>::measure(&widget, None, &mut ctx)
        .expect("empty TextWidget should still report Some(ZERO)");
    assert_eq!(size.width, 0.0);
    assert_eq!(size.height, 0.0);
}

#[test]
fn text_widget_rich_total_width_matches_plain_concat() {
    // End-to-end mirror of `shape_rich_two_default_spans_total_width_matches_concat`
    // at the widget layer. If TextWidget::rich plumbs the wrong attrs (or
    // measure forgets to route through shape_rich) this drifts.
    use shroud_text::{TextEngine, TextSpan};
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let mut ctx = MeasureContext::new(&mut engine, &theme);

    let plain = <TextWidget as Widget>::measure(
        &TextWidget::new("Hello world").font_size(16.0),
        None,
        &mut ctx,
    )
    .unwrap();
    let rich = <TextWidget as Widget>::measure(
        &TextWidget::rich(vec![TextSpan::new("Hello "), TextSpan::new("world")]).font_size(16.0),
        None,
        &mut ctx,
    )
    .unwrap();

    assert!(
        (plain.width - rich.width).abs() < 1.0,
        "plain measure width {} vs rich measure width {}",
        plain.width,
        rich.width
    );
    assert_eq!(
        plain.height, rich.height,
        "single-line rich should match plain height"
    );
}

#[test]
fn text_widget_rich_empty_spans_measure_to_zero() {
    use shroud_text::{TextEngine, TextSpan};
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let mut ctx = MeasureContext::new(&mut engine, &theme);

    // Pure-empty spans must short-circuit to ZERO, same as TextWidget::new("").
    let size = <TextWidget as Widget>::measure(
        &TextWidget::rich(vec![TextSpan::new(""), TextSpan::new("")]),
        None,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(size.width, 0.0);
    assert_eq!(size.height, 0.0);
}

#[test]
fn text_widget_rich_wraps_when_narrow_available_width() {
    // Gap #3 raison d'être at widget level: a single bold span that
    // overflows the available width must wrap and the widget reports a
    // taller size. Without `shape_rich` the only wrap point would be
    // between widgets (gap #1's flex_wrap), and a single span would not
    // wrap at all.
    use shroud_text::{TextEngine, TextSpan};
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let mut ctx = MeasureContext::new(&mut engine, &theme);

    let natural = <TextWidget as Widget>::measure(
        &TextWidget::rich(vec![
            TextSpan::new("regular text and "),
            TextSpan::new("a moderately long bold phrase here").bold(),
        ])
        .font_size(16.0),
        None,
        &mut ctx,
    )
    .unwrap();
    let narrow = <TextWidget as Widget>::measure(
        &TextWidget::rich(vec![
            TextSpan::new("regular text and "),
            TextSpan::new("a moderately long bold phrase here").bold(),
        ])
        .font_size(16.0),
        Some(80.0),
        &mut ctx,
    )
    .unwrap();
    assert!(
        narrow.height > natural.height,
        "narrow available_width should force wrap; natural h={} narrow h={}",
        natural.height,
        narrow.height
    );
    assert!(
        narrow.width <= 80.0 + 1.0,
        "wrapped width {} must respect available_width 80",
        narrow.width
    );
}

#[test]
fn button_measures_to_label_size() {
    use shroud_text::TextEngine;
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let button = Button::new("OK").font_size(16.0);

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let size = <Button as Widget>::measure(&button, None, &mut ctx)
        .expect("Button should report a measured size");
    assert!(size.width > 0.0, "Button content width should be > 0");
    assert!(
        size.height >= 16.0,
        "Button content height should be at least font_size"
    );
}

#[test]
fn container_has_no_intrinsic_measure() {
    // Non-leaf layout widgets should return None so Taffy sizes them by flex.
    use shroud_text::TextEngine;
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let container = Container::column();

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let size = <Container as Widget>::measure(&container, None, &mut ctx);
    assert!(
        size.is_none(),
        "Container should not report an intrinsic size"
    );
}

#[test]
fn measured_layout_gives_text_widget_nonzero_width_in_center_column() {
    // This is the regression guard for the `.center()` collapse bug: without
    // `Widget::measure`, a TextWidget inside `Container::column().center()`
    // would have width 0 because align_items:Center doesn't stretch children.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).center());
    let text_idx = tree.add_child(root, TextWidget::new("Hello World").font_size(24.0));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);

    let rect = tree.layout_rect(text_idx);
    assert!(
        rect.size.width > 0.0,
        "TextWidget inside .center() should have nonzero width via measure, \
         got width={}",
        rect.size.width,
    );
    assert!(rect.size.height > 0.0, "and nonzero height");
}

#[test]
fn text_measure_width_survives_integer_rounding() {
    // Regression guard for the counter "Count: 0 wraps" bug. The shaped
    // natural width of "Count: 0" at 32px is fractional (~118.58). Taffy
    // pixel-rounds layout output, so if measure returned the raw fractional
    // width, the widget's final layout_rect.width could be one pixel short
    // of natural. Then paint's `shape_text(max_width=layout_width)` would
    // wrap at the last space. Measure must round up (ceil) so the allocated
    // width is always ≥ natural shape width.
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).center());
    let text_idx = tree.add_child(root, TextWidget::new("Count: 0").font_size(32.0));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);
    let text_rect = tree.layout_rect(text_idx);

    // Shape natural to know the target
    let natural = engine.shape_text("Count: 0", 32.0, 22.0, None);

    assert!(
        text_rect.size.width >= natural.width,
        "layout width ({}) must be >= natural shape width ({}) so paint \
         does not wrap with `max_width = layout_width`",
        text_rect.size.width,
        natural.width,
    );
    // Sanity: we got a single-line layout (two-line would be >=44)
    assert!(
        text_rect.size.height < 40.0,
        "single-line height expected, got {}",
        text_rect.size.height,
    );
}

#[test]
fn text_measure_does_not_wrap_when_space_is_ample() {
    // Regression guard: during flex probing Taffy may call measure with a
    // narrow available_width. If we naively shape with that as max_width we
    // get a mis-wrapped result (e.g., "Count: 0" → two lines) and Taffy
    // uses that tall/thin size as the widget's natural size. The widget
    // should only wrap when the natural width actually overflows.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let widget = TextWidget::new("Count: 0").font_size(32.0);

    // Simulate Taffy probing with a small available_width
    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let probe = <TextWidget as Widget>::measure(&widget, Some(50.0), &mut ctx)
        .expect("measure should return Some");

    // And the natural max-content (no constraint)
    let natural = <TextWidget as Widget>::measure(&widget, None, &mut ctx)
        .expect("measure should return Some");

    // Under a tight probe we *will* wrap; that's fine.
    // But with no constraint we must get the single-line size.
    // The critical property: passing a LARGE available_width must NOT wrap.
    let big = <TextWidget as Widget>::measure(&widget, Some(1000.0), &mut ctx)
        .expect("measure should return Some");
    assert_eq!(
        big.width, natural.width,
        "with ample available_width, measure must match natural max-content \
         (no spurious wrapping). natural={:?}, big={:?}, probe={:?}",
        natural, big, probe,
    );
    assert_eq!(big.height, natural.height);
}

#[test]
fn reactive_text_relayouts_when_content_grows() {
    // Regression guard for the counter "10からずれる" bug. Taffy memoizes
    // leaf measure results by (node, available_width, available_height); a
    // reactive widget whose closure now returns a longer string was getting
    // its old (narrower) width back from the cache, then paint re-shaped
    // with `max_width = stale_width` and wrapped. `compute_layout_with_
    // measure` now marks every node dirty per pass to force re-measure.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let counter = Rc::new(Cell::new(0i32));
    let counter_for_text = Rc::clone(&counter);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).center());
    let text_idx = tree.add_child(
        root,
        TextWidget::reactive(move || format!("Count: {}", counter_for_text.get())).font_size(32.0),
    );

    let mut engine = TextEngine::new();
    let theme = Theme::default();

    let mut widths = Vec::new();
    for n in 0..=12 {
        counter.set(n);
        tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);
        let rect = tree.layout_rect(text_idx);
        widths.push((n, rect.size.width, rect.size.height));
    }

    // n=9 → "Count: 9" (1 digit); n=10 → "Count: 10" (2 digits, wider).
    let w9 = widths[9].1;
    let w10 = widths[10].1;
    assert!(
        w10 > w9,
        "layout width at n=10 ({}) must exceed n=9 ({}) — Taffy is caching \
         the stale measure. Full table: {:?}",
        w10,
        w9,
        widths,
    );

    // Every entry must remain a single line (height < 40 at line_height 22).
    for (n, _w, h) in &widths {
        assert!(
            *h < 40.0,
            "n={} produced multi-line layout (height={}). Full table: {:?}",
            n,
            h,
            widths,
        );
    }
}

// ── Reactive Button / Container (Phase 14b) ──────────────────────

#[test]
fn button_background_accepts_literal_via_reactive() {
    // Regression: a literal `Color` still flows through `Button::background`
    // now that it takes `impl Into<Reactive<Color>>`.
    let custom = Color::rgb(0.7, 0.1, 0.1);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(80.0));
    tree.add_child(root, Button::new("X").background(custom));
    tree.compute_layout(200.0, 80.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let bg = &ctx.rects[0];
    assert_eq!(
        bg.color, custom,
        "static color must still reach the Button bg"
    );
}

#[test]
fn button_background_tracks_signal_updates() {
    // Signal<Color> → Reactive<Color> via `From<Signal<T>>`. Flipping the
    // signal and repainting must produce the new bg color. Mirrors
    // `text_color_tracks_signal_updates` for TextWidget.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let green = Color::rgb(0.0, 1.0, 0.0);
    let bg = Signal::new(red);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(80.0));
    tree.add_child(root, Button::new("Y").background(bg));
    tree.compute_layout(200.0, 80.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    assert_eq!(ctx1.rects[0].color, red);

    bg.set(green);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert_eq!(
        ctx2.rects[0].color, green,
        "Button background must reflect the updated Signal on next paint"
    );
}

#[test]
fn button_text_color_tracks_signal_updates() {
    // Signal<Color> driving the glyph color — the other reactive color
    // channel on Button. Glyphs are emitted after the bg rect.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let green = Color::rgb(0.0, 1.0, 0.0);
    let text = Signal::new(red);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(80.0));
    tree.add_child(root, Button::new("Z").text_color(text));
    tree.compute_layout(200.0, 80.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    assert!(!ctx1.glyphs.is_empty(), "expected label glyphs");
    for g in &ctx1.glyphs {
        assert_eq!(g.color, red);
    }

    text.set(green);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(!ctx2.glyphs.is_empty());
    for g in &ctx2.glyphs {
        assert_eq!(g.color, green);
    }
}

#[test]
fn button_reactive_label_reflects_signal() {
    // `Button::reactive_label` parallels `TextWidget::reactive` — the label
    // closure is re-read each paint, so updating the signal changes the
    // rendered glyph set. We assert glyph count grows when the label does.
    let count = Signal::new(0i32);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(
        root,
        Button::reactive_label(move || format!("n={}", count.get())).font_size(18.0),
    );
    tree.compute_layout(400.0, 100.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    let short_glyphs = ctx1.glyphs.len();
    assert!(short_glyphs >= 3, "expected glyphs for 'n=0'");

    count.set(123456);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(
        ctx2.glyphs.len() > short_glyphs,
        "reactive label should repaint with longer text (got {} → {})",
        short_glyphs,
        ctx2.glyphs.len()
    );
}

#[test]
fn button_builder_accepts_reactive_color_sources() {
    // Compile-time regression: every color setter must accept a literal,
    // a `Signal`, and a `Reactive::derive` closure — the three conversion
    // paths users will reach for.
    use shroud_reactive::Reactive;
    let sig = Signal::new(Color::rgb(0.2, 0.2, 0.2));
    let _btn = Button::new("T")
        .background(Color::rgb(0.1, 0.1, 0.1))
        .hover_background(sig)
        .press_background(Reactive::derive(|| Color::rgb(0.3, 0.3, 0.3)))
        .text_color(Color::WHITE);
}

#[test]
fn container_background_accepts_literal_via_reactive() {
    // Regression: existing `.background(Color::X)` calls still reach the
    // Container's paint path after the `Reactive<Color>` migration.
    let custom = Color::rgb(0.2, 0.4, 0.6);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(100.0)
            .height(50.0)
            .background(custom),
    );
    let _ = root;
    tree.compute_layout(100.0, 50.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert_eq!(ctx.rects.len(), 1, "container with bg should paint 1 rect");
    assert_eq!(ctx.rects[0].color, custom);
}

#[test]
fn container_background_tracks_signal_updates() {
    // Signal<Color> → Container bg. The pull-based model means the updated
    // value is observed on the next paint without any manual subscription.
    let red = Color::rgb(1.0, 0.0, 0.0);
    let green = Color::rgb(0.0, 1.0, 0.0);
    let bg = Signal::new(red);

    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(100.0).height(50.0).background(bg));
    tree.compute_layout(100.0, 50.0);

    let mut ctx1 = PaintContext::default();
    tree.paint(&mut ctx1);
    assert_eq!(ctx1.rects[0].color, red);

    bg.set(green);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert_eq!(
        ctx2.rects[0].color, green,
        "Container background must reflect the updated Signal on next paint"
    );
}

#[test]
fn container_background_accepts_reactive_derive_closure() {
    // `Reactive::derive` is the escape hatch when neither a literal nor a
    // direct `Signal`/`Memo` conversion fits — here we combine a bool with
    // two literal colors to pick a background reactively.
    use shroud_reactive::Reactive;
    let on_color = Color::rgb(0.2, 0.8, 0.2);
    let off_color = Color::rgb(0.2, 0.2, 0.2);
    let enabled = Signal::new(true);
    let derived = Reactive::derive(move || if enabled.get() { on_color } else { off_color });

    let mut tree = WidgetTree::new();
    tree.set_root(
        Container::column()
            .width(100.0)
            .height(50.0)
            .background(derived),
    );
    tree.compute_layout(100.0, 50.0);

    let mut ctx_on = PaintContext::default();
    tree.paint(&mut ctx_on);
    assert_eq!(ctx_on.rects[0].color, on_color);

    enabled.set(false);
    let mut ctx_off = PaintContext::default();
    tree.paint(&mut ctx_off);
    assert_eq!(ctx_off.rects[0].color, off_color);
}

// ── Visibility (Phase 18b) ───────────────────────────────────────

#[test]
fn hidden_container_collapses_layout_space() {
    // `.visible(false)` must give `display: none` semantics: the container
    // occupies zero space so sibling layout closes up. Regression guard for
    // the gap #2 in the password_manager MVP (conditional rows).
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(100.0));
    tree.add_child(root, Container::row().width(100.0).height(100.0));
    let hidden = tree.add_child(
        root,
        Container::row().width(100.0).height(100.0).visible(false),
    );
    let after = tree.add_child(root, Container::row().width(100.0).height(100.0));

    tree.compute_layout(400.0, 100.0);

    let hidden_rect = tree.layout_rect(hidden);
    assert_eq!(
        hidden_rect.size.width, 0.0,
        "hidden container should collapse to width 0, got {}",
        hidden_rect.size.width
    );

    let after_rect = tree.layout_rect(after);
    assert_eq!(
        after_rect.origin.x, 100.0,
        "sibling after a hidden node should sit flush against the previous \
         visible sibling (x=100), got x={}",
        after_rect.origin.x,
    );
}

#[test]
fn signal_flip_toggles_container_visibility() {
    // Flipping a `Signal<bool>` between layout passes must propagate to
    // Taffy — the widget should reclaim space when the signal goes `true`
    // and release it when it goes `false`.
    let shown = Signal::new(true);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(100.0));
    let toggled = tree.add_child(
        root,
        Container::row().width(100.0).height(100.0).visible(shown),
    );
    let tail = tree.add_child(root, Container::row().width(100.0).height(100.0));

    tree.compute_layout(400.0, 100.0);
    assert_eq!(tree.layout_rect(toggled).size.width, 100.0);
    assert_eq!(tree.layout_rect(tail).origin.x, 100.0);

    shown.set(false);
    tree.compute_layout(400.0, 100.0);
    assert_eq!(
        tree.layout_rect(toggled).size.width,
        0.0,
        "after Signal flip to false, width should collapse"
    );
    assert_eq!(
        tree.layout_rect(tail).origin.x,
        0.0,
        "after Signal flip, sibling should slide left"
    );

    shown.set(true);
    tree.compute_layout(400.0, 100.0);
    assert_eq!(
        tree.layout_rect(toggled).size.width,
        100.0,
        "Signal flipping back to true must restore the widget's size"
    );
    assert_eq!(tree.layout_rect(tail).origin.x, 100.0);
}

#[test]
fn hidden_subtree_is_not_painted() {
    // A hidden Container must skip paint for both itself and its descendants.
    // We count rects: a visible Container with one Button child paints 2
    // rects (container bg + button bg); hiding the container should drop
    // both to 0.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let group = tree.add_child(
        root,
        Container::column()
            .width(200.0)
            .height(100.0)
            .background(Color::rgb(0.5, 0.5, 0.5))
            .visible(false),
    );
    tree.add_child(group, Button::new("inside"));

    tree.compute_layout(400.0, 200.0);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    assert_eq!(
        ctx.rects.len(),
        0,
        "hidden subtree must produce no rects (got {})",
        ctx.rects.len()
    );
    assert_eq!(ctx.glyphs.len(), 0, "hidden subtree must produce no glyphs");
}

#[test]
fn hidden_button_does_not_receive_click() {
    // Hit-test must walk past a hidden widget, so its handler never fires.
    // Covers dispatch_to_node's early return on !visible(): without it the
    // click would still reach the Button even though its layout_rect is
    // collapsed — because child dispatch shifts through the subtree first.
    let clicked = Rc::new(Cell::new(false));
    let clicked2 = clicked.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    tree.add_child(
        root,
        Button::new("hidden")
            .on_click(move |_ctx| clicked2.set(true))
            .visible(false),
    );
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(10.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(10.0, 10.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(
        !clicked.get(),
        "hidden Button must not receive click events"
    );
}

#[test]
fn measured_button_layout_honors_visibility() {
    // Phase 18a / b interaction: under `compute_layout_with_measure`, a
    // hidden Button still has `measure()` side-stepped because its Taffy
    // style is `display: none`. The resulting layout_rect must be zero-sized.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::row().width(400.0).height(100.0));
    let hidden_btn = tree.add_child(root, Button::new("hidden-btn").visible(false));
    let after = tree.add_child(root, Button::new("shown-btn"));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 100.0, &mut engine, &theme);

    let h_rect = tree.layout_rect(hidden_btn);
    assert_eq!(
        h_rect.size.width, 0.0,
        "hidden Button should have width 0 under measured layout"
    );

    let a_rect = tree.layout_rect(after);
    assert_eq!(
        a_rect.origin.x, 0.0,
        "visible sibling should sit at x=0 when the only earlier child is hidden"
    );
}

#[test]
fn measured_button_inside_center_has_label_width() {
    // Same regression guard but for Button: its measured width must exceed
    // zero (shaped label + padding) so `.center()` positions it sanely.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).center());
    let button_idx = tree.add_child(root, Button::new("Increment"));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(400.0, 300.0, &mut engine, &theme);

    let rect = tree.layout_rect(button_idx);
    // Button padding is 8px per side = +16 added by Taffy, so the total width
    // must exceed 16 for the shaped label to fit.
    assert!(
        rect.size.width > 16.0,
        "Button inside .center() should have width > padding (got {})",
        rect.size.width,
    );
    assert!(rect.size.height > 0.0);
}

// ── Phase 18c-1: dynamic tree mutation ────────────────────────────

/// Counts invocations of `Drop` via a shared `Cell`. Used to assert that
/// `remove` and `replace_root` actually drop every widget in the subtree,
/// which is what drives zeroize for secure widgets.
struct DropCounter {
    counter: Rc<Cell<u32>>,
}

impl DropCounter {
    fn new(counter: Rc<Cell<u32>>) -> Self {
        Self { counter }
    }
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.counter.set(self.counter.get() + 1);
    }
}

impl shroud_widgets::Widget for DropCounter {
    fn style(&self) -> shroud_layout::FlexStyle {
        shroud_layout::FlexStyle::new().width(10.0).height(10.0)
    }
    fn paint(&self, _layout: shroud_core::Rect, _ctx: &mut PaintContext) {}
}

#[test]
fn tree_remove_leaf_tombstones_slot_and_keeps_siblings() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let a = tree.add_child(root, Container::row().height(50.0));
    let b = tree.add_child(root, Container::row().height(50.0));
    assert_eq!(tree.len(), 3);

    tree.remove(a);

    assert!(!tree.contains(a), "removed slot must tombstone");
    assert!(tree.try_widget(a).is_none());
    assert!(tree.contains(b), "sibling index stays stable across remove");
    assert!(tree.contains(root));
    assert_eq!(tree.len(), 2);
}

#[test]
fn tree_remove_cascades_to_descendants() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let branch = tree.add_child(root, Container::column());
    let leaf = tree.add_child(branch, TextWidget::new("leaf"));

    tree.remove(branch);

    assert!(!tree.contains(branch));
    assert!(
        !tree.contains(leaf),
        "descendants must be removed with their parent"
    );
    assert!(tree.contains(root));
    assert_eq!(tree.len(), 1);
}

#[test]
fn tree_remove_drops_every_widget_in_subtree() {
    let counter = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let parent = tree.add_child(root, DropCounter::new(counter.clone()));
    let _c1 = tree.add_child(parent, DropCounter::new(counter.clone()));
    let _c2 = tree.add_child(parent, DropCounter::new(counter.clone()));

    assert_eq!(counter.get(), 0);
    tree.remove(parent);
    assert_eq!(counter.get(), 3, "parent + 2 children should all drop");
}

#[test]
fn tree_remove_root_clears_root_pointer() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, Container::row().height(50.0));

    tree.remove(root);

    assert!(tree.root().is_none());
    assert!(!tree.contains(root));
    assert!(tree.is_empty());
}

#[test]
fn tree_remove_tombstoned_index_is_noop() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let a = tree.add_child(root, Container::row().height(50.0));

    tree.remove(a);
    // Repeated remove of a tombstone must not panic.
    tree.remove(a);
    // Out-of-range index: also silently ignored.
    tree.remove(9999);

    assert!(!tree.contains(a));
    assert_eq!(tree.len(), 1);
}

#[test]
fn tree_remove_clears_hover_when_hovered_widget_goes_away() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let target = tree.add_child(root, Button::new("hover me"));
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(target);
    let mut event_ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(tree.hovered(), Some(target));

    tree.remove(target);
    assert!(
        tree.hovered().is_none(),
        "removed widget must leave hover cleared"
    );
}

#[test]
fn tree_replace_root_drops_old_subtree() {
    let counter = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let old_root = tree.set_root(DropCounter::new(counter.clone()));
    tree.add_child(old_root, DropCounter::new(counter.clone()));
    assert_eq!(tree.len(), 2);

    let new_root = tree.replace_root(Container::column().width(10.0).height(10.0));
    assert_eq!(
        counter.get(),
        2,
        "replace_root drops old root + descendants"
    );
    assert!(!tree.contains(old_root));
    assert_eq!(tree.root(), Some(new_root));
    assert_eq!(tree.len(), 1);
}

#[test]
fn replace_root_flags_a_swap_for_the_shape_cache() {
    // The event loop reads this flag to drop the shape cache on a screen swap
    // (e.g. a vault lock), so glyph geometry derived from the old screen's
    // plaintext does not outlive it. It must be one-shot: set by replace_root,
    // cleared on read, and never set by a plain set_root / add_child.
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(10.0).height(10.0));
    assert!(
        !tree.take_root_replaced(),
        "the initial set_root is not a swap"
    );

    tree.add_child(tree.root().unwrap(), TextWidget::new("child"));
    assert!(!tree.take_root_replaced(), "adding a child is not a swap");

    tree.replace_root(Container::column().width(20.0).height(20.0));
    assert!(tree.take_root_replaced(), "replace_root flags a swap");
    assert!(
        !tree.take_root_replaced(),
        "the flag is one-shot: cleared on read"
    );
}

#[test]
fn event_handler_can_queue_remove_via_context() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));

    let victim = tree.add_child(root, Button::new("victim"));
    let killer = tree.add_child(
        root,
        Button::new("kill").on_click(move |ctx| {
            ctx.remove(victim);
        }),
    );
    tree.compute_layout(400.0, 200.0);

    let rect = tree.layout_rect(killer);
    let pos = Point::new(
        rect.origin.x + rect.size.width / 2.0,
        rect.origin.y + rect.size.height / 2.0,
    );
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(
        !tree.contains(victim),
        "handler-enqueued remove fires after dispatch"
    );
    assert!(tree.contains(killer));
}

#[test]
fn event_handler_replace_screen_rebuilds_tree() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let btn = tree.add_child(
        root,
        Button::new("transition").on_click(move |ctx| {
            ctx.replace_screen(|tree| {
                let new_root = tree.set_root(Container::row().width(400.0).height(100.0));
                tree.add_child(new_root, TextWidget::new("screen 2"));
            });
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(btn);
    let pos = Point::new(
        rect.origin.x + rect.size.width / 2.0,
        rect.origin.y + rect.size.height / 2.0,
    );
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(
        !tree.contains(root),
        "old root tombstoned by replace_screen"
    );
    assert!(!tree.contains(btn));
    let new_root = tree.root().expect("new root installed by rebuild closure");
    assert!(tree.contains(new_root));
    assert_eq!(tree.len(), 2, "new screen = root + its one child");
}

#[test]
fn secure_input_drops_when_removed_from_tree() {
    // SecureString zeroize-on-drop is covered in shroud_security's own tests.
    // Here we pin down the tree-level guarantee: removing a SecureInput
    // must actually drop it (so the zeroize chain is reachable), and must
    // not disturb unrelated siblings.
    let counter = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let pw = tree.add_child(root, SecureInput::new().placeholder("pw"));
    tree.add_child(root, DropCounter::new(counter.clone()));

    tree.remove(pw);
    assert!(!tree.contains(pw));
    assert_eq!(counter.get(), 0, "unrelated sibling must not drop");

    tree.remove(root);
    assert_eq!(counter.get(), 1, "cascading root removal drops the sibling");
}

// ── Phase 18c-2: subtree rebuild ──────────────────────────────────

#[test]
fn rebuild_children_replaces_existing_children() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _old1 = tree.add_child(root, Container::row().height(20.0));
    let _old2 = tree.add_child(root, Container::row().height(20.0));
    assert_eq!(tree.len(), 3);

    let mut event_ctx = EventContext::new();
    event_ctx.rebuild_children(root, |tree, parent| {
        tree.add_child(parent, Container::row().height(30.0));
        tree.add_child(parent, Container::row().height(30.0));
        tree.add_child(parent, Container::row().height(30.0));
    });
    // Drain via a no-op dispatch so apply_commands runs.
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert!(tree.contains(root), "parent must survive rebuild");
    assert_eq!(
        tree.len(),
        4,
        "root + 3 fresh children; old children tombstoned"
    );
}

#[test]
fn rebuild_children_drops_old_subtree() {
    let counter = Rc::new(Cell::new(0u32));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let mid = tree.add_child(root, DropCounter::new(counter.clone()));
    tree.add_child(mid, DropCounter::new(counter.clone()));
    tree.add_child(mid, DropCounter::new(counter.clone()));
    assert_eq!(counter.get(), 0);

    let mut event_ctx = EventContext::new();
    event_ctx.rebuild_children(root, |tree, parent| {
        tree.add_child(parent, Container::row().height(10.0));
    });
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert_eq!(
        counter.get(),
        3,
        "rebuild drops mid + both grandchildren; cascades through subtree"
    );
}

#[test]
fn rebuild_children_preserves_parent_and_unrelated_siblings() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let list = tree.add_child(root, Container::column());
    tree.add_child(list, Container::row().height(20.0));
    let keep = tree.add_child(root, Container::row().height(40.0));
    assert_eq!(tree.len(), 4);

    let mut event_ctx = EventContext::new();
    event_ctx.rebuild_children(list, |tree, parent| {
        tree.add_child(parent, Container::row().height(20.0));
        tree.add_child(parent, Container::row().height(20.0));
    });
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert!(tree.contains(list), "list parent index stays live");
    assert!(
        tree.contains(keep),
        "sibling outside the rebuilt subtree is untouched"
    );
    // root + keep + list + 2 fresh rows = 5
    assert_eq!(tree.len(), 5);
}

#[test]
fn rebuild_children_is_noop_when_parent_tombstoned_first() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let list = tree.add_child(root, Container::column());
    tree.add_child(list, Container::row().height(20.0));

    // Queue: first remove the list, then try to rebuild its children. The
    // rebuild must silently skip once drain notices the tombstone — without
    // this the builder could install children on a dead parent and leak.
    let rebuild_fired = Rc::new(Cell::new(false));
    let flag = Rc::clone(&rebuild_fired);
    let mut event_ctx = EventContext::new();
    event_ctx.remove(list);
    event_ctx.rebuild_children(list, move |tree, parent| {
        flag.set(true);
        tree.add_child(parent, Container::row().height(20.0));
    });
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert!(!tree.contains(list), "list was removed first");
    assert!(
        !rebuild_fired.get(),
        "builder must not run when parent was tombstoned mid-queue"
    );
}

#[test]
fn rebuild_children_empty_parent_just_runs_builder() {
    // A parent with zero existing children is a legal starting state — e.g.
    // a freshly-added list container. Rebuild should still populate.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let list = tree.add_child(root, Container::column());
    assert_eq!(tree.len(), 2);

    let mut event_ctx = EventContext::new();
    event_ctx.rebuild_children(list, |tree, parent| {
        tree.add_child(parent, Container::row().height(10.0));
        tree.add_child(parent, Container::row().height(10.0));
    });
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(-999.0, -999.0),
        },
        &mut event_ctx,
    );

    assert_eq!(tree.len(), 4);
}

#[test]
fn event_handler_rebuild_children_from_button_click() {
    // Exercises the full password-manager-style path: a click handler
    // enqueues `rebuild_children` for a sibling list container, and the
    // drain at the end of dispatch applies it.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let list = tree.add_child(root, Container::column());
    tree.add_child(list, Container::row().height(20.0));
    tree.add_child(list, Container::row().height(20.0));

    let btn = tree.add_child(
        root,
        Button::new("rebuild").on_click(move |ctx| {
            ctx.rebuild_children(list, |tree, parent| {
                // New row count (1) differs from old (2) so len() changes.
                tree.add_child(parent, Container::row().height(20.0));
            });
        }),
    );
    tree.compute_layout(400.0, 200.0);

    let rect = tree.layout_rect(btn);
    let pos = Point::new(
        rect.origin.x + rect.size.width / 2.0,
        rect.origin.y + rect.size.height / 2.0,
    );
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: pos,
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    assert!(tree.contains(list));
    assert!(tree.contains(btn));
    // root + list + button + 1 new row = 4
    assert_eq!(tree.len(), 4);
}

// ── Phase 18d: reactive value + ClearTrigger ──────────────────────

#[test]
fn input_value_seeds_buffer_from_signal() {
    let sig = Signal::new(String::from("prefilled"));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let _idx = tree.add_child(root, Input::new().value(sig));
    tree.compute_layout(400.0, 100.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    // If the signal's initial value didn't seed the buffer, the input
    // would paint as empty (no glyphs) since there's no placeholder.
    assert!(
        !ctx.glyphs.is_empty(),
        "bound input must render the signal's initial value"
    );
}

#[test]
fn input_typing_writes_back_to_bound_signal() {
    let sig = Signal::new(String::new());

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(sig));
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    for ch in ['h', 'i'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    assert_eq!(
        sig.get_clone(),
        "hi",
        "each keystroke should flush to the bound signal"
    );
}

#[test]
fn input_cursor_signal_mirrors_caret() {
    // The bound cursor signal must track the caret as it moves, so a sibling
    // widget (e.g. a formatting toolbar) can read where the caret is.
    let sig = Signal::new(String::new());
    let csig = Signal::new(0usize);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(sig).cursor_signal(csig));
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    for ch in ['a', 'b', 'c'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }
    assert_eq!(csig.get(), 3, "caret signal should follow typing to byte 3");

    tree.dispatch_event(
        &key(shroud_widgets::event::NamedKey::ArrowLeft),
        &mut event_ctx,
    );
    assert_eq!(csig.get(), 2, "ArrowLeft must mirror the moved caret back");
}

#[test]
fn input_cursor_signal_external_set_moves_caret() {
    // Writing the cursor signal from outside must reposition the caret on the
    // next sync — the mechanism a toolbar uses to drop the caret onto its
    // freshly-inserted text. Probe with a marker char.
    let sig = Signal::new(String::from("abcdef"));
    let csig = Signal::new(0usize);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(sig).cursor_signal(csig));
    tree.compute_layout(400.0, 100.0);

    // Focus (MouseDown lands the caret at the end, byte 6).
    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    // External write, as a toolbar would do after inserting text.
    csig.set(2);
    // The next event rebases the caret before applying the edit (sync runs at
    // the top of `event`), so the marker lands at byte 2.
    assert_cursor_inserts_at(&mut tree, &mut event_ctx, &sig, "abMcdef");
}

#[test]
fn input_cursor_signal_snaps_to_char_boundary() {
    // An external offset that falls inside a multi-byte codepoint must snap
    // down to the nearest char boundary, or the paint-side `&value[..cursor]`
    // slice would panic. "あい" = bytes 0..3 ("あ") and 3..6 ("い"); asking
    // for byte 4 (mid-"い") must clamp to byte 3.
    let sig = Signal::new(String::from("あい"));
    let csig = Signal::new(0usize);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(sig).cursor_signal(csig));
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );

    csig.set(4);
    assert_cursor_inserts_at(&mut tree, &mut event_ctx, &sig, "あMい");
}

#[test]
fn input_external_signal_set_rebases_buffer_by_next_paint() {
    // Binds a signal, externally overwrites it, confirms the next paint
    // renders the new value rather than the stale local buffer. Covers the
    // "app sets signal → redraw → widget reflects it without an event" path.
    let sig = Signal::new(String::from("old"));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let _idx = tree.add_child(root, Input::new().value(sig));
    tree.compute_layout(400.0, 100.0);

    // First paint — baseline that the widget rendered at all.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    let first_glyphs = ctx.glyphs.len();
    assert!(first_glyphs > 0);

    // External write, then a fresh paint. The new value is longer, so the
    // glyph count should grow even though no event dispatched in between.
    sig.set("old_and_new".into());
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert!(
        ctx2.glyphs.len() > first_glyphs,
        "paint must resync from the signal (first={} second={})",
        first_glyphs,
        ctx2.glyphs.len()
    );
}

#[test]
fn input_external_shorter_value_clamps_cursor() {
    // Regression: if cursor pointed past the end of the new buffer, paint
    // would slice `&value[..cursor]` and panic. Bind a signal with a long
    // value, type at the end to push the cursor there, then shrink the
    // signal and ensure paint succeeds.
    let sig = Signal::new(String::from("longer value"));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(sig));
    tree.compute_layout(400.0, 100.0);

    // Focus so the cursor is drawn (which exercises the slice path).
    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    // MouseDown placed cursor at end (byte len "longer value" = 12).
    // Now shrink externally to 2 bytes.
    sig.set("hi".into());

    // Should not panic — cursor must be clamped to 2 before the paint-side
    // `&value[..cursor]` slice runs.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
}

#[test]
fn input_on_change_fires_after_signal_writeback() {
    // Bound inputs should still deliver on_change with the fresh text.
    // Regression guard for the ordering (write-back → on_change).
    let seen = Rc::new(RefCell::new(String::new()));
    let seen2 = Rc::clone(&seen);

    let sig = Signal::new(String::new());

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(
        root,
        Input::new().value(sig).on_change(move |s, _ctx| {
            *seen2.borrow_mut() = s.to_string();
        }),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'x' }, &mut event_ctx);

    assert_eq!(sig.get_clone(), "x");
    assert_eq!(*seen.borrow(), "x");
}

#[test]
fn secure_input_clear_on_zeroizes_after_bump() {
    // Type a character, bump the trigger, verify the buffer is empty on
    // the next event-driven sync. Uses a keyless event (Escape) so the
    // test doesn't depend on any specific handler running.
    let trigger = ClearTrigger::new();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, SecureInput::new().placeholder("pw").clear_on(trigger));
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    for ch in ['s', 'e', 'c'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    // Pre-bump: three characters are held in the SecureString. We paint
    // (which also syncs clear) first to confirm the sync doesn't trigger
    // without a bump.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    // Bump the trigger. Next paint's sync should zeroize the buffer.
    trigger.bump();
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);

    // After clear, the widget renders the placeholder (not masked dots).
    // We can't reach `value` directly from outside — instead, confirm
    // that typing a fresh character now produces exactly one mask glyph
    // on the next paint (cursor was reset to 0, char_count is 1).
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'a' }, &mut event_ctx);
    let mut ctx3 = PaintContext::default();
    tree.paint(&mut ctx3);

    // Three chars would have produced 3 mask glyphs + cursor. After clear
    // + one char, we expect 1 mask glyph + cursor. The exact glyph count
    // depends on the font, but it should be strictly less than pre-clear.
    let pre_clear_glyphs = ctx.glyphs.len();
    let post_clear_plus_one_glyphs = ctx3.glyphs.len();
    assert!(
        post_clear_plus_one_glyphs < pre_clear_glyphs,
        "clear_on must shrink the rendered glyph count (pre={} post={})",
        pre_clear_glyphs,
        post_clear_plus_one_glyphs
    );
}

#[test]
fn secure_input_clear_on_observed_without_any_event() {
    // Paint-only path: no events dispatched between the bump and the
    // observing paint. Ensures paint carries its own sync and the app
    // doesn't have to fake an event to see the clear.
    let trigger = ClearTrigger::new();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, SecureInput::new().placeholder("pw").clear_on(trigger));
    tree.compute_layout(400.0, 100.0);

    // Focus + type via events (the only way to add characters).
    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    for ch in ['s', 'e', 'c', 'r', 'e', 't'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    // Snapshot glyph count with 6 chars held.
    let mut pre = PaintContext::default();
    tree.paint(&mut pre);
    let pre_count = pre.glyphs.len();
    assert!(pre_count >= 6, "6 masked chars expected, got {}", pre_count);

    // Bump and paint again — no events in between.
    trigger.bump();
    let mut post = PaintContext::default();
    tree.paint(&mut post);

    // Empty buffer → placeholder only, which is shorter than 6 dots.
    assert!(
        post.glyphs.len() < pre_count,
        "paint-only sync should have zeroized the buffer (pre={} post={})",
        pre_count,
        post.glyphs.len()
    );
}

#[test]
fn secure_input_attaching_prebumped_trigger_does_not_spuriously_clear() {
    // If bump() was called before the widget was even built, attaching the
    // trigger must capture that version as the baseline — otherwise the
    // widget would clear itself on first paint, defeating any typed value
    // before the caller actually bumps post-attach.
    let trigger = ClearTrigger::new();
    trigger.bump();
    trigger.bump();

    // Deliberately no placeholder so an empty buffer renders zero glyphs —
    // makes "was it cleared?" directly checkable from the outside.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, SecureInput::new().clear_on(trigger));
    tree.compute_layout(400.0, 100.0);

    // Type one char.
    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'a' }, &mut event_ctx);

    // Paint. No bump since clear_on → no clear. Glyphs should include the
    // mask char. Would be 0 if the pre-bumped version had been treated as
    // "fire a clear on first observation".
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert!(
        !ctx.glyphs.is_empty(),
        "typed char must survive a pre-bumped trigger attachment"
    );

    // Sanity: an explicit bump *after* attach still clears.
    trigger.bump();
    let mut after = PaintContext::default();
    tree.paint(&mut after);
    assert!(
        after.glyphs.is_empty(),
        "bump post-attach must zeroize — empty buffer → no glyphs (no placeholder)"
    );
}

// ── Phase 19a-1: focus primitive & Tab routing ────────────────────

/// Test widget that (a) opts into focus via `focusable()`, (b) can be
/// hidden to exercise visibility-skipping in tab order, and (c) records
/// every `FocusGained` / `FocusLost` it receives so tests can assert on
/// the dispatch sequence.
struct FocusProbe {
    focusable: bool,
    visible: bool,
    events: Rc<RefCell<Vec<String>>>,
    tag: &'static str,
}

impl FocusProbe {
    fn new(tag: &'static str, events: Rc<RefCell<Vec<String>>>) -> Self {
        Self {
            focusable: true,
            visible: true,
            events,
            tag,
        }
    }

    fn non_focusable(mut self) -> Self {
        self.focusable = false;
        self
    }

    fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }
}

impl shroud_widgets::Widget for FocusProbe {
    fn focusable(&self) -> bool {
        self.focusable
    }
    fn visible(&self) -> bool {
        self.visible
    }
    fn style(&self) -> shroud_layout::FlexStyle {
        shroud_layout::FlexStyle::new().width(10.0).height(10.0)
    }
    fn paint(&self, _: shroud_core::Rect, _: &mut PaintContext) {}
    fn event(
        &mut self,
        event: &WidgetEvent,
        _: shroud_core::Rect,
        _: &mut EventContext,
    ) -> EventResult {
        match event {
            WidgetEvent::FocusGained => {
                self.events
                    .borrow_mut()
                    .push(format!("{}:gained", self.tag));
                EventResult::Consumed
            }
            WidgetEvent::FocusLost => {
                self.events.borrow_mut().push(format!("{}:lost", self.tag));
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }
}

/// Three focusable probes under a plain container root — the standard
/// harness for tab-order assertions.
fn three_probe_tree() -> (WidgetTree, Rc<RefCell<Vec<String>>>, usize, usize, usize) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(200.0));
    let events = Rc::new(RefCell::new(Vec::new()));
    let a = tree.add_child(root, FocusProbe::new("a", events.clone()));
    let b = tree.add_child(root, FocusProbe::new("b", events.clone()));
    let c = tree.add_child(root, FocusProbe::new("c", events.clone()));
    tree.compute_layout(200.0, 200.0);
    (tree, events, a, b, c)
}

#[test]
fn widget_focusable_default_false() {
    // Sanity: a plain `Container` (no override) must not participate in
    // tab order. Regressing this would drop Tab on containers first.
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(100.0).height(100.0));
    tree.compute_layout(100.0, 100.0);

    assert_eq!(tree.focusable_in_tab_order(), Vec::<usize>::new());
}

#[test]
fn tab_order_is_dfs_preorder_over_focusables() {
    let (tree, _events, a, b, c) = three_probe_tree();

    // Root container is non-focusable, so it must not appear. Children
    // follow their insertion order (DFS pre-order of a flat parent).
    assert_eq!(tree.focusable_in_tab_order(), vec![a, b, c]);
}

#[test]
fn tab_order_skips_non_focusable_widgets() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(200.0));
    let events = Rc::new(RefCell::new(Vec::new()));
    let a = tree.add_child(root, FocusProbe::new("a", events.clone()));
    let _skip = tree.add_child(root, FocusProbe::new("x", events.clone()).non_focusable());
    let c = tree.add_child(root, FocusProbe::new("c", events));
    tree.compute_layout(200.0, 200.0);

    assert_eq!(tree.focusable_in_tab_order(), vec![a, c]);
}

#[test]
fn tab_order_skips_self_invisible_focusable() {
    // Distinct from "invisible parent hides focusable child": here the
    // focusable widget itself is hidden. Catches a regression where the
    // visibility check is only on ancestors, not on the widget being
    // evaluated.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(200.0));
    let events = Rc::new(RefCell::new(Vec::new()));
    let a = tree.add_child(root, FocusProbe::new("a", events.clone()));
    let _hidden = tree.add_child(root, FocusProbe::new("hidden", events.clone()).hidden());
    let c = tree.add_child(root, FocusProbe::new("c", events));
    tree.compute_layout(200.0, 200.0);

    assert_eq!(tree.focusable_in_tab_order(), vec![a, c]);
}

#[test]
fn tab_order_skips_invisible_subtrees() {
    // An invisible parent must hide its focusable children — matches
    // `display: none` collapse semantics from Phase 18b.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(200.0));
    let events = Rc::new(RefCell::new(Vec::new()));

    let hidden_parent = tree.add_child(
        root,
        Container::column().visible(shroud_reactive::Reactive::from(false)),
    );
    let _buried = tree.add_child(hidden_parent, FocusProbe::new("buried", events.clone()));

    let visible_sibling = tree.add_child(root, FocusProbe::new("sib", events));
    tree.compute_layout(200.0, 200.0);

    assert_eq!(tree.focusable_in_tab_order(), vec![visible_sibling]);
}

#[test]
fn advance_forward_from_none_focuses_first() {
    let (mut tree, events, a, _, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    let landed = tree.advance_focus(FocusDirection::Forward, &mut ctx);

    assert_eq!(landed, Some(a));
    assert_eq!(tree.focused(), Some(a));
    assert_eq!(*events.borrow(), vec!["a:gained"]);
}

#[test]
fn advance_backward_from_none_focuses_last() {
    let (mut tree, events, _, _, c) = three_probe_tree();
    let mut ctx = EventContext::new();

    let landed = tree.advance_focus(FocusDirection::Backward, &mut ctx);

    assert_eq!(landed, Some(c));
    assert_eq!(tree.focused(), Some(c));
    assert_eq!(*events.borrow(), vec!["c:gained"]);
}

#[test]
fn advance_forward_wraps_from_last_to_first() {
    let (mut tree, events, a, _, c) = three_probe_tree();
    let mut ctx = EventContext::new();

    // Seed focus on `c` via a forward-from-none + two forward steps is
    // tedious; use two backwards from none to land on the tail instead.
    tree.advance_focus(FocusDirection::Backward, &mut ctx);
    assert_eq!(tree.focused(), Some(c));
    events.borrow_mut().clear();

    let landed = tree.advance_focus(FocusDirection::Forward, &mut ctx);

    assert_eq!(landed, Some(a), "forward from last must wrap to first");
    assert_eq!(tree.focused(), Some(a));
    // Old focused (c) gets FocusLost, new (a) gets FocusGained. Order
    // matters: lost-before-gained keeps any cross-widget coordinator
    // code seeing a consistent "exactly one focused" invariant.
    assert_eq!(*events.borrow(), vec!["c:lost", "a:gained"]);
}

#[test]
fn advance_backward_wraps_from_first_to_last() {
    let (mut tree, events, a, _, c) = three_probe_tree();
    let mut ctx = EventContext::new();

    tree.advance_focus(FocusDirection::Forward, &mut ctx);
    assert_eq!(tree.focused(), Some(a));
    events.borrow_mut().clear();

    let landed = tree.advance_focus(FocusDirection::Backward, &mut ctx);

    assert_eq!(landed, Some(c), "backward from first must wrap to last");
    assert_eq!(*events.borrow(), vec!["a:lost", "c:gained"]);
}

#[test]
fn advance_focus_returns_none_when_no_focusables() {
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(100.0).height(100.0));
    tree.compute_layout(100.0, 100.0);
    let mut ctx = EventContext::new();

    let landed = tree.advance_focus(FocusDirection::Forward, &mut ctx);

    assert_eq!(landed, None);
    assert_eq!(tree.focused(), None);
}

#[test]
fn tab_keydown_advances_focus_forward() {
    // The tree intercepts Tab at dispatch_event and routes it to
    // advance_focus. Widgets must never see the raw Tab KeyDown, so
    // their event handlers cannot misinterpret it as a literal input.
    let (mut tree, events, a, _, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    let result = tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Tab),
        },
        &mut ctx,
    );

    assert_eq!(result, EventResult::Consumed);
    assert_eq!(tree.focused(), Some(a));
    assert_eq!(*events.borrow(), vec!["a:gained"]);
}

#[test]
fn shift_tab_keydown_advances_focus_backward() {
    // Shift is read from EventContext::modifiers (populated by the
    // event loop on ModifiersChanged). The tree's Tab interception
    // flips direction based on that snapshot.
    let (mut tree, events, _, _, c) = three_probe_tree();
    let mut ctx = EventContext::new();
    ctx.modifiers.shift = true;

    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Tab),
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), Some(c));
    assert_eq!(*events.borrow(), vec!["c:gained"]);
}

#[test]
fn removed_focused_widget_clears_focus() {
    // Focus must not dangle on a tombstoned index — the next Tab would
    // otherwise see a stale pointer, fail the `order.contains` lookup,
    // and fall back to first/last. Catch this at the remove boundary.
    let (mut tree, _events, a, _, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    tree.advance_focus(FocusDirection::Forward, &mut ctx);
    assert_eq!(tree.focused(), Some(a));

    tree.remove(a);

    assert_eq!(tree.focused(), None);
}

#[test]
fn advance_from_stale_focused_falls_back_to_first() {
    // If focus is somehow set to a widget that disappears from tab
    // order (hidden mid-session without going through remove), the
    // next advance should still do something sensible instead of
    // panicking or getting stuck.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(200.0));
    let events = Rc::new(RefCell::new(Vec::new()));
    let a = tree.add_child(root, FocusProbe::new("a", events.clone()));
    let b = tree.add_child(root, FocusProbe::new("b", events.clone()));
    tree.compute_layout(200.0, 200.0);

    let mut ctx = EventContext::new();
    tree.advance_focus(FocusDirection::Forward, &mut ctx);
    assert_eq!(tree.focused(), Some(a));

    // Remove `a` via an event-queued command routed through a Tab dispatch
    // would be the realistic path, but the simple route is direct removal
    // — focus clears, so re-seed manually via another advance starting
    // from the survivor.
    let _ = b;
    tree.remove(a);
    events.borrow_mut().clear();

    let landed = tree.advance_focus(FocusDirection::Forward, &mut ctx);
    assert_eq!(landed, Some(b));
    assert_eq!(*events.borrow(), vec!["b:gained"]);
}

// ── Phase 19a-2: click-to-focus + programmatic focus ──────────────

/// Center point of a widget's layout rect — convenience for click-to-focus
/// tests where the click has to land *inside* the target.
fn center_of(tree: &WidgetTree, idx: usize) -> Point {
    let rect = tree.layout_rect(idx);
    Point::new(
        rect.origin.x + rect.size.width / 2.0,
        rect.origin.y + rect.size.height / 2.0,
    )
}

#[test]
fn mousedown_on_focusable_widget_focuses_it() {
    // Click-to-focus: the tree hit-tests the cursor position, sees the
    // widget is focusable, and promotes it via FocusManager before the
    // widget's own MouseDown handler runs.
    let (mut tree, events, a, _, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: center_of(&tree, a),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), Some(a));
    assert_eq!(*events.borrow(), vec!["a:gained"]);
}

#[test]
fn mousedown_on_non_focusable_region_clears_focus() {
    // Click outside every focusable (e.g. on the root container) must
    // drop focus. Replaces the old broadcast_focus_lost behavior with
    // a single targeted FocusLost to the previously-focused widget.
    let (mut tree, events, _, _, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    // Seed focus on `a` (first focusable) via Tab so we have a clear
    // start state before asserting on the click-to-clear behavior.
    tree.advance_focus(FocusDirection::Forward, &mut ctx);
    events.borrow_mut().clear();

    // Click on an empty area of the root container. Root is non-focusable,
    // and no probe sits at (150, 150) — hit_test lands on the container.
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(150.0, 150.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), None);
    assert_eq!(*events.borrow(), vec!["a:lost"]);
}

#[test]
fn mousedown_transitions_focus_from_a_to_b() {
    // Lost-before-gained ordering matters: a coordinator watching both
    // events never sees two focused widgets simultaneously.
    let (mut tree, events, a, b, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: center_of(&tree, a),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
    events.borrow_mut().clear();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: center_of(&tree, b),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), Some(b));
    assert_eq!(*events.borrow(), vec!["a:lost", "b:gained"]);
}

#[test]
fn mousedown_on_already_focused_widget_is_noop() {
    // Short-circuit: focus(Some(x)) when already x must not re-dispatch
    // FocusGained. Widgets that do work on Gained (e.g. start a caret
    // blink timer) would misbehave if we fired it repeatedly.
    let (mut tree, events, a, _, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    tree.advance_focus(FocusDirection::Forward, &mut ctx);
    events.borrow_mut().clear();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: center_of(&tree, a),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), Some(a));
    assert!(
        events.borrow().is_empty(),
        "clicking the already-focused widget must not re-fire focus events"
    );
}

#[test]
fn programmatic_focus_dispatches_gain_and_returns_prev() {
    // The public tree.focus(...) entrypoint — apps call this to seed
    // focus after a screen transition. Returns the previous focus so
    // the caller can restore it (undo / cancel flow).
    let (mut tree, events, a, b, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    let prev1 = tree.focus(Some(a), &mut ctx);
    assert_eq!(prev1, None);
    assert_eq!(tree.focused(), Some(a));

    let prev2 = tree.focus(Some(b), &mut ctx);
    assert_eq!(prev2, Some(a));
    assert_eq!(tree.focused(), Some(b));
    assert_eq!(*events.borrow(), vec!["a:gained", "a:lost", "b:gained"]);
}

#[test]
fn programmatic_focus_none_blurs_current() {
    // tree.focus(None) must blur the currently-focused widget without
    // granting focus to anyone — symmetric with the click-on-blank-area
    // path, but callable directly.
    let (mut tree, events, a, _, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    tree.focus(Some(a), &mut ctx);
    events.borrow_mut().clear();

    let prev = tree.focus(None, &mut ctx);

    assert_eq!(prev, Some(a));
    assert_eq!(tree.focused(), None);
    assert_eq!(*events.borrow(), vec!["a:lost"]);
}

#[test]
fn mousedown_on_non_focusable_widget_clears_focus() {
    // Distinct from the "missed all widgets" case: here the click *does*
    // land on a widget, but that widget has `focusable() == false` — so
    // focus should still drop rather than stick on the previous target.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(200.0));
    let events = Rc::new(RefCell::new(Vec::new()));
    let a = tree.add_child(root, FocusProbe::new("a", events.clone()));
    let non_fc = tree.add_child(root, FocusProbe::new("x", events.clone()).non_focusable());
    tree.compute_layout(200.0, 200.0);

    let mut ctx = EventContext::new();
    tree.advance_focus(FocusDirection::Forward, &mut ctx);
    assert_eq!(tree.focused(), Some(a));
    events.borrow_mut().clear();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: center_of(&tree, non_fc),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), None);
    assert_eq!(*events.borrow(), vec!["a:lost"]);
}

#[test]
fn input_focused_state_is_tree_driven_after_click() {
    // Real-widget integration: Input::focused must become `true` as a
    // side effect of the tree's click-to-focus routing — *before* Input's
    // own MouseDown handler runs, so the handler observes a consistent
    // `focused` flag (i.e. FocusGained has already landed).
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new());
    tree.compute_layout(400.0, 100.0);

    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: center_of(&tree, idx),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), Some(idx));
    // Probe via typing — a non-focused Input ignores CharInput.
    let result = tree.dispatch_event(&WidgetEvent::CharInput { ch: 'z' }, &mut ctx);
    assert_eq!(result, EventResult::Consumed);
}

#[test]
fn tab_then_click_releases_previous_focus() {
    // Cross-modal transition: keyboard Tab focuses `a`, then a mouse
    // click on `b` takes over. Focus management must be agnostic to
    // which path set it — both funnel through the same FocusManager.
    let (mut tree, events, a, b, _) = three_probe_tree();
    let mut ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Tab),
        },
        &mut ctx,
    );
    assert_eq!(tree.focused(), Some(a));
    events.borrow_mut().clear();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: center_of(&tree, b),
            button: MouseButton::Left,
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), Some(b));
    assert_eq!(*events.borrow(), vec!["a:lost", "b:gained"]);
}

// ── Phase 19a-3: Button / Checkbox focus & keyboard activation ────

#[test]
fn button_is_focusable_by_default() {
    // Sanity check on the trait override — without this, Tab routing
    // would skip Buttons and the keyboard-activation arms below would
    // never be reachable in a real app.
    let btn = Button::new("OK");
    assert!(<Button as shroud_widgets::Widget>::focusable(&btn));
}

#[test]
fn checkbox_is_focusable_by_default() {
    let cb = Checkbox::new("Accept");
    assert!(<Checkbox as shroud_widgets::Widget>::focusable(&cb));
}

#[test]
fn button_enter_triggers_click_when_focused() {
    // Browser parity: Enter on a focused button activates it. Probed via
    // the click counter rather than `is_focused()` so the assertion proves
    // the activation path actually invoked the handler, not just that the
    // focus flag flipped.
    let clicks = Rc::new(Cell::new(0u32));
    let clicks_in = clicks.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(100.0));
    let btn = tree.add_child(
        root,
        Button::new("Go").on_click(move |_ctx| {
            clicks_in.set(clicks_in.get() + 1);
        }),
    );
    tree.compute_layout(200.0, 100.0);

    let mut ctx = EventContext::new();
    tree.focus(Some(btn), &mut ctx);
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Enter),
        },
        &mut ctx,
    );

    assert_eq!(
        clicks.get(),
        1,
        "Enter on focused Button must fire on_click"
    );
}

#[test]
fn button_space_triggers_click_when_focused() {
    // Space arrives via CharInput (winit routes the spacebar through the
    // character pipeline alongside other printable keys), so this is a
    // separate code path from Enter.
    let clicks = Rc::new(Cell::new(0u32));
    let clicks_in = clicks.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(100.0));
    let btn = tree.add_child(
        root,
        Button::new("Go").on_click(move |_ctx| {
            clicks_in.set(clicks_in.get() + 1);
        }),
    );
    tree.compute_layout(200.0, 100.0);

    let mut ctx = EventContext::new();
    tree.focus(Some(btn), &mut ctx);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: ' ' }, &mut ctx);

    assert_eq!(
        clicks.get(),
        1,
        "Space on focused Button must fire on_click"
    );
}

#[test]
fn button_enter_does_nothing_when_not_focused() {
    // Without focus, Enter must not stray into a Button's handler — every
    // Button in the tree sees the event during dispatch, but only the
    // focused one (none here) is allowed to act.
    let clicks = Rc::new(Cell::new(0u32));
    let clicks_in = clicks.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(100.0));
    tree.add_child(
        root,
        Button::new("Go").on_click(move |_ctx| {
            clicks_in.set(clicks_in.get() + 1);
        }),
    );
    tree.compute_layout(200.0, 100.0);

    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Enter),
        },
        &mut ctx,
    );

    assert_eq!(clicks.get(), 0, "Unfocused Button must ignore Enter");
}

#[test]
fn checkbox_space_toggles_when_focused() {
    let toggles = Rc::new(RefCell::new(Vec::<bool>::new()));
    let toggles_in = toggles.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(100.0));
    let cb = tree.add_child(
        root,
        Checkbox::new("Remember").on_change(move |checked, _ctx| {
            toggles_in.borrow_mut().push(checked);
        }),
    );
    tree.compute_layout(200.0, 100.0);

    let mut ctx = EventContext::new();
    tree.focus(Some(cb), &mut ctx);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: ' ' }, &mut ctx);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: ' ' }, &mut ctx);

    assert_eq!(*toggles.borrow(), vec![true, false], "Space must toggle");
}

#[test]
fn checkbox_enter_does_not_toggle_when_focused() {
    // Browser convention: Enter on a checkbox is a form-submit, not a
    // toggle. Locking this in keeps Enter free for the surrounding screen
    // (e.g. a dialog's default action) when a checkbox happens to hold
    // focus.
    let toggles = Rc::new(RefCell::new(Vec::<bool>::new()));
    let toggles_in = toggles.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(100.0));
    let cb = tree.add_child(
        root,
        Checkbox::new("Remember").on_change(move |checked, _ctx| {
            toggles_in.borrow_mut().push(checked);
        }),
    );
    tree.compute_layout(200.0, 100.0);

    let mut ctx = EventContext::new();
    tree.focus(Some(cb), &mut ctx);
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Enter),
        },
        &mut ctx,
    );

    assert!(
        toggles.borrow().is_empty(),
        "Enter must not toggle Checkbox"
    );
}

#[test]
fn tab_order_includes_button_and_checkbox() {
    // Real-widget integration: Input + Button + Checkbox under one root.
    // Verifies the focusable() override on Button/Checkbox actually lands
    // them in the tab cycle (not just that the trait bit is set).
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let input = tree.add_child(root, Input::new());
    let button = tree.add_child(root, Button::new("Go"));
    let checkbox = tree.add_child(root, Checkbox::new("Yes"));
    tree.compute_layout(400.0, 300.0);

    assert_eq!(tree.focusable_in_tab_order(), vec![input, button, checkbox]);
}

// ── Phase 19b: focus ring rendering ───────────────────────────────

#[test]
fn focus_ring_appears_when_button_focused_with_theme_color() {
    // Two paint snapshots — once unfocused, once focused — and the diff
    // must be exactly 4 rects in the theme's focus ring color (top, bottom,
    // left, right strokes). Button has no other paint that varies with
    // focus, so the +4 delta is a tight assertion.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    let btn = tree.add_child(root, Button::new("Go"));
    tree.compute_layout(200.0, 60.0);

    let mut unfocused = PaintContext::default();
    tree.paint(&mut unfocused);
    let baseline = unfocused.rects.len();

    let mut ev = EventContext::new();
    tree.focus(Some(btn), &mut ev);
    let mut focused = PaintContext::default();
    tree.paint(&mut focused);

    assert_eq!(
        focused.rects.len() - baseline,
        4,
        "focused Button must add exactly 4 ring rects"
    );

    let ring = Theme::default().focus.ring_color;
    let ring_rect_count = focused.rects.iter().filter(|r| r.color == ring).count();
    assert_eq!(
        ring_rect_count, 4,
        "all 4 added rects should use theme.focus.ring_color"
    );
}

#[test]
fn focus_ring_absent_without_focus() {
    // Companion test to the above: no widget focused → no rect should
    // bear the focus ring color. Catches a regression where the ring
    // paints unconditionally.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    tree.add_child(root, Button::new("Go"));
    tree.compute_layout(200.0, 60.0);

    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let ring = Theme::default().focus.ring_color;
    assert!(
        !ctx.rects.iter().any(|r| r.color == ring),
        "no rect should use the focus ring color when nothing is focused"
    );
}

#[test]
fn focus_ring_color_override_takes_precedence() {
    let custom = Color::rgb(1.0, 0.0, 0.0);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    let btn = tree.add_child(root, Button::new("Go").focus_ring_color(custom));
    tree.compute_layout(200.0, 60.0);

    let mut ev = EventContext::new();
    tree.focus(Some(btn), &mut ev);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let n = ctx.rects.iter().filter(|r| r.color == custom).count();
    assert_eq!(n, 4, "all 4 ring rects should use the override color");
}

#[test]
fn focus_ring_sits_outside_widget_rect() {
    // Geometry contract: with offset=2 and width=2 (defaults), the ring's
    // outer edge sits 4px beyond each widget edge. Probes via the top
    // stroke — it has the smallest y of the four ring rects, and its
    // y-coord must equal widget_y - (offset + width).
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(80.0));
    let btn = tree.add_child(root, Button::new("X"));
    tree.compute_layout(200.0, 80.0);
    let widget_rect = tree.layout_rect(btn);

    let mut ev = EventContext::new();
    tree.focus(Some(btn), &mut ev);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let theme = Theme::default();
    let ring_rects: Vec<_> = ctx
        .rects
        .iter()
        .filter(|r| r.color == theme.focus.ring_color)
        .collect();
    assert_eq!(ring_rects.len(), 4);

    let top = ring_rects
        .iter()
        .min_by(|a, b| a.y.partial_cmp(&b.y).unwrap())
        .unwrap();
    let expected = widget_rect.origin.y - (theme.focus.ring_offset + theme.focus.ring_width);
    assert!(
        (top.y - expected).abs() < 0.01,
        "top stroke y={} should equal widget_y - (offset + width) = {expected}",
        top.y
    );
}

#[test]
fn focus_ring_paints_for_input() {
    // Smoke test: focusing an Input emits 4 ring rects. Catches a
    // regression where someone removes the paint_focus_ring call from
    // Input's paint method specifically.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    let idx = tree.add_child(root, Input::new());
    tree.compute_layout(200.0, 60.0);

    let mut ev = EventContext::new();
    tree.focus(Some(idx), &mut ev);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let ring = Theme::default().focus.ring_color;
    let n = ctx.rects.iter().filter(|r| r.color == ring).count();
    assert_eq!(n, 4, "Input focus ring should render 4 rects");
}

#[test]
fn focus_ring_paints_for_checkbox() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(60.0));
    let idx = tree.add_child(root, Checkbox::new("Yes"));
    tree.compute_layout(200.0, 60.0);

    let mut ev = EventContext::new();
    tree.focus(Some(idx), &mut ev);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);

    let ring = Theme::default().focus.ring_color;
    let n = ctx.rects.iter().filter(|r| r.color == ring).count();
    assert_eq!(n, 4);
}

// ── Knot gap 3: focus_initially + EventContext::focus ─────────────

#[test]
fn focus_initially_dispatches_on_flush() {
    // The build path has no EventContext, so it can only enqueue a
    // pending focus. The event loop calls flush_pending_focus before
    // first paint — that's where FocusGained must actually fire.
    let (mut tree, events, _a, b, _c) = three_probe_tree();

    tree.focus_initially(b);
    assert_eq!(
        events.borrow().len(),
        0,
        "focus_initially must not dispatch — only enqueue"
    );

    let mut ctx = EventContext::new();
    tree.flush_pending_focus(&mut ctx);

    assert_eq!(tree.focused(), Some(b));
    assert_eq!(*events.borrow(), vec!["b:gained".to_string()]);
}

#[test]
fn flush_pending_focus_is_one_shot() {
    // After the pending target is consumed, a second flush is a no-op.
    // Catches a regression where the field stays armed and refires every
    // redraw (would re-fire FocusGained on every paint frame).
    let (mut tree, events, a, _b, _c) = three_probe_tree();

    tree.focus_initially(a);
    let mut ctx = EventContext::new();
    tree.flush_pending_focus(&mut ctx);
    events.borrow_mut().clear();

    tree.flush_pending_focus(&mut ctx);
    assert!(events.borrow().is_empty());
    // Focus stays put — flush is a no-op, not a clear.
    assert_eq!(tree.focused(), Some(a));
}

#[test]
fn flush_pending_focus_no_op_when_unset() {
    // Cheapest path: nothing pending. Must not touch focus or fire any
    // event. Event loop calls this on every redraw, so the no-op path
    // has to stay free.
    let (mut tree, events, _a, _b, _c) = three_probe_tree();
    let mut ctx = EventContext::new();
    tree.flush_pending_focus(&mut ctx);
    assert_eq!(tree.focused(), None);
    assert!(events.borrow().is_empty());
}

#[test]
fn focus_initially_overwrites_prior_pending() {
    // Two `focus_initially` calls before any flush — the second one
    // wins. Models the common case: a build closure that conditionally
    // re-targets focus before returning.
    let (mut tree, events, a, _b, c) = three_probe_tree();

    tree.focus_initially(a);
    tree.focus_initially(c);
    let mut ctx = EventContext::new();
    tree.flush_pending_focus(&mut ctx);

    assert_eq!(tree.focused(), Some(c));
    assert_eq!(*events.borrow(), vec!["c:gained".to_string()]);
}

#[test]
fn focus_initially_skips_silently_when_target_tombstoned() {
    // Race: build closure stashes index, then a different command
    // tombstones it before flush. Must not panic; pending field still
    // clears (one-shot semantics) so the next redraw is clean.
    let (mut tree, events, a, _b, _c) = three_probe_tree();

    tree.focus_initially(a);
    tree.remove(a);

    let mut ctx = EventContext::new();
    tree.flush_pending_focus(&mut ctx);

    assert_eq!(tree.focused(), None);
    // Removed widget cannot receive FocusGained.
    assert!(events.borrow().is_empty());
    // Re-flush also a no-op — the pending field was cleared even though
    // the dispatch skipped.
    tree.flush_pending_focus(&mut ctx);
    assert!(events.borrow().is_empty());
}

#[test]
fn event_context_focus_command_dispatches_on_drain() {
    // EventContext::focus enqueues TreeCommand::Focus; the drain loop
    // after dispatch must apply it via tree.focus, firing FocusGained
    // on the new target.
    let (mut tree, events, _a, b, _c) = three_probe_tree();

    let mut ctx = EventContext::new();
    ctx.focus(b);
    // Empty MouseMove dispatch — the only purpose is to trigger the
    // command-drain at the end of dispatch_event.
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: shroud_core::Point::new(-1.0, -1.0),
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), Some(b));
    assert_eq!(*events.borrow(), vec!["b:gained".to_string()]);
}

#[test]
fn event_context_blur_clears_focus_via_drain() {
    let (mut tree, events, a, _b, _c) = three_probe_tree();

    // Seed focus via the deferred path so we exercise the same plumbing.
    let mut ctx = EventContext::new();
    ctx.focus(a);
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: shroud_core::Point::new(-1.0, -1.0),
        },
        &mut ctx,
    );
    assert_eq!(tree.focused(), Some(a));
    events.borrow_mut().clear();

    ctx.blur();
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: shroud_core::Point::new(-1.0, -1.0),
        },
        &mut ctx,
    );

    assert_eq!(tree.focused(), None);
    assert_eq!(*events.borrow(), vec!["a:lost".to_string()]);
}

// ── Phase 23: hover state + ancestor bubble ───────────────────────

/// Test widget that records every MouseEnter / MouseLeave so the
/// hover-bubble path can be asserted at the test level. Mirrors
/// FocusProbe's shape; lives at a fixed 10x10 so layout is trivial.
struct HoverProbe {
    events: Rc<RefCell<Vec<String>>>,
    tag: &'static str,
}

impl HoverProbe {
    fn new(tag: &'static str, events: Rc<RefCell<Vec<String>>>) -> Self {
        Self { events, tag }
    }
}

impl shroud_widgets::Widget for HoverProbe {
    fn style(&self) -> shroud_layout::FlexStyle {
        shroud_layout::FlexStyle::new().width(40.0).height(40.0)
    }
    fn paint(&self, _: shroud_core::Rect, _: &mut PaintContext) {}
    fn event(
        &mut self,
        event: &WidgetEvent,
        _: shroud_core::Rect,
        _: &mut EventContext,
    ) -> EventResult {
        match event {
            WidgetEvent::MouseEnter => {
                self.events.borrow_mut().push(format!("{}:enter", self.tag));
                EventResult::Ignored
            }
            WidgetEvent::MouseLeave => {
                self.events.borrow_mut().push(format!("{}:leave", self.tag));
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }
}

#[test]
fn theme_dark_and_light_provide_hover_tokens() {
    // Just a smoke test — we don't want to lock in exact RGB values, but
    // both themes must populate `hover` and the bg must differ from each
    // theme's plain surface (otherwise hover is invisible against it).
    let dark = Theme::dark();
    let light = Theme::light();
    assert_ne!(dark.hover.bg, dark.colors.surface);
    assert_ne!(light.hover.bg, light.colors.surface);
}

#[test]
fn hoverable_container_paints_theme_hover_bg_when_hovered() {
    // Container with `.hoverable()` and no resting bg must produce a
    // hover-bg rect the moment the cursor lands inside it.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(200.0)
            .height(200.0)
            .padding(10.0)
            .hoverable()
            // Instant transition isolates the color-selection logic under
            // test from the (time-driven) default hover fade.
            .hover_transition(std::time::Duration::ZERO),
    );
    tree.compute_layout(200.0, 200.0);
    let root_rect = tree.layout_rect(root);

    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(root_rect.origin.x + 5.0, root_rect.origin.y + 5.0),
        },
        &mut event_ctx,
    );

    let theme = Theme::default();
    let mut paint = PaintContext::new(theme.clone());
    tree.paint(&mut paint);

    assert_eq!(paint.rects.len(), 1, "hoverable bg must paint exactly once");
    assert_eq!(paint.rects[0].color, theme.hover.bg);
}

#[test]
fn container_without_hoverable_paints_nothing_on_hover() {
    // Mirror image of the previous test — a plain Container with no
    // background and no `.hoverable()` opt-in must stay invisible no
    // matter where the cursor sits. Guards against accidentally turning
    // every container into a hover target.
    let mut tree = WidgetTree::new();
    let _root = tree.set_root(Container::column().width(200.0).height(200.0));
    tree.compute_layout(200.0, 200.0);

    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(50.0, 50.0),
        },
        &mut event_ctx,
    );

    let mut paint = PaintContext::default();
    tree.paint(&mut paint);
    assert!(
        paint.rects.is_empty(),
        "non-hoverable container with no bg must not paint"
    );
}

#[test]
fn hover_background_overrides_theme_default() {
    // Explicit override must win even when the theme provides a hover
    // token — the override is the user's "this row's hover doesn't
    // match the theme" escape hatch.
    let override_color = Color::rgb(0.9, 0.1, 0.1);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(100.0)
            .height(100.0)
            .hover_background(override_color)
            .hover_transition(std::time::Duration::ZERO),
    );
    tree.compute_layout(100.0, 100.0);
    let root_rect = tree.layout_rect(root);

    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(root_rect.origin.x + 5.0, root_rect.origin.y + 5.0),
        },
        &mut event_ctx,
    );

    let mut paint = PaintContext::default();
    tree.paint(&mut paint);
    assert_eq!(paint.rects.len(), 1);
    assert_eq!(paint.rects[0].color, override_color);
}

#[test]
fn hover_fade_default_animates_and_requests_frames() {
    // B-8 wiring: with the default (non-zero) transition, the first paint
    // after entering a hoverable container must not have arrived at the
    // hover color yet — it must be mid-fade and vote for another frame so
    // the event loop keeps pumping until the fade settles.
    use shroud_reactive::animation::{frame_requested, reset_frame_request};

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(100.0).height(100.0).hoverable());
    tree.compute_layout(100.0, 100.0);
    let root_rect = tree.layout_rect(root);

    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(root_rect.origin.x + 5.0, root_rect.origin.y + 5.0),
        },
        &mut event_ctx,
    );

    reset_frame_request();
    let mut paint = PaintContext::default();
    tree.paint(&mut paint);
    assert!(
        frame_requested(),
        "an in-flight hover fade must vote for another frame"
    );
}

#[test]
fn hover_fade_zero_duration_flips_both_ways() {
    // With an instant transition, a hoverable container with a resting
    // background snaps to the hover color on enter and back to the resting
    // color on leave — covering the leave path the bubble tests don't paint.
    let resting = Color::rgb(0.1, 0.1, 0.1);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(100.0)
            .height(100.0)
            .background(resting)
            .hoverable()
            .hover_transition(std::time::Duration::ZERO),
    );
    tree.compute_layout(100.0, 100.0);
    let root_rect = tree.layout_rect(root);
    let mut event_ctx = EventContext::new();
    let theme = Theme::default();

    // Enter → hover color.
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(root_rect.origin.x + 5.0, root_rect.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    let mut p1 = PaintContext::new(theme.clone());
    tree.paint(&mut p1);
    assert_eq!(p1.rects[0].color, theme.hover.bg, "enter → hover color");

    // Leave (move far outside) → back to the resting color.
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(999.0, 999.0),
        },
        &mut event_ctx,
    );
    let mut p2 = PaintContext::new(theme.clone());
    tree.paint(&mut p2);
    assert_eq!(p2.rects[0].color, resting, "leave → resting color");
}

#[test]
fn button_hover_fade_zero_duration_and_press_is_instant() {
    // Button mirrors the container fade between normal and hover, but the
    // pressed state always overrides instantly (a press must read as
    // immediate). Driven with a zero-duration transition for determinism.
    let normal = Color::rgb(0.1, 0.1, 0.1);
    let hover = Color::rgb(0.2, 0.2, 0.2);
    let press = Color::rgb(0.3, 0.3, 0.3);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(120.0).height(80.0));
    let btn = tree.add_child(
        root,
        Button::new("Go")
            .background(normal)
            .hover_background(hover)
            .press_background(press)
            .hover_transition(std::time::Duration::ZERO)
            .on_click(|_| {}),
    );
    tree.compute_layout(120.0, 80.0);
    let r = tree.layout_rect(btn);
    let mut event_ctx = EventContext::new();
    let theme = Theme::default();

    let paint_bg = |tree: &mut WidgetTree, theme: &Theme| {
        let mut p = PaintContext::new(theme.clone());
        tree.paint(&mut p);
        p.rects[0].color
    };

    // Resting → normal.
    assert_eq!(paint_bg(&mut tree, &theme), normal);

    // Enter → hover.
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(r.origin.x + 5.0, r.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(paint_bg(&mut tree, &theme), hover, "enter → hover");

    // Press → press color, instantly (no fade).
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            button: MouseButton::Left,
            position: Point::new(r.origin.x + 5.0, r.origin.y + 5.0),
        },
        &mut event_ctx,
    );
    assert_eq!(paint_bg(&mut tree, &theme), press, "press → press color");
}

#[test]
fn mouse_enter_leave_bubble_to_ancestors() {
    // Probe in: outer → middle → leaf. Cursor enters the leaf only;
    // hover must bubble up so middle and outer also see MouseEnter.
    // Order matters too — outer first, then middle, then leaf — so a
    // parent's state is set before its child reacts.
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut tree = WidgetTree::new();
    let outer = tree.set_root(Container::column().width(200.0).height(200.0).padding(10.0));
    let middle_probe = tree.add_child(outer, HoverProbe::new("middle", events.clone()));
    // Wrap a leaf probe inside middle by re-parenting via add_child to
    // middle_probe. HoverProbe has fixed size; layout will place leaf
    // inside middle's box.
    let leaf_probe = tree.add_child(middle_probe, HoverProbe::new("leaf", events.clone()));
    tree.compute_layout(200.0, 200.0);

    let leaf_rect = tree.layout_rect(leaf_probe);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(leaf_rect.origin.x + 1.0, leaf_rect.origin.y + 1.0),
        },
        &mut event_ctx,
    );

    // Bubble emits MouseEnter outer-most first.
    assert_eq!(
        *events.borrow(),
        vec!["middle:enter".to_string(), "leaf:enter".to_string()]
    );

    // Move cursor away — bubble emits MouseLeave leaf first, then
    // ancestors in leaf-up order.
    events.borrow_mut().clear();
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(999.0, 999.0),
        },
        &mut event_ctx,
    );
    assert_eq!(
        *events.borrow(),
        vec!["leaf:leave".to_string(), "middle:leave".to_string()]
    );
}

#[test]
fn hover_bubble_skips_common_ancestor_when_moving_within_subtree() {
    // Cursor moves from one sibling to another under the same hoverable
    // parent. The parent must NOT receive a redundant leave→enter pair
    // — that would cause a parent's hover bg to flash off-and-on as the
    // user moves between rows.
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut tree = WidgetTree::new();
    let outer = tree.set_root(Container::row().width(400.0).height(60.0));
    let parent = tree.add_child(outer, HoverProbe::new("parent", events.clone()));
    let leaf_a = tree.add_child(parent, HoverProbe::new("a", events.clone()));
    let leaf_b = tree.add_child(parent, HoverProbe::new("b", events.clone()));
    tree.compute_layout(400.0, 60.0);

    let rect_a = tree.layout_rect(leaf_a);
    let rect_b = tree.layout_rect(leaf_b);
    let mut event_ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(rect_a.origin.x + 1.0, rect_a.origin.y + 1.0),
        },
        &mut event_ctx,
    );
    // Parent + a got entered.
    assert_eq!(
        *events.borrow(),
        vec!["parent:enter".to_string(), "a:enter".to_string()]
    );
    events.borrow_mut().clear();

    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(rect_b.origin.x + 1.0, rect_b.origin.y + 1.0),
        },
        &mut event_ctx,
    );
    // Only the leaves flipped — parent stayed quietly hovered.
    assert_eq!(
        *events.borrow(),
        vec!["a:leave".to_string(), "b:enter".to_string()]
    );
}

// ── Input: multi-line (Phase 25, A-2) ────────────────────────────

/// Build a 400×300 tree, install a single Input as the root's only child,
/// bind it to `sig`, focus it, and return `(tree, idx, event_ctx, sig)`.
/// `multiline=true` flips the textarea behavior on. Cursor lands at the
/// end of `sig`'s current value after the focus click.
fn build_input_with_signal(
    initial: &str,
    multiline: bool,
) -> (WidgetTree, usize, EventContext, Signal<String>) {
    let sig = Signal::new(String::from(initial));
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let mut input = Input::new().value(sig);
    if multiline {
        input = input.multiline().lines(4);
    }
    let idx = tree.add_child(root, input);
    tree.compute_layout(400.0, 300.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    (tree, idx, event_ctx, sig)
}

fn key(named: shroud_widgets::event::NamedKey) -> WidgetEvent {
    WidgetEvent::KeyDown {
        key: shroud_widgets::event::Key::Named(named),
    }
}

/// Cursor-position assertion via a side-effect probe. Cursor state isn't
/// observable through the public Widget API, so we type a marker char and
/// check where it lands in the bound signal: if the cursor was at byte N,
/// the signal goes from `<prefix><suffix>` to `<prefix>M<suffix>`.
fn assert_cursor_inserts_at(
    tree: &mut WidgetTree,
    ctx: &mut EventContext,
    sig: &Signal<String>,
    expected_after: &str,
) {
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'M' }, ctx);
    assert_eq!(sig.get_clone(), expected_after);
}

#[test]
fn multiline_enter_inserts_newline_into_signal() {
    let (mut tree, idx, mut ctx, sig) = build_input_with_signal("", true);

    for ch in ['a', 'b'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ctx);
    }
    tree.dispatch_event(&key(shroud_widgets::event::NamedKey::Enter), &mut ctx);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'c' }, &mut ctx);

    assert_eq!(sig.get_clone(), "ab\nc");
    // Bonus: the index is still valid (Enter didn't replace the widget).
    assert!(tree.contains(idx));
}

#[test]
fn multiline_enter_does_not_fire_on_submit() {
    let submitted = Rc::new(Cell::new(false));
    let submitted2 = submitted.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let idx = tree.add_child(
        root,
        Input::new()
            .multiline()
            .on_submit(move |_text, _ctx| submitted2.set(true)),
    );
    tree.compute_layout(400.0, 300.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&key(shroud_widgets::event::NamedKey::Enter), &mut event_ctx);

    assert!(
        !submitted.get(),
        "on_submit must stay quiet in multi-line mode — Enter is a newline there"
    );
}

#[test]
fn single_line_enter_still_fires_on_submit() {
    // Regression guard: the multi-line branch must not bleed into the
    // default single-line behavior.
    let submitted = Rc::new(Cell::new(false));
    let submitted2 = submitted.clone();

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(
        root,
        Input::new().on_submit(move |_text, _ctx| submitted2.set(true)),
    );
    tree.compute_layout(400.0, 100.0);

    let rect = tree.layout_rect(idx);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    tree.dispatch_event(&key(shroud_widgets::event::NamedKey::Enter), &mut event_ctx);

    assert!(submitted.get());
    let _ = idx; // silence dead-code warning
}

// Multi-line ArrowUp/Down navigation moved to *visual*-row + sticky-x semantics
// (FW-2): the move now needs the text engine and is resolved in `paint`, so the
// old synchronous, char-column, no-paint probes here no longer model it. The
// behavior is pinned in `tests/input_vnav_spike.rs` (engine + paint), including
// the soft-wrap case the old hard-line model couldn't express.

#[test]
fn single_line_arrow_up_down_are_no_ops() {
    // ArrowUp/Down must do nothing in single-line mode. Probe 'M' lands
    // at end of "hello" — proves cursor stayed put.
    let (mut tree, _idx, mut ctx, sig) = build_input_with_signal("hello", false);

    tree.dispatch_event(&key(shroud_widgets::event::NamedKey::ArrowUp), &mut ctx);
    tree.dispatch_event(&key(shroud_widgets::event::NamedKey::ArrowDown), &mut ctx);

    assert_cursor_inserts_at(&mut tree, &mut ctx, &sig, "helloM");
}

#[test]
fn multiline_min_height_scales_with_lines() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(800.0));
    let single = tree.add_child(root, Input::new());
    let multi5 = tree.add_child(root, Input::new().multiline().lines(5));
    tree.compute_layout(400.0, 800.0);

    let h_single = tree.layout_rect(single).size.height;
    let h_multi = tree.layout_rect(multi5).size.height;
    assert!(
        h_multi > h_single * 2.0,
        "lines(5) multi-line Input must be substantially taller than the single-line default \
         (got multi={h_multi}, single={h_single})"
    );
}

#[test]
fn multiline_does_not_intercept_tab() {
    // Two multi-line Inputs in a row. Tab from the first should advance
    // focus to the second — the focus manager owns Tab, the widget must
    // not consume it.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(400.0));
    let first = tree.add_child(root, Input::new().multiline());
    let second = tree.add_child(root, Input::new().multiline());
    tree.compute_layout(400.0, 400.0);

    let rect = tree.layout_rect(first);
    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut event_ctx,
    );
    assert_eq!(tree.focused(), Some(first));

    tree.dispatch_event(&key(shroud_widgets::event::NamedKey::Tab), &mut event_ctx);
    assert_eq!(
        tree.focused(),
        Some(second),
        "Tab inside a multi-line Input must still advance focus"
    );
}

#[test]
fn hoverable_container_lights_up_when_cursor_is_in_child_button() {
    // The canonical "list row contains an action button" case. Without
    // ancestor bubbling, the row would never see MouseEnter because the
    // Button is the deepest hit and consumes the event-walk return.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let row = tree.add_child(
        root,
        Container::row()
            .padding(10.0)
            .gap(8.0)
            .align_center()
            .hoverable()
            .hover_transition(std::time::Duration::ZERO),
    );
    let btn = tree.add_child(row, Button::new("Delete").on_click(|_| {}));
    tree.compute_layout(400.0, 200.0);
    let btn_rect = tree.layout_rect(btn);

    let mut event_ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(btn_rect.origin.x + 5.0, btn_rect.origin.y + 5.0),
        },
        &mut event_ctx,
    );

    let theme = Theme::default();
    let mut paint = PaintContext::new(theme.clone());
    tree.paint(&mut paint);

    // Paint order: container row first, button bg second. The row's
    // rect (rects[0]) must use the hover tint even though the cursor
    // is on the button.
    assert!(
        paint.rects.len() >= 2,
        "expected row bg + button bg, got {}",
        paint.rects.len()
    );
    assert_eq!(
        paint.rects[0].color, theme.hover.bg,
        "row bg should be theme.hover.bg when cursor is in inner button"
    );
}

// ── Truncate / wrap (Phase 26) ────────────────────────────────────

const LONG_LATIN: &str =
    "This is a deliberately overlong sentence that should not fit inside a narrow box.";

#[test]
fn truncate_false_is_default_and_still_wraps() {
    // Baseline for A-9: a TextWidget with no truncate flag, dropped into a
    // narrow column, must wrap onto multiple visual lines. Guards the
    // wrap-by-default behavior that we are not adding a knob for.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(120.0).height(400.0));
    let text_idx = tree.add_child(root, TextWidget::new(LONG_LATIN).font_size(16.0));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(120.0, 400.0, &mut engine, &theme);

    let rect = tree.layout_rect(text_idx);
    let line_height = theme.typography.body.line_height;
    assert!(
        rect.size.height > line_height * 1.5,
        "wrap-by-default should produce multi-line height; \
         got {} for line_height {}",
        rect.size.height,
        line_height,
    );
}

#[test]
fn text_wrap_kicks_in_when_narrower_than_natural() {
    // Companion to the test above — directly verifies that paint emits
    // glyphs on more than one Y row when the available width is narrower
    // than the natural shaped width. A-9 regression guard.
    use shroud_core::Theme;
    use shroud_text::TextEngine;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(80.0).height(400.0));
    let _text_idx = tree.add_child(
        root,
        TextWidget::new("Hello wrap world test").font_size(16.0),
    );

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(80.0, 400.0, &mut engine, &theme);

    let mut ctx = PaintContext::new(theme);
    tree.paint(&mut ctx);

    let unique_ys: std::collections::BTreeSet<i32> = ctx.glyphs.iter().map(|g| g.y).collect();
    assert!(
        unique_ys.len() >= 2,
        "wrapped text should produce glyphs on at least two baselines; \
         got {} unique y-rows",
        unique_ys.len(),
    );
}

#[test]
fn truncate_true_short_text_fits_in_one_line_with_no_ellipsis() {
    // Truncate-on, but the text already fits — must not append ellipsis,
    // and must produce the same glyph count as natural shaping.
    use shroud_core::Rect as CoreRect;
    use shroud_core::Theme;

    let theme = Theme::default();
    let widget = TextWidget::new("Hi").font_size(16.0).truncate(true);
    let mut ctx = PaintContext::new(theme.clone());

    // Wide enough to obviously fit "Hi".
    let layout = CoreRect::new(0.0, 0.0, 400.0, 22.0);
    <TextWidget as Widget>::paint(&widget, layout, &mut ctx);

    let natural = ctx.text_engine.shape_text("Hi", 16.0, 22.0, None);
    assert_eq!(
        ctx.glyphs.len(),
        natural.glyphs.len(),
        "truncate-on with text that fits must paint exactly the natural glyph stream"
    );

    let ellipsis = ctx.text_engine.shape_text("\u{2026}", 16.0, 22.0, None);
    let ellipsis_gid = ellipsis.glyphs.first().map(|g| g.cache_key.glyph_id);
    if let Some(gid) = ellipsis_gid {
        assert!(
            !ctx.glyphs.iter().any(|g| g.cache_key.glyph_id == gid),
            "no ellipsis glyph should appear when the text already fits"
        );
    }
}

#[test]
fn truncate_true_long_text_appends_ellipsis() {
    use shroud_core::Rect as CoreRect;
    use shroud_core::Theme;

    let theme = Theme::default();
    let widget = TextWidget::new(LONG_LATIN).font_size(16.0).truncate(true);
    let mut ctx = PaintContext::new(theme);

    let layout = CoreRect::new(0.0, 0.0, 120.0, 22.0);
    <TextWidget as Widget>::paint(&widget, layout, &mut ctx);

    // cache_key carries x_bin subpixel state, so two `…` glyphs at different
    // positions don't compare equal. Match on `glyph_id` instead (the font's
    // glyph index, which is position-independent).
    let ellipsis_only = ctx.text_engine.shape_text("\u{2026}", 16.0, 22.0, None);
    let ellipsis_gid = ellipsis_only
        .glyphs
        .first()
        .map(|g| g.cache_key.glyph_id)
        .expect("ellipsis must produce at least one glyph in the system font");

    assert!(
        ctx.glyphs
            .iter()
            .any(|g| g.cache_key.glyph_id == ellipsis_gid),
        "truncated overflow should include the ellipsis glyph in the paint stream"
    );

    // And the last glyph should be the ellipsis (truncate appends at the tail).
    let last_gid = ctx.glyphs.last().map(|g| g.cache_key.glyph_id);
    assert_eq!(
        last_gid,
        Some(ellipsis_gid),
        "ellipsis must be the trailing glyph for tail truncation"
    );

    // And we actually painted less than the full natural string would (proof
    // that the prefix was clipped, not just rendered).
    let natural = ctx.text_engine.shape_text(LONG_LATIN, 16.0, 22.0, None);
    assert!(
        ctx.glyphs.len() < natural.glyphs.len(),
        "truncated paint must drop some prefix glyphs (got {} of {} natural)",
        ctx.glyphs.len(),
        natural.glyphs.len(),
    );
}

#[test]
fn truncate_true_measure_returns_single_line_height() {
    use shroud_text::TextEngine;

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    let widget = TextWidget::new(LONG_LATIN).font_size(16.0).truncate(true);

    let mut ctx = MeasureContext::new(&mut engine, &theme);
    let size = <TextWidget as Widget>::measure(&widget, Some(120.0), &mut ctx)
        .expect("measure should report Some");

    let line_height = theme.typography.body.line_height.ceil();
    assert_eq!(
        size.height, line_height,
        "truncated widget must report exactly one line of height \
         (got {}, expected {})",
        size.height, line_height,
    );
    assert!(
        size.width <= 120.0,
        "truncated width must not exceed available_width; got {}",
        size.width,
    );
}

#[test]
fn truncate_true_long_text_does_not_overflow_layout_width() {
    use shroud_core::Rect as CoreRect;
    use shroud_core::Theme;

    let theme = Theme::default();
    let widget = TextWidget::new(LONG_LATIN).font_size(16.0).truncate(true);
    let mut ctx = PaintContext::new(theme);

    let layout = CoreRect::new(10.0, 5.0, 120.0, 22.0);
    <TextWidget as Widget>::paint(&widget, layout, &mut ctx);

    // The drawn glyphs' right edges (relative to the paint stream) must
    // stay within layout's right edge. Allow 1px of slop for sub-pixel
    // rounding.
    let right_edge = layout.origin.x + layout.size.width + 1.0;
    let max_right = ctx
        .glyphs
        .iter()
        .map(|g| g.x as f32 + g.image.width as f32 + g.image.left as f32)
        .fold(0.0_f32, f32::max);
    assert!(
        max_right <= right_edge,
        "truncated glyphs must not paint past layout right edge \
         (max_right={}, allowed={})",
        max_right,
        right_edge,
    );

    // Every glyph in the stream should also carry the layout clip — defense
    // in depth via PaintContext::push_clip.
    for g in &ctx.glyphs {
        assert_eq!(
            g.clip_rect,
            Some(layout),
            "every truncated glyph must record the layout rect as its clip"
        );
    }
}

#[test]
fn truncate_true_with_cjk_text_walks_char_boundary() {
    use shroud_core::Rect as CoreRect;
    use shroud_core::Theme;

    let theme = Theme::default();
    // 5 repeats of 日本語 = 15 multi-byte chars, no ASCII fast path.
    let widget = TextWidget::new("日本語日本語日本語日本語日本語")
        .font_size(16.0)
        .truncate(true);
    let mut ctx = PaintContext::new(theme);

    let layout = CoreRect::new(0.0, 0.0, 80.0, 22.0);
    // The original byte-vs-char-index bug would `panic!` inside `&text[..end]`
    // when `end` landed mid-character. The primary value of this test is that
    // paint succeeds at all — no panic, no UTF-8 slice violation.
    <TextWidget as Widget>::paint(&widget, layout, &mut ctx);

    // Secondary: prove the prefix was actually trimmed. Comparing glyph_id
    // against `shape_text("\u{2026}")` alone is fragile because cosmic-text
    // can pick a different (Japanese-fallback) font for the ellipsis when
    // it sits between CJK chars vs. on its own — so the glyph_id differs.
    // Just check that we emitted fewer glyphs than the natural full string.
    let natural = ctx
        .text_engine
        .shape_text("日本語日本語日本語日本語日本語", 16.0, 22.0, None);
    assert!(
        ctx.glyphs.len() < natural.glyphs.len(),
        "CJK truncate must drop some prefix; got {} of {} natural",
        ctx.glyphs.len(),
        natural.glyphs.len(),
    );
}

#[test]
fn truncate_true_with_zero_width_paints_nothing() {
    use shroud_core::Rect as CoreRect;
    use shroud_core::Theme;

    let theme = Theme::default();
    let widget = TextWidget::new("anything").font_size(16.0).truncate(true);
    let mut ctx = PaintContext::new(theme);

    let layout = CoreRect::new(0.0, 0.0, 0.0, 22.0);
    <TextWidget as Widget>::paint(&widget, layout, &mut ctx);

    assert!(
        ctx.glyphs.is_empty(),
        "truncate at zero width must paint no glyphs (graceful, not panic); \
         got {} glyphs",
        ctx.glyphs.len(),
    );
}

// ── accepts_text predicate (Phase 27 / A-11) ──────────────────────

#[test]
fn input_and_secure_input_accept_text() {
    // The shortcut router uses `accepts_text` to decide whether a
    // default-scope (WhenNoTextInput) binding fires while a widget has
    // focus. Inputs must opt in; everything else (Button/Checkbox/
    // Container) must keep the default `false` so e.g. Ctrl+N still
    // works while a button is focused.
    let input = Input::new();
    let secure = SecureInput::new();
    let button = Button::new("ok");
    let checkbox = Checkbox::new("opt");
    let container = Container::column();

    assert!(Widget::accepts_text(&input));
    assert!(Widget::accepts_text(&secure));
    assert!(!Widget::accepts_text(&button));
    assert!(!Widget::accepts_text(&checkbox));
    assert!(!Widget::accepts_text(&container));
}

#[test]
fn shortcut_fires_through_tree_with_no_focus() {
    use shroud_widgets::event::{Key, Modifiers, WidgetEvent};
    use shroud_widgets::shortcut::Shortcut;

    let mut tree = WidgetTree::new();
    tree.set_root(Container::column().width(100.0).height(100.0));

    let fired = Rc::new(Cell::new(false));
    let f = fired.clone();
    tree.shortcut_router_mut()
        .register(Shortcut::ctrl('l'), move |_| f.set(true));

    let mut ctx = EventContext::new();
    ctx.modifiers = Modifiers::CTRL;
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Character('l'),
        },
        &mut ctx,
    );

    assert!(fired.get());
}

#[test]
fn shortcut_handler_can_replace_screen() {
    // End-to-end check: a shortcut handler enqueues a TreeCommand
    // (replace_screen) and the post-dispatch drain runs it. Without
    // this the AppScope::on_shortcut DX is hollow — handlers couldn't
    // actually mutate the tree they were registered against.
    use shroud_widgets::event::{Key, Modifiers, WidgetEvent};
    use shroud_widgets::shortcut::Shortcut;

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(100.0).height(100.0));
    tree.add_child(root, TextWidget::new("before"));

    tree.shortcut_router_mut()
        .register(Shortcut::ctrl('l'), |ctx| {
            ctx.event_ctx.replace_screen(|t| {
                t.set_root(Container::column().width(100.0).height(100.0));
            });
        });

    let initial_len = tree.len();
    let mut ctx = EventContext::new();
    ctx.modifiers = Modifiers::CTRL;
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Character('l'),
        },
        &mut ctx,
    );

    // After drain: the old root + its child are tombstoned, a fresh root
    // is in place. len() counts live nodes only, so it should be 1.
    assert_ne!(tree.len(), initial_len);
}

// ── Phase 28: Input numeric mode + Signal<i64> binding ─────────────

/// Helper: focus an input by clicking inside its rect so subsequent
/// CharInput events are accepted (Input only commits when `self.focused`).
fn focus_input(tree: &mut WidgetTree, idx: usize, event_ctx: &mut EventContext) {
    let rect = tree.layout_rect(idx);
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        event_ctx,
    );
}

#[test]
fn input_numeric_filters_non_digit_chars() {
    // In numeric mode, only ASCII digits should land in the buffer.
    // Letters / punctuation / whitespace are dropped silently — same model
    // as HTML <input type="number"> rejecting non-numeric typing.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().numeric());
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut event_ctx);

    // Mix of digits and non-digits: only the digits should commit.
    for ch in ['1', 'a', '2', '!', '3', ' ', '4'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    let input = tree
        .widget_as::<Input>(idx)
        .expect("widget should still be Input");
    assert_eq!(
        input.value_clone(),
        "1234",
        "numeric mode must drop non-digit CharInput events"
    );
}

#[test]
fn input_number_value_seeds_buffer_and_enables_numeric() {
    // number_value() seeds the buffer with the signal's current value
    // (rendered as a decimal int) and implicitly enables numeric mode.
    let sig = Signal::new(42i64);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().number_value(sig));
    tree.compute_layout(400.0, 100.0);

    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.value_clone(), "42", "buffer seeds from Signal<i64>");

    // Implicit numeric: letters should be dropped even though caller
    // never wrote `.numeric()`.
    let mut event_ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut event_ctx);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'x' }, &mut event_ctx);
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(
        input.value_clone(),
        "42",
        "letters rejected after number_value()"
    );
}

#[test]
fn input_numeric_pushes_parsed_value_to_signal() {
    // Typing a digit at the end of the buffer should parse-and-write the
    // freshly-parsed integer to the bound signal on every keystroke.
    let sig = Signal::new(0i64);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().number_value(sig));
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut event_ctx);
    // MouseDown moves cursor to end of buffer ("0"), so digits append.
    for ch in ['7', '2'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }

    assert_eq!(sig.get(), 72, "signal must track each parsed buffer state");
}

#[test]
fn input_numeric_clamps_to_max_on_edit() {
    // Typing past max_value should clamp the *signal* even though the
    // buffer keeps showing the typed digits (snap-back happens on FocusLost).
    let sig = Signal::new(5i64);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(
        root,
        Input::new().min_value(1).max_value(10).number_value(sig),
    );
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut event_ctx);
    // Cursor at end of "5" → typing "9" makes buffer "59" → 59 clamped to 10.
    tree.dispatch_event(&WidgetEvent::CharInput { ch: '9' }, &mut event_ctx);

    assert_eq!(sig.get(), 10, "out-of-range edit must clamp signal to max");
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(
        input.value_clone(),
        "59",
        "buffer stays free-form mid-edit; canonicalize is FocusLost's job"
    );
}

#[test]
fn input_numeric_clamps_to_min_via_focus_lost_when_buffer_empty() {
    // User clears the buffer fully. The signal should not flap to 0 on
    // each backspace (parse fails on ""), and on FocusLost the buffer
    // should snap back to the canonical render of the last valid signal
    // value clamped to min.
    let sig = Signal::new(7i64);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(
        root,
        Input::new().min_value(1).max_value(100).number_value(sig),
    );
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut event_ctx);
    // Clear the buffer ("7" → ""). signal must not move to 0.
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Backspace),
        },
        &mut event_ctx,
    );
    assert_eq!(
        sig.get(),
        7,
        "empty buffer keeps signal at last parsed value"
    );

    // FocusLost should re-render the signal value into the buffer.
    tree.dispatch_event(&WidgetEvent::FocusLost, &mut event_ctx);
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(
        input.value_clone(),
        "7",
        "FocusLost canonicalizes buffer from signal"
    );
}

#[test]
fn input_numeric_canonicalizes_leading_zeros_on_focus_lost() {
    // Buffer "007" is a valid parse (= 7) and the signal already holds 7.
    // FocusLost should normalize the visible buffer to "7" by re-rendering
    // the signal value.
    let sig = Signal::new(0i64);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().number_value(sig));
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut event_ctx);
    // Buffer starts as "0", cursor at end. Typing "07" → "007", parses to 7.
    for ch in ['0', '7'] {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
    }
    assert_eq!(sig.get(), 7);

    tree.dispatch_event(&WidgetEvent::FocusLost, &mut event_ctx);
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(
        input.value_clone(),
        "7",
        "leading zeros stripped on canonicalize"
    );
}

#[test]
fn input_numeric_external_signal_set_rebases_when_unfocused() {
    // While the field is unfocused, paint should re-render an externally
    // set signal value. Mirror of the Signal<String> "external_signal_set"
    // test, but for the typed binding.
    let sig = Signal::new(1i64);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().number_value(sig));
    tree.compute_layout(400.0, 100.0);

    // Initial paint to prime sync.
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "1");

    // External write — widget is unfocused, so the next paint must rebase.
    sig.set(999);
    let mut ctx2 = PaintContext::default();
    tree.paint(&mut ctx2);
    assert_eq!(
        tree.widget_as::<Input>(idx).unwrap().value_clone(),
        "999",
        "unfocused widget must rebase from external signal write"
    );
}

#[test]
fn input_numeric_external_signal_does_not_clobber_focused_buffer() {
    // While focused, the user's mid-edit buffer must survive an external
    // signal write. Otherwise the user could be typing and have the field
    // jump out from under them (e.g. an on_frame handler nudging the value).
    let sig = Signal::new(1i64);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().number_value(sig));
    tree.compute_layout(400.0, 100.0);

    let mut event_ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut event_ctx);
    // Type a digit so the buffer diverges from the signal in a typed-by-user way.
    tree.dispatch_event(&WidgetEvent::CharInput { ch: '5' }, &mut event_ctx);
    // Now buffer = "15", signal = 15.
    assert_eq!(sig.get(), 15);

    // External overwrite while focused — the buffer should NOT snap.
    sig.set(7);
    let mut ctx = PaintContext::default();
    tree.paint(&mut ctx);
    assert_eq!(
        tree.widget_as::<Input>(idx).unwrap().value_clone(),
        "15",
        "external signal write must not clobber a focused user buffer"
    );
}

#[test]
fn input_numeric_min_clamp_on_edit_only() {
    // Only setting min_value (no max) should clamp the lower side via
    // user edits. The displayed value mirrors the signal verbatim — no
    // construct-time or paint-time clamp — so an initial out-of-range
    // signal value renders honestly until the user edits.
    let sig = Signal::new(0i64);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().min_value(10).number_value(sig));
    tree.compute_layout(400.0, 100.0);

    // Verbatim seed: signal=0 → buffer "0", no silent snap to min.
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.value_clone(), "0", "buffer renders signal raw");
    assert_eq!(sig.get(), 0);

    // First edit triggers the min clamp. Cursor is at end → typing "5"
    // makes buffer "05" → parses to 5 → clamped up to 10.
    let mut event_ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut event_ctx);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: '5' }, &mut event_ctx);
    assert_eq!(sig.get(), 10, "min_value clamps on edit (no max set)");

    // Defocus canonicalizes the buffer to the signal's decimal form.
    tree.dispatch_event(&WidgetEvent::FocusLost, &mut event_ctx);
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.value_clone(), "10");
}

// ── Input text selection model ─────────────────────────────────────
//
// Keyboard selection / clipboard tests drive the state machine through
// `dispatch_event` with no paint pass. The precise click-to-caret and
// drag-select paths resolve at paint time (the widget has no text engine in
// `event`) and are covered by the engine's `offset_at_point` round-trip
// tests; here we seed the caret with `with_value` (caret lands at the end).

/// Build a focused single-line Input seeded with `text` (caret at end).
fn selection_input(text: &str) -> (WidgetTree, usize, EventContext) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().with_value(text));
    tree.compute_layout(400.0, 100.0);
    let mut ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut ctx);
    (tree, idx, ctx)
}

/// Dispatch a KeyDown with `mods` held, then reset modifiers so a following
/// CharInput isn't accidentally treated as a chord.
fn dispatch_key(tree: &mut WidgetTree, key: Key, mods: Modifiers, ctx: &mut EventContext) {
    ctx.modifiers = mods;
    tree.dispatch_event(&WidgetEvent::KeyDown { key }, ctx);
    ctx.modifiers = Modifiers::NONE;
}

#[test]
fn shift_arrow_left_extends_selection() {
    let (mut tree, idx, mut ctx) = selection_input("hello");
    // Caret at end (5). Shift+Left twice selects the last two chars.
    for _ in 0..2 {
        dispatch_key(
            &mut tree,
            Key::Named(NamedKey::ArrowLeft),
            Modifiers::SHIFT,
            &mut ctx,
        );
    }
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert!(input.has_selection());
    assert_eq!(input.selected_text().as_deref(), Some("lo"));
}

#[test]
fn shift_arrow_selects_multibyte_char() {
    // Selection must split on char boundaries: each kana is 3 UTF-8 bytes.
    let (mut tree, idx, mut ctx) = selection_input("あい");
    dispatch_key(
        &mut tree,
        Key::Named(NamedKey::ArrowLeft),
        Modifiers::SHIFT,
        &mut ctx,
    );
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.selected_text().as_deref(), Some("い"));
}

#[test]
fn home_then_shift_end_selects_whole_line() {
    let (mut tree, idx, mut ctx) = selection_input("hello");
    dispatch_key(
        &mut tree,
        Key::Named(NamedKey::Home),
        Modifiers::NONE,
        &mut ctx,
    );
    dispatch_key(
        &mut tree,
        Key::Named(NamedKey::End),
        Modifiers::SHIFT,
        &mut ctx,
    );
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.selected_text().as_deref(), Some("hello"));
}

#[test]
fn ctrl_a_selects_all() {
    let (mut tree, idx, mut ctx) = selection_input("hello world");
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx);
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.selected_text().as_deref(), Some("hello world"));
}

#[test]
fn typing_replaces_selection() {
    let (mut tree, idx, mut ctx) = selection_input("hello");
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx);
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'Z' }, &mut ctx);
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.value_clone(), "Z");
    assert!(!input.has_selection(), "insert collapses the selection");
}

#[test]
fn backspace_deletes_selection_as_a_unit() {
    let (mut tree, idx, mut ctx) = selection_input("hello");
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx);
    dispatch_key(
        &mut tree,
        Key::Named(NamedKey::Backspace),
        Modifiers::NONE,
        &mut ctx,
    );
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.value_clone(), "");
    assert!(!input.has_selection());
}

#[test]
fn delete_key_removes_selection() {
    let (mut tree, idx, mut ctx) = selection_input("hello");
    // Select the last two chars, then Delete removes exactly that range.
    for _ in 0..2 {
        dispatch_key(
            &mut tree,
            Key::Named(NamedKey::ArrowLeft),
            Modifiers::SHIFT,
            &mut ctx,
        );
    }
    dispatch_key(
        &mut tree,
        Key::Named(NamedKey::Delete),
        Modifiers::NONE,
        &mut ctx,
    );
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.value_clone(), "hel");
    assert!(!input.has_selection());
}

#[test]
fn ctrl_c_copies_selection_to_clipboard() {
    let (mut tree, idx, mut ctx) = selection_input("hello");
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx);
    dispatch_key(&mut tree, Key::Character('c'), Modifiers::CTRL, &mut ctx);
    assert_eq!(ctx.take_clipboard_write().as_deref(), Some("hello"));
    // Copy does not mutate the buffer.
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "hello");
}

#[test]
fn ctrl_c_with_no_selection_writes_nothing() {
    let (mut tree, _idx, mut ctx) = selection_input("hello");
    dispatch_key(&mut tree, Key::Character('c'), Modifiers::CTRL, &mut ctx);
    assert!(ctx.take_clipboard_write().is_none());
}

#[test]
fn ctrl_x_cuts_selection() {
    let (mut tree, idx, mut ctx) = selection_input("hello world");
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx);
    dispatch_key(&mut tree, Key::Character('x'), Modifiers::CTRL, &mut ctx);
    assert_eq!(ctx.take_clipboard_write().as_deref(), Some("hello world"));
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "");
}

// --- Undo / redo ----------------------------------------------------------

#[test]
fn undo_reverts_a_typed_run() {
    // A burst of typing coalesces into one undo step, so a single Ctrl+Z
    // clears the whole run rather than one character at a time.
    let (mut tree, idx, mut ctx) = selection_input("");
    for ch in "abc".chars() {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ctx);
    }
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "abc");
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "");
}

#[test]
fn redo_restores_an_undone_run() {
    let (mut tree, idx, mut ctx) = selection_input("");
    for ch in "abc".chars() {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ctx);
    }
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "");
    dispatch_key(&mut tree, Key::Character('y'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "abc");
}

#[test]
fn ctrl_shift_z_also_redoes() {
    // The mac / browser redo chord. It carries Shift, so it must be matched
    // ahead of the (Shift-rejecting) command-combo arm in `Input::event`.
    let (mut tree, idx, mut ctx) = selection_input("");
    for ch in "abc".chars() {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ctx);
    }
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    dispatch_key(
        &mut tree,
        Key::Character('z'),
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        },
        &mut ctx,
    );
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "abc");
}

#[test]
fn caret_move_splits_undo_into_separate_steps() {
    // Moving the caret ends the coalescing run, so text typed before and after
    // the move are distinct undo steps.
    let (mut tree, idx, mut ctx) = selection_input("");
    for ch in "ab".chars() {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ctx);
    }
    dispatch_key(
        &mut tree,
        Key::Named(NamedKey::Home),
        Modifiers::NONE,
        &mut ctx,
    );
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'X' }, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "Xab");
    // First undo removes only the post-move insert.
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "ab");
    // Second undo removes the original run.
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "");
}

#[test]
fn deletes_coalesce_and_undo_as_one_step() {
    // A run of Backspaces is one undo step, separate from the preceding typing.
    let (mut tree, idx, mut ctx) = selection_input("");
    for ch in "abc".chars() {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ctx);
    }
    for _ in 0..2 {
        dispatch_key(
            &mut tree,
            Key::Named(NamedKey::Backspace),
            Modifiers::NONE,
            &mut ctx,
        );
    }
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "a");
    // One undo restores both deleted chars (the delete run), then another
    // restores the typed run.
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "abc");
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "");
}

#[test]
fn typing_over_a_selection_undoes_in_one_step() {
    let (mut tree, idx, mut ctx) = selection_input("hello");
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx); // select all
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'Z' }, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "Z");
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "hello");
}

#[test]
fn new_edit_after_undo_clears_the_redo_stack() {
    let (mut tree, idx, mut ctx) = selection_input("");
    for ch in "abc".chars() {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ctx);
    }
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx); // -> ""
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'x' }, &mut ctx); // forks history
    dispatch_key(&mut tree, Key::Character('y'), Modifiers::CTRL, &mut ctx); // redo is inert
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "x");
}

#[test]
fn undo_with_empty_history_is_a_noop() {
    let (mut tree, idx, mut ctx) = selection_input("");
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "");
}

#[test]
fn undo_restores_a_multibyte_snapshot() {
    // Snapshots round-trip multi-byte text without splitting a codepoint.
    let (mut tree, idx, mut ctx) = selection_input("あ");
    tree.dispatch_event(&WidgetEvent::CharInput { ch: 'い' }, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "あい");
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "あ");
}

#[test]
fn external_value_change_clears_undo_history() {
    // A note switch / programmatic set rebases the buffer; undo must not cross
    // that boundary, so Ctrl+Z afterwards is inert and the buffer keeps the
    // externally-set text.
    let sig = Signal::new(String::new());
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(sig));
    tree.compute_layout(400.0, 100.0);
    let mut ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut ctx);

    for ch in "ab".chars() {
        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut ctx);
    }
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "ab");

    // External rewrite (e.g. switching to another note's body). The next event
    // rebases the buffer and clears history, so the undo is a no-op.
    sig.set("xyz".to_string());
    dispatch_key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(tree.widget_as::<Input>(idx).unwrap().value_clone(), "xyz");
}

#[test]
fn plain_arrow_left_collapses_selection_to_left_edge() {
    let (mut tree, idx, mut ctx) = selection_input("hello");
    // Ctrl+A → anchor 0, caret 5. Plain ArrowLeft collapses to the left edge
    // (offset 0) rather than stepping one char left from the caret.
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx);
    dispatch_key(
        &mut tree,
        Key::Named(NamedKey::ArrowLeft),
        Modifiers::NONE,
        &mut ctx,
    );
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert!(!input.has_selection());
    assert_eq!(input.cursor(), 0);
}

#[test]
fn plain_arrow_right_collapses_selection_to_right_edge() {
    let (mut tree, idx, mut ctx) = selection_input("hello");
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx);
    dispatch_key(
        &mut tree,
        Key::Named(NamedKey::ArrowRight),
        Modifiers::NONE,
        &mut ctx,
    );
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert!(!input.has_selection());
    assert_eq!(input.cursor(), 5);
}

#[test]
fn selection_signal_mirrors_range_to_sibling() {
    // A sibling (e.g. a formatting toolbar) reads the live selection range via
    // a bound signal — the bridge that lets it wrap the selection in markers.
    let body = Signal::new(String::from("hello"));
    let sel = Signal::new(None);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(body).selection_signal(sel));
    tree.compute_layout(400.0, 100.0);
    let mut ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut ctx);

    assert_eq!(sel.get(), None, "nothing selected yet");
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx);
    assert_eq!(
        sel.get(),
        Some((0, 5)),
        "Ctrl+A mirrors the whole range to the sibling signal"
    );
}

#[test]
fn external_value_change_with_none_signal_clears_selection() {
    // A sibling that rewrites the value and writes `None` to the selection
    // signal clears the selection (the caret-only edit path).
    let body = Signal::new(String::from("hello"));
    let sel = Signal::new(None);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(body).selection_signal(sel));
    tree.compute_layout(400.0, 100.0);
    let mut ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut ctx);
    dispatch_key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx);
    assert!(tree.widget_as::<Input>(idx).unwrap().has_selection());

    body.set(String::from("hello world"));
    sel.set(None);
    // Any event runs `sync_from_source` first, which performs the rebase.
    tree.dispatch_event(&WidgetEvent::FocusGained, &mut ctx);
    let input = tree.widget_as::<Input>(idx).unwrap();
    assert!(!input.has_selection());
    assert_eq!(input.value_clone(), "hello world");
    assert_eq!(sel.get(), None);
}

#[test]
fn selection_signal_external_write_reselects_after_value_change() {
    // The toolbar re-select path: rewrite the value *and* write a fresh range
    // into the selection signal — the widget adopts it, so the wrapped text
    // stays selected instead of collapsing to a caret.
    let body = Signal::new(String::from("hello"));
    let sel = Signal::new(None);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(body).selection_signal(sel));
    tree.compute_layout(400.0, 100.0);
    let mut ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut ctx);

    // Simulate a toolbar wrapping "hello" → "**hello**" and re-selecting the
    // inner text, which now sits at bytes [2, 7).
    body.set(String::from("**hello**"));
    sel.set(Some((2, 7)));
    tree.dispatch_event(&WidgetEvent::FocusGained, &mut ctx);

    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.value_clone(), "**hello**");
    assert_eq!(
        input.selected_text().as_deref(),
        Some("hello"),
        "the inner text written back to the signal is re-selected"
    );
    // The caret (active end) follows the range's upper bound, and the mirror
    // settles on the adopted range.
    assert_eq!(input.cursor(), 7);
    assert_eq!(sel.get(), Some((2, 7)));
}

#[test]
fn selection_signal_external_write_snaps_to_char_boundaries() {
    // A range whose bounds fall inside multi-byte codepoints is snapped down to
    // boundaries rather than panicking the paint-side slice. "あい" is 6 bytes;
    // a write of (1, 4) snaps to (0, 3) = the first kana.
    let body = Signal::new(String::from("あい"));
    let sel = Signal::new(None);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(100.0));
    let idx = tree.add_child(root, Input::new().value(body).selection_signal(sel));
    tree.compute_layout(400.0, 100.0);
    let mut ctx = EventContext::new();
    focus_input(&mut tree, idx, &mut ctx);

    sel.set(Some((1, 4)));
    tree.dispatch_event(&WidgetEvent::FocusGained, &mut ctx);

    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(input.selected_text().as_deref(), Some("あ"));
}

// ── Pointer capture (drag-select past the widget rect) ─────────────
//
// An Input that begins a drag captures the pointer so MouseMove / MouseUp
// keep reaching it even when the cursor leaves its rect. Without capture the
// tree's hit-test would drop those events: the drag would freeze at the edge
// and a release outside would be missed, leaving the field stuck "selecting".

#[test]
fn input_mousedown_captures_pointer() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let idx = tree.add_child(root, Input::new().with_value("hello"));
    tree.compute_layout(400.0, 200.0);
    let rect = tree.layout_rect(idx);
    let mut ctx = EventContext::new();

    assert_eq!(tree.pointer_capture(), None);
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
    assert_eq!(
        tree.pointer_capture(),
        Some(idx),
        "starting a drag captures the pointer for the input"
    );
}

#[test]
fn mouseup_outside_rect_is_routed_and_releases_capture() {
    // The release lands well below the input — a normal hit-test would miss
    // it, but the capture routes it to the input so the drag ends cleanly.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(400.0));
    let idx = tree.add_child(root, Input::new().with_value("hello"));
    tree.compute_layout(400.0, 400.0);
    let rect = tree.layout_rect(idx);
    let mut ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
    assert_eq!(tree.pointer_capture(), Some(idx));

    tree.dispatch_event(
        &WidgetEvent::MouseUp {
            position: Point::new(rect.origin.x + 5.0, rect.bottom() + 80.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
    assert_eq!(
        tree.pointer_capture(),
        None,
        "a captured MouseUp outside the rect still reaches the input and releases"
    );
}

#[test]
fn drag_extends_selection_past_the_rect() {
    use shroud_text::TextEngine;
    // End-to-end: press at the start of the text, drag far to the right
    // (outside the field), and the selection must reach the end — proving the
    // out-of-rect MouseMove was delivered to the captured input rather than
    // dropped by the hit-test.
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(120.0));
    let idx = tree.add_child(root, Input::new().with_value("hello world"));
    let theme = Theme::default();
    let mut engine = TextEngine::new();
    tree.compute_layout_with_measure(200.0, 120.0, &mut engine, &theme);
    let rect = tree.layout_rect(idx);
    let mid_y = rect.origin.y + rect.size.height * 0.5;
    let mut ctx = EventContext::new();

    // Press at the left edge → caret collapses to offset 0.
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 1.0, mid_y),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
    let mut paint = PaintContext::new(theme.clone());
    tree.paint(&mut paint);

    // Drag far to the right, well past the field's right edge.
    tree.dispatch_event(
        &WidgetEvent::MouseMove {
            position: Point::new(rect.right() + 200.0, mid_y),
        },
        &mut ctx,
    );
    let mut paint2 = PaintContext::new(theme.clone());
    tree.paint(&mut paint2);

    let input = tree.widget_as::<Input>(idx).unwrap();
    assert_eq!(
        input.selected_text().as_deref(),
        Some("hello world"),
        "dragging past the right edge selects to the end of the text"
    );
}

#[test]
fn removing_captured_input_clears_capture() {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let idx = tree.add_child(root, Input::new().with_value("hello"));
    tree.compute_layout(400.0, 200.0);
    let rect = tree.layout_rect(idx);
    let mut ctx = EventContext::new();

    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
    assert_eq!(tree.pointer_capture(), Some(idx));

    tree.remove(idx);
    assert_eq!(
        tree.pointer_capture(),
        None,
        "capture must not dangle on a removed widget"
    );
}
