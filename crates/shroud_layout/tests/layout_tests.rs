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
