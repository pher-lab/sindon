//! Integration tests for `Dropdown` — the popover-based single-select.
//!
//! Covers the click toggle, mouse selection writing back to the bound
//! signal, automatic dismiss paths (outside-click, Escape, option click),
//! and keyboard activation via Enter/Space on the focused trigger.

use sindon_core::{Color, Point, Theme};
use sindon_reactive::Signal;
use sindon_text::TextEngine;
use sindon_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Container, Dropdown};

fn measured_layout(tree: &mut WidgetTree, w: f32, h: f32) {
    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(w, h, &mut engine, &theme);
}

fn dispatch(tree: &mut WidgetTree, ev: WidgetEvent) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(&ev, &mut ctx);
}

/// Build a tree with a single Dropdown inside a fixed-size root.
/// Returns (tree, dropdown_idx, selected_signal).
fn dropdown_tree(options: Vec<&str>) -> (WidgetTree, usize, Signal<usize>) {
    let selected = Signal::new(0_usize);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(
        Container::column()
            .width(400.0)
            .height(300.0)
            .padding(20.0)
            .background(Color::rgb(0.05, 0.05, 0.05)),
    );
    let dd = tree.add_child(
        root,
        Dropdown::new(options.iter().map(|s| s.to_string()).collect(), selected).placeholder("--"),
    );
    (tree, dd, selected)
}

fn click(tree: &mut WidgetTree, x: f32, y: f32) {
    dispatch(
        tree,
        WidgetEvent::MouseDown {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
    );
    dispatch(
        tree,
        WidgetEvent::MouseUp {
            position: Point::new(x, y),
            button: MouseButton::Left,
        },
    );
}

#[test]
fn click_trigger_opens_popover_layer() {
    let (mut tree, dd, _sig) = dropdown_tree(vec!["One", "Two", "Three"]);
    measured_layout(&mut tree, 400.0, 300.0);
    assert_eq!(tree.layer_count(), 0);

    let r = tree.layout_rect(dd);
    click(&mut tree, r.origin.x + 10.0, r.origin.y + 10.0);

    assert_eq!(tree.layer_count(), 1, "popover should open");
}

#[test]
fn click_trigger_twice_toggles_closed() {
    let (mut tree, dd, _sig) = dropdown_tree(vec!["One", "Two"]);
    measured_layout(&mut tree, 400.0, 300.0);

    let r = tree.layout_rect(dd);
    click(&mut tree, r.origin.x + 10.0, r.origin.y + 10.0);
    assert_eq!(tree.layer_count(), 1);

    // Second click — note that while the popover is up, clicks land on the
    // layer, not the main tree. A click on the trigger's rect is outside
    // the popover's rect, so the layer's `dismiss_on_outside_click` path
    // closes it.
    measured_layout(&mut tree, 400.0, 300.0);
    click(&mut tree, r.origin.x + 10.0, r.origin.y + 10.0);
    assert_eq!(tree.layer_count(), 0, "outside-click closes popover");
}

#[test]
fn click_option_writes_signal_and_closes_popover() {
    let (mut tree, dd, sig) = dropdown_tree(vec!["A", "B", "C"]);
    measured_layout(&mut tree, 400.0, 300.0);
    assert_eq!(sig.get(), 0);

    let trigger = tree.layout_rect(dd);
    click(&mut tree, trigger.origin.x + 5.0, trigger.origin.y + 5.0);
    measured_layout(&mut tree, 400.0, 300.0);
    assert_eq!(tree.layer_count(), 1);

    // The popover root is the topmost layer's root index. Walk its
    // children: 3 OptionItems. Click the second one ("B" → index 1).
    let popover_root = tree.top_layer_root().expect("layer up");
    let options = tree.children(popover_root);
    assert_eq!(options.len(), 3);
    let opt_rect = tree.layout_rect(options[1]);
    // Click the middle of the second option's rect (translated by layer
    // offset — `layout_rect` returns layout-engine local coords, but the
    // event loop translates back). Use the option's own coords directly
    // since dispatch routes through the layer with the right offset.
    let layer_off = tree.top_layer_offset().expect("layer offset");
    click(
        &mut tree,
        layer_off.0 + opt_rect.origin.x + 10.0,
        layer_off.1 + opt_rect.origin.y + 5.0,
    );

    assert_eq!(sig.get(), 1, "signal updated to option B");
    assert_eq!(tree.layer_count(), 0, "popover dismissed after select");
}

#[test]
fn escape_closes_popover() {
    let (mut tree, dd, _sig) = dropdown_tree(vec!["A", "B"]);
    measured_layout(&mut tree, 400.0, 300.0);
    let r = tree.layout_rect(dd);
    click(&mut tree, r.origin.x + 5.0, r.origin.y + 5.0);
    assert_eq!(tree.layer_count(), 1);

    dispatch(
        &mut tree,
        WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Escape),
        },
    );
    assert_eq!(tree.layer_count(), 0, "Escape dismisses topmost layer");
}

#[test]
fn out_of_range_signal_shows_placeholder() {
    // Build a dropdown with a signal pointing past the options. The trigger
    // should still render — the placeholder kicks in via `current_label`.
    let selected = Signal::new(99_usize);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    let _dd = tree.add_child(
        root,
        Dropdown::new(vec!["A".into(), "B".into()], selected).placeholder("Choose"),
    );
    // Just verify it lays out & paints without panicking — placeholder
    // resolution is exercised by `current_label`.
    measured_layout(&mut tree, 400.0, 300.0);
    let mut ctx = sindon_widgets::paint::PaintContext::default();
    tree.paint(&mut ctx);
    assert!(!ctx.rects.is_empty());
}

