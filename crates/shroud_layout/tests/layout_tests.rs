use shroud_layout::{FlexStyle, LayoutEngine};
use taffy::prelude::*;

#[test]
fn basic_leaf_layout() {
    let mut engine = LayoutEngine::new();
    let root = engine.add_leaf(FlexStyle::new().width(100.0).height(50.0));
    engine.compute(root, 800.0, 600.0);

    let rect = engine.layout(root);
    assert_eq!(rect.size.width, 100.0);
    assert_eq!(rect.size.height, 50.0);
}

#[test]
fn column_layout() {
    let mut engine = LayoutEngine::new();

    let child1 = engine.add_leaf(FlexStyle::new().width(100.0).height(30.0));
    let child2 = engine.add_leaf(FlexStyle::new().width(100.0).height(40.0));
    let root = engine.add_container(
        FlexStyle::new().column().width(200.0).height(200.0),
        &[child1, child2],
    );

    engine.compute(root, 800.0, 600.0);

    let r1 = engine.layout(child1);
    let r2 = engine.layout(child2);

    assert_eq!(r1.origin.y, 0.0);
    assert_eq!(r1.size.height, 30.0);
    assert_eq!(r2.origin.y, 30.0);
    assert_eq!(r2.size.height, 40.0);
}

#[test]
fn row_layout() {
    let mut engine = LayoutEngine::new();

    let child1 = engine.add_leaf(FlexStyle::new().width(80.0).height(50.0));
    let child2 = engine.add_leaf(FlexStyle::new().width(120.0).height(50.0));
    let root = engine.add_container(
        FlexStyle::new().row().width(300.0).height(100.0),
        &[child1, child2],
    );

    engine.compute(root, 800.0, 600.0);

    let r1 = engine.layout(child1);
    let r2 = engine.layout(child2);

    assert_eq!(r1.origin.x, 0.0);
    assert_eq!(r1.size.width, 80.0);
    assert_eq!(r2.origin.x, 80.0);
    assert_eq!(r2.size.width, 120.0);
}

#[test]
fn padding_layout() {
    let mut engine = LayoutEngine::new();

    let child = engine.add_leaf(FlexStyle::new().grow(1.0));
    let root = engine.add_container(
        FlexStyle::new()
            .column()
            .width(200.0)
            .height(200.0)
            .padding(10.0),
        &[child],
    );

    engine.compute(root, 800.0, 600.0);

    let child_rect = engine.layout(child);
    // Child should be offset by padding
    assert_eq!(child_rect.origin.x, 10.0);
    assert_eq!(child_rect.origin.y, 10.0);
    // Child should fill remaining space (200 - 20 padding)
    assert_eq!(child_rect.size.width, 180.0);
}

#[test]
fn gap_layout() {
    let mut engine = LayoutEngine::new();

    let c1 = engine.add_leaf(FlexStyle::new().width(50.0).height(30.0));
    let c2 = engine.add_leaf(FlexStyle::new().width(50.0).height(30.0));
    let c3 = engine.add_leaf(FlexStyle::new().width(50.0).height(30.0));
    let root = engine.add_container(
        FlexStyle::new()
            .column()
            .width(200.0)
            .height(200.0)
            .gap(10.0),
        &[c1, c2, c3],
    );

    engine.compute(root, 800.0, 600.0);

    let r1 = engine.layout(c1);
    let r2 = engine.layout(c2);
    let r3 = engine.layout(c3);

    assert_eq!(r1.origin.y, 0.0);
    assert_eq!(r2.origin.y, 40.0); // 30 + 10 gap
    assert_eq!(r3.origin.y, 80.0); // 30 + 10 + 30 + 10
}

#[test]
fn absolute_rect_nested() {
    let mut engine = LayoutEngine::new();

    let leaf = engine.add_leaf(FlexStyle::new().width(40.0).height(20.0));
    let inner = engine.add_container(FlexStyle::new().column().padding(5.0), &[leaf]);
    let root = engine.add_container(
        FlexStyle::new()
            .column()
            .width(200.0)
            .height(200.0)
            .padding(10.0),
        &[inner],
    );

    engine.compute(root, 800.0, 600.0);

    let abs = engine.absolute_rect(leaf);
    // root padding (10) + inner padding (5) = 15
    assert_eq!(abs.origin.x, 15.0);
    assert_eq!(abs.origin.y, 15.0);
}

#[test]
fn flex_grow_distributes_space() {
    let mut engine = LayoutEngine::new();

    let c1 = engine.add_leaf(FlexStyle::new().grow(1.0).height(30.0));
    let c2 = engine.add_leaf(FlexStyle::new().grow(2.0).height(30.0));
    let root = engine.add_container(FlexStyle::new().row().width(300.0).height(100.0), &[c1, c2]);

    engine.compute(root, 800.0, 600.0);

    let r1 = engine.layout(c1);
    let r2 = engine.layout(c2);

    // grow(1) gets 1/3 = 100, grow(2) gets 2/3 = 200
    assert_eq!(r1.size.width, 100.0);
    assert_eq!(r2.size.width, 200.0);
}

#[test]
fn style_builder_chaining() {
    let style: taffy::Style = FlexStyle::new()
        .column()
        .width(100.0)
        .height(200.0)
        .padding(10.0)
        .gap(5.0)
        .center()
        .grow(1.0)
        .into();

    assert_eq!(style.flex_direction, FlexDirection::Column);
    assert_eq!(style.flex_grow, 1.0);
    assert_eq!(style.align_items, Some(AlignItems::Center));
    assert_eq!(style.justify_content, Some(JustifyContent::Center));
}

