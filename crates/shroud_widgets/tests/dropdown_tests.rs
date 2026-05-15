//! Integration tests for `Dropdown` — the popover-based single-select.
//!
//! Covers the click toggle, mouse selection writing back to the bound
//! signal, automatic dismiss paths (outside-click, Escape, option click),
//! and keyboard activation via Enter/Space on the focused trigger.

use shroud_core::{Color, Point, Theme};
use shroud_reactive::Signal;
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Dropdown};

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
    let mut ctx = shroud_widgets::paint::PaintContext::default();
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