#[test]
fn enter_on_focused_trigger_opens_popover() {
    let (mut tree, dd, _sig) = dropdown_tree(vec!["A", "B"]);
    measured_layout(&mut tree, 400.0, 300.0);

    // Focus the trigger programmatically.
    let mut ctx = EventContext::new();
    tree.focus(Some(dd), &mut ctx);

    dispatch(
        &mut tree,
        WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Enter),
        },
    );
    assert_eq!(tree.layer_count(), 1, "Enter opens popover");
}

#[test]
fn space_on_focused_trigger_opens_popover() {
    let (mut tree, dd, _sig) = dropdown_tree(vec!["A", "B"]);
    measured_layout(&mut tree, 400.0, 300.0);
    let mut ctx = EventContext::new();
    tree.focus(Some(dd), &mut ctx);

    dispatch(&mut tree, WidgetEvent::CharInput { ch: ' ' });
    assert_eq!(tree.layer_count(), 1, "Space opens popover");
}

#[test]
fn trigger_border_mode_recolors_border_and_omits_ring() {
    // FW-26 (G7): the trigger always draws a border, so a Border-mode theme
    // recolors it to the focus color on focus and suppresses the ring.
    use sindon_core::FocusIndicator;
    let (mut tree, dd, _sig) = dropdown_tree(vec!["A", "B"]);
    measured_layout(&mut tree, 400.0, 300.0);

    let mut ev = EventContext::new();
    tree.focus(Some(dd), &mut ev);

    let mut theme = Theme::default();
    theme.focus.indicator = FocusIndicator::Border;
    let focus_color = theme.focus.ring_color;
    let mut ctx = sindon_widgets::paint::PaintContext::new(theme);
    tree.paint(&mut ctx);

    let strokes: Vec<_> = ctx.rects.iter().filter(|r| r.border_width > 0.0).collect();
    assert_eq!(
        strokes.len(),
        1,
        "trigger border recolored — no separate ring stroke"
    );
    assert_eq!(
        strokes[0].border_width, 1.0,
        "still a 1px border, not a 2px ring"
    );
    assert_eq!(
        strokes[0].color, focus_color,
        "the trigger border is recolored to the focus color"
    );
}

// ── G16: trigger box metrics (padding_x / padding_y / min_height) ──────────

/// Lay out a single configured dropdown and return its border-box height plus
/// the default body font size.
fn trigger_height(build: impl FnOnce(Signal<usize>) -> Dropdown) -> (f32, f32) {
    let selected = Signal::new(0_usize);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).padding(20.0));
    let dd = tree.add_child(root, build(selected));
    measured_layout(&mut tree, 400.0, 300.0);
    let font = Theme::default().typography.body.font_size;
    (tree.layout_rect(dd).size.height, font)
}

#[test]
fn default_trigger_height_is_one_line_plus_padding_not_doubled() {
    // Border box = ceil(line_height) + 2 * padding_y(8). Crucially NOT the old
    // doubled `font + 32`: `measure` used to bake the padding into the content
    // height so Taffy added it a second time (the "Dropdown too big" gap).
    let (h, font) = trigger_height(|sig| Dropdown::new(vec!["A".into(), "B".into()], sig));
    let expected = (font * 1.2).ceil() + 16.0;
    assert!(
        (h - expected).abs() < 1.0,
        "default trigger height {h}, expected ~{expected}"
    );
    assert!(
        h < font + 32.0,
        "height {h} should be below the old doubled font+32 ({})",
        font + 32.0
    );
}

#[test]
fn padding_y_controls_trigger_height() {
    let (tall, _) = trigger_height(|sig| Dropdown::new(vec!["A".into()], sig).padding_y(12.0));
    let (short, font) = trigger_height(|sig| Dropdown::new(vec!["A".into()], sig).padding_y(2.0));
    assert!(
        tall > short,
        "more vertical padding → taller trigger ({tall} vs {short})"
    );
    let expected_short = (font * 1.2).ceil() + 4.0; // + 2 * padding_y(2)
    assert!(
        (short - expected_short).abs() < 1.0,
        "short trigger height {short}, expected ~{expected_short}"
    );
}

#[test]
fn min_height_floors_the_trigger_border_box() {
    // `min_height` is a border-box floor: even with tiny padding the trigger is
    // at least 60px tall.
    let (h, _) = trigger_height(|sig| {
        Dropdown::new(vec!["A".into()], sig)
            .padding_y(2.0)
            .min_height(60.0)
    });
    assert!(
        (h - 60.0).abs() < 1.0,
        "min_height(60) should floor the border box to 60, got {h}"
    );
}

#[test]
fn trigger_border_is_a_single_rounded_stroke() {
    // The trigger border is one rounded SDF stroke (border_width > 0 at the
    // trigger radius), not four sharp hairline fills that square off corners.
    let selected = Signal::new(0_usize);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0).padding(20.0));
    let _dd = tree.add_child(
        root,
        Dropdown::new(vec!["A".into(), "B".into()], selected).radius(6.0),
    );
    measured_layout(&mut tree, 400.0, 300.0);

    let mut ctx = sindon_widgets::paint::PaintContext::default();
    tree.paint(&mut ctx);
    let strokes: Vec<_> = ctx.rects.iter().filter(|r| r.border_width > 0.0).collect();
    assert_eq!(
        strokes.len(),
        1,
        "trigger should emit exactly one stroked border rect, got {}",
        strokes.len()
    );
    assert!(
        (strokes[0].radius - 6.0).abs() < 0.01,
        "border stroke should follow the trigger radius (6.0), got {}",
        strokes[0].radius
    );
}