#[test]
fn justify_center_does_not_collapse_cross_axis() {
    // Column container with only main-axis centering: each child should keep
    // its natural width (Stretch default), and the group should be centered
    // vertically inside the parent. This is the failure mode that motivated
    // the new helper — `.center()` would collapse `width_full()` children to
    // min-content and break text wrapping.
    let mut engine = LayoutEngine::new();
    let c1 = engine.add_leaf(FlexStyle::new().width_full().height(40.0));
    let c2 = engine.add_leaf(FlexStyle::new().width_full().height(60.0));
    let root = engine.add_container(
        FlexStyle::new()
            .column()
            .width(400.0)
            .height(300.0)
            .justify_center(),
        &[c1, c2],
    );

    engine.compute(root, 800.0, 600.0);

    let r1 = engine.layout(c1);
    let r2 = engine.layout(c2);

    // Children retain their full parent width — not collapsed by Center.
    assert_eq!(r1.size.width, 400.0);
    assert_eq!(r2.size.width, 400.0);
    // Combined child height = 100; parent height = 300 → 100 px of free
    // space, half above (50) and half below.
    assert_eq!(r1.origin.y, 100.0);
    assert_eq!(r2.origin.y, 140.0);
}

#[test]
fn max_width_clamps_growth() {
    // Inner column declares `width_full().max_width(200)`. Even though the
    // parent offers 800 px of horizontal space, the inner node should stop
    // at 200 px.
    let mut engine = LayoutEngine::new();
    let inner = engine.add_container(FlexStyle::new().column().width_full().max_width(200.0), &[]);
    let root = engine.add_container(FlexStyle::new().row().width(800.0).height(100.0), &[inner]);

    engine.compute(root, 1000.0, 600.0);

    assert_eq!(engine.layout(inner).size.width, 200.0);
}

#[test]
fn max_width_does_not_force_growth() {
    // max_width is a clamp, not a target — a fixed-width child stays at its
    // declared width even when the clamp is larger.
    let mut engine = LayoutEngine::new();
    let inner = engine.add_leaf(FlexStyle::new().width(120.0).max_width(400.0).height(20.0));
    let root = engine.add_container(FlexStyle::new().row().width(800.0).height(100.0), &[inner]);

    engine.compute(root, 1000.0, 600.0);

    assert_eq!(engine.layout(inner).size.width, 120.0);
}

#[test]
fn flex_basis_zero_with_grow_takes_row_leftover_space() {
    // CSS `flex: 1 1 0` shape: the body item starts at zero main size and
    // grows to fill whatever the fixed-width sibling didn't claim. Without
    // `flex_basis(0)`, Taffy resolves the body's basis to its content's
    // natural size — which for wrappable text overflows the row entirely.
    let mut engine = LayoutEngine::new();
    let bar = engine.add_leaf(FlexStyle::new().width(4.0));
    let body = engine.add_leaf(FlexStyle::new().flex_basis(0.0).grow(1.0));
    let row = engine.add_container(
        FlexStyle::new().row().width(300.0).height(100.0).gap(12.0),
        &[bar, body],
    );

    engine.compute(row, 800.0, 600.0);

    // 300 (row) - 4 (bar) - 12 (gap) = 284 leftover for body.
    assert_eq!(engine.layout(body).size.width, 284.0);
    assert_eq!(engine.layout(bar).size.width, 4.0);
}

#[test]
fn flex_wrap_wrap_breaks_row_when_children_overflow() {
    // With wrap enabled, children that don't fit on one line should flow to
    // the next. With wrap disabled (default) they would all stay on one row
    // and overflow.
    let mut engine = LayoutEngine::new();
    let c1 = engine.add_leaf(FlexStyle::new().width(120.0).height(30.0));
    let c2 = engine.add_leaf(FlexStyle::new().width(120.0).height(30.0));
    let c3 = engine.add_leaf(FlexStyle::new().width(120.0).height(30.0));
    let row = engine.add_container(
        FlexStyle::new().row().width(250.0).flex_wrap(true),
        &[c1, c2, c3],
    );

    engine.compute(row, 800.0, 600.0);

    // First two children fit; third must wrap.
    assert_eq!(engine.layout(c1).origin.y, 0.0);
    assert_eq!(engine.layout(c2).origin.y, 0.0);
    assert!(
        engine.layout(c3).origin.y >= 30.0,
        "third child should be on a new line, got y={}",
        engine.layout(c3).origin.y,
    );
}

#[test]
fn row_align_center_keeps_children_at_natural_height() {
    // Row-direction container with two children of different heights. Default
    // cross-axis alignment (Stretch) would make the shorter child grow to the
    // taller one's height — visually awkward for a header (button stretching
    // to title height). `align_center` should:
    //   1. size each child to its own declared height
    //   2. center them vertically on the row
    let mut engine = LayoutEngine::new();
    let title = engine.add_leaf(FlexStyle::new().width(120.0).height(40.0));
    let button = engine.add_leaf(FlexStyle::new().width(80.0).height(20.0));
    let header = engine.add_container(
        FlexStyle::new()
            .row()
            .width(400.0)
            .height(60.0)
            .align_center(),
        &[title, button],
    );

    engine.compute(header, 800.0, 200.0);

    let title_rect = engine.layout(title);
    let button_rect = engine.layout(button);

    // Heights stay at the declared values — Stretch did not kick in.
    assert_eq!(title_rect.size.height, 40.0);
    assert_eq!(button_rect.size.height, 20.0);

    // Both children sit centered on the 60 px row: title (40 h) at y=10,
    // button (20 h) at y=20.
    assert_eq!(title_rect.origin.y, 10.0);
    assert_eq!(button_rect.origin.y, 20.0);
}
