//! Tests for the `ReactiveChildren` primitive: a layout container whose
//! children are rebuilt by an app-supplied builder when a version token
//! changes. The rebuild runs from the tree's layout pass
//! (`sync_reactive_children`), so these drive it via `compute_layout`.

use std::cell::Cell;
use std::rc::Rc;

use sindon_reactive::Signal;
use sindon_widgets::tree::WidgetTree;
use sindon_widgets::{Container, ReactiveChildren};

/// When the version token changes, the next layout pass tombstones the old
/// children and repopulates from the builder.
#[test]
fn version_change_rebuilds_children() {
    // The signal doubles as the version token *and* the child count the
    // builder emits, so a bump is observable in `children(...).len()`.
    let count = Signal::new(2u64);

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(200.0));

    let version_src = count;
    let build_src = count;
    let rc = tree.add_child(
        root,
        ReactiveChildren::column().source(
            move || version_src.get(),
            move |tree, parent| {
                for _ in 0..build_src.get() {
                    tree.add_child(parent, Container::row());
                }
            },
        ),
    );

    tree.compute_layout(200.0, 200.0);
    assert_eq!(tree.children(rc).len(), 2, "first build emits two children");

    // Bump the token (and the child count): the next pass rebuilds.
    count.set(5);
    tree.compute_layout(200.0, 200.0);
    assert_eq!(
        tree.children(rc).len(),
        5,
        "a version change repopulates the subtree"
    );
}

/// A steady version token must not rebuild — the builder runs once and stays
/// put across repeated layout passes, and fires again only on a real change.
#[test]
fn stable_version_does_not_rebuild() {
    let version = Signal::new(7u64);
    let build_calls = Rc::new(Cell::new(0usize));

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(200.0).height(200.0));

    let version_src = version;
    let calls = Rc::clone(&build_calls);
    let rc = tree.add_child(
        root,
        ReactiveChildren::column().source(
            move || version_src.get(),
            move |tree, parent| {
                calls.set(calls.get() + 1);
                tree.add_child(parent, Container::row());
            },
        ),
    );

    // Three passes with an unchanged token → exactly one build.
    tree.compute_layout(200.0, 200.0);
    tree.compute_layout(200.0, 200.0);
    tree.compute_layout(200.0, 200.0);
    assert_eq!(
        build_calls.get(),
        1,
        "an unchanged version must build the children only once"
    );
    assert_eq!(tree.children(rc).len(), 1);

    // A token change triggers exactly one more build.
    version.set(8);
    tree.compute_layout(200.0, 200.0);
    assert_eq!(
        build_calls.get(),
        2,
        "a version change triggers exactly one rebuild"
    );
    assert_eq!(tree.children(rc).len(), 1);
}
