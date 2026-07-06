//! G18 (second half): viewport-relative sizing (`vh`/`vw`).
//!
//! The widget tree bakes `Container::max_height_vh` & friends into concrete
//! pixels against the current viewport, resolving on install and re-resolving
//! on resize. This is the `max-h-[80vh]` a modal card relies on to cap its
//! height at a fraction of the window (and scroll its body) regardless of the
//! parent's own size. The pure resolution math is covered in
//! `shroud_layout/tests/layout_tests.rs`; here we exercise the tree plumbing.

use shroud_widgets::tree::WidgetTree;
use shroud_widgets::*;

#[test]
fn max_height_vh_caps_card_at_viewport_fraction() {
    // A card whose content wants 2000 px, capped at 80vh. Against a 1000 px
    // viewport the cap is 800 — the card must clamp there, not balloon to its
    // content height.
    let mut tree = WidgetTree::new();
    let card = tree.set_root(Container::column().max_height_vh(80.0));
    let _tall = tree.add_child(card, Container::column().width(200.0).height(2000.0));
    tree.compute_layout(1200.0, 1000.0);

    assert!(
        (tree.layout_rect(card).size.height - 800.0).abs() < 0.5,
        "80vh of a 1000px viewport should cap the card at 800, got {}",
        tree.layout_rect(card).size.height
    );
}

#[test]
fn max_height_vh_reresolves_on_resize() {
    // Same card, laid out once at 1000 px tall then again at 500 px tall. The
    // vh cap must track the new viewport (400), proving the resize re-resolve
    // path fires rather than reusing the install-time pixels.
    let mut tree = WidgetTree::new();
    let card = tree.set_root(Container::column().max_height_vh(80.0));
    let _tall = tree.add_child(card, Container::column().width(200.0).height(2000.0));

    tree.compute_layout(1200.0, 1000.0);
    assert!((tree.layout_rect(card).size.height - 800.0).abs() < 0.5);

    tree.compute_layout(1200.0, 500.0);
    assert!(
        (tree.layout_rect(card).size.height - 400.0).abs() < 0.5,
        "after resize to 500px viewport, 80vh should re-resolve to 400, got {}",
        tree.layout_rect(card).size.height
    );
}

#[test]
fn width_vw_resolves_against_viewport_width() {
    let mut tree = WidgetTree::new();
    // A fixed-height box whose width is 50vw.
    let box_idx = tree.set_root(Container::column().height(40.0).width_vw(50.0));
    tree.compute_layout(800.0, 600.0);

    assert!(
        (tree.layout_rect(box_idx).size.width - 400.0).abs() < 0.5,
        "50vw of an 800px viewport should be 400, got {}",
        tree.layout_rect(box_idx).size.width
    );
}

#[test]
fn plain_pixel_container_is_untouched_by_viewport_path() {
    // Regression guard: a container with no vh/vw keeps its exact pixel size
    // across a resize (it must not be dragged through the re-resolve path).
    let mut tree = WidgetTree::new();
    let fixed = tree.set_root(Container::column().width(300.0).height(150.0));
    tree.compute_layout(800.0, 600.0);
    assert_eq!(tree.layout_rect(fixed).size.width, 300.0);
    tree.compute_layout(1600.0, 1200.0);
    assert_eq!(tree.layout_rect(fixed).size.width, 300.0);
    assert_eq!(tree.layout_rect(fixed).size.height, 150.0);
}
