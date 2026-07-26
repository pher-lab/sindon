//! `TreeView` as an OS assistive technology sees it, wired up in a real tree.
//!
//! The per-node arithmetic (which state a row reports, which disclosure request
//! it honours) is unit-tested next to the widget in `tree_view.rs`. These tests
//! cover what only a live tree can show:
//!
//! - the container reports itself as a tree and its rows as items carrying the
//!   depth that the flattened row list would otherwise lose;
//! - a11y focus follows the *roving cursor* even though OS focus never leaves
//!   the host — the delegate that makes the ARIA tree pattern audible;
//! - an AT can open, close, and select a row, and the row list that a
//!   disclosure rebuilds leaves the delegate pointing at something live.

use shroud_core::{AccessAction, AccessRole};
use shroud_widgets::*;

/// A closed `src` branch with three children, then a leaf.
fn nested_tree() -> (WidgetTree, usize) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(600.0));
    let items = vec![
        TreeItem::with_children(
            1,
            "src",
            vec![
                TreeItem::new(2, "main.rs"),
                TreeItem::new(3, "lib.rs"),
                TreeItem::new(4, "app.rs"),
            ],
        ),
        TreeItem::new(5, "README.md"),
    ];
    let host = TreeView::new(items).label("Files").build(&mut tree, root);
    tree.compute_layout(800.0, 600.0);
    (tree, host)
}

/// The row node indices, in display order.
fn rows(tree: &WidgetTree, host: usize) -> Vec<usize> {
    let reactive = tree.children(host);
    assert_eq!(reactive.len(), 1, "the host owns exactly the row list");
    tree.children(reactive[0])
}

/// Every `TreeItem` entry in the snapshot, in display order.
fn items(snap: &AccessSnapshot, tree: &WidgetTree, host: usize) -> Vec<AccessEntry> {
    rows(tree, host)
        .into_iter()
        .map(|idx| {
            snap.entries
                .iter()
                .find(|e| e.id == idx as u64)
                .unwrap_or_else(|| panic!("row {idx} must appear in the snapshot"))
                .clone()
        })
        .collect()
}

fn press(tree: &mut WidgetTree, named: NamedKey) {
    let mut ctx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::KeyDown {
            key: Key::Named(named),
        },
        &mut ctx,
    );
    tree.compute_layout(800.0, 600.0);
}

// ── Perceivable ───────────────────────────────────────────────────

#[test]
fn the_container_is_a_tree_and_the_rows_are_items() {
    let (tree, host) = nested_tree();
    let snap = tree.accessibility_snapshot();

    let container = snap
        .entries
        .iter()
        .find(|e| e.id == host as u64)
        .expect("the host is in the snapshot");
    assert_eq!(container.node.role, AccessRole::Tree);
    assert_eq!(
        container.node.name.as_deref(),
        Some("Files"),
        "the tree carries the accessible name it was given"
    );
    assert!(
        container.focusable,
        "the container is the tab stop, so it advertises focus"
    );

    let items = items(&snap, &tree, host);
    assert_eq!(items.len(), 2, "starts collapsed: src, README.md");
    for item in &items {
        assert_eq!(item.node.role, AccessRole::TreeItem);
        assert!(
            !item.focusable,
            "a row is never a tab stop — the cursor roves inside the host"
        );
    }
    assert_eq!(items[0].node.name.as_deref(), Some("src"));
    assert_eq!(
        items[0].node.expanded,
        Some(false),
        "a closed branch reports that it can open"
    );
    assert_eq!(
        items[1].node.expanded, None,
        "a leaf has nothing to disclose, which is not the same as being closed"
    );
}

#[test]
fn depth_reaches_the_at_as_a_level() {
    // The rows are one flat list, so `level` is the only thing carrying the
    // hierarchy a sighted user reads off the indentation.
    let (mut tree, host) = nested_tree();
    press(&mut tree, NamedKey::Tab);
    press(&mut tree, NamedKey::ArrowRight); // open `src`

    let snap = tree.accessibility_snapshot();
    let items = items(&snap, &tree, host);
    assert_eq!(items.len(), 5, "src is open: src, 3 children, README.md");
    assert_eq!(
        items.iter().map(|e| e.node.level).collect::<Vec<_>>(),
        vec![Some(1), Some(2), Some(2), Some(2), Some(1)],
        "levels are 1-based and follow the depth-first row order"
    );
    assert_eq!(
        items[0].node.expanded,
        Some(true),
        "the branch now reports itself open"
    );
}

#[test]
fn selection_state_reaches_the_at() {
    let (mut tree, host) = nested_tree();
    press(&mut tree, NamedKey::Tab);
    press(&mut tree, NamedKey::ArrowDown); // cursor to README.md
    press(&mut tree, NamedKey::Enter); // commit it

    let snap = tree.accessibility_snapshot();
    let items = items(&snap, &tree, host);
    assert_eq!(
        items.iter().map(|e| e.node.selected).collect::<Vec<_>>(),
        vec![Some(false), Some(true)],
        "exactly the committed row reports itself selected"
    );
}

// ── Focus: the roving delegate ────────────────────────────────────

#[test]
fn a11y_focus_follows_the_cursor_while_os_focus_stays_on_the_host() {
    let (mut tree, host) = nested_tree();
    assert_eq!(
        tree.accessibility_snapshot().focus_id,
        u64::MAX,
        "nothing focused yet, so focus is the window root"
    );

    press(&mut tree, NamedKey::Tab);
    let first = rows(&tree, host)[0];
    assert_eq!(tree.focused(), Some(host), "OS focus lands on the host");
    assert_eq!(
        tree.accessibility_snapshot().focus_id,
        first as u64,
        "but the AT is pointed at the cursor row, not the container — otherwise \
         a screen reader announces the tree once and goes silent"
    );

    press(&mut tree, NamedKey::ArrowDown);
    let second = rows(&tree, host)[1];
    assert_eq!(
        tree.focused(),
        Some(host),
        "the cursor roves; keyboard focus does not move"
    );
    assert_eq!(
        tree.accessibility_snapshot().focus_id,
        second as u64,
        "the delegate tracked the arrow key"
    );
}

#[test]
fn the_delegate_survives_the_rebuild_a_disclosure_causes() {
    // Opening a branch tombstones every row widget and builds new ones. The
    // cursor rides on shared state rather than on a row, so the delegate has to
    // come back pointing at a *live* node.
    let (mut tree, host) = nested_tree();
    press(&mut tree, NamedKey::Tab);
    let before = rows(&tree, host);
    press(&mut tree, NamedKey::ArrowRight); // open `src`, rebuilding the rows
    let after = rows(&tree, host);
    assert!(
        after.iter().all(|idx| !before.contains(idx)),
        "the rebuild really did replace the row widgets"
    );

    let snap = tree.accessibility_snapshot();
    assert_eq!(
        snap.focus_id, after[0] as u64,
        "focus points at the rebuilt cursor row"
    );
    assert!(
        snap.entries.iter().any(|e| e.id == snap.focus_id),
        "and that node is present in the snapshot (accesskit rejects a dangling focus)"
    );
}

#[test]
fn losing_focus_gives_the_delegate_up() {
    let (mut tree, host) = nested_tree();
    press(&mut tree, NamedKey::Tab);
    assert_ne!(tree.accessibility_snapshot().focus_id, u64::MAX);

    let mut ctx = EventContext::new();
    tree.focus(None, &mut ctx);
    tree.compute_layout(800.0, 600.0);

    assert_eq!(
        tree.accessibility_snapshot().focus_id,
        u64::MAX,
        "a delegate is only consulted for the focused widget"
    );
    let _ = host;
}

// ── Operable ──────────────────────────────────────────────────────

#[test]
fn an_at_can_open_and_close_a_branch() {
    let (mut tree, host) = nested_tree();
    let branch = rows(&tree, host)[0];

    let mut ctx = EventContext::new();
    assert!(tree.perform_access_action(branch as u64, AccessAction::Expand, &mut ctx));
    tree.compute_layout(800.0, 600.0);
    assert_eq!(rows(&tree, host).len(), 5, "the children appeared");

    // The row list was rebuilt, so re-resolve rather than reusing a tombstone.
    let branch = rows(&tree, host)[0];
    let mut ctx = EventContext::new();
    assert!(tree.perform_access_action(branch as u64, AccessAction::Collapse, &mut ctx));
    tree.compute_layout(800.0, 600.0);
    assert_eq!(rows(&tree, host).len(), 2, "and went away again");
}

#[test]
fn a_disclosure_request_is_not_a_toggle() {
    // `Expand` on an open row must not close it, and neither request means
    // anything to a leaf — an AT is free to ask, and the answer is "no".
    let (mut tree, host) = nested_tree();
    let branch = rows(&tree, host)[0];
    let mut ctx = EventContext::new();
    tree.perform_access_action(branch as u64, AccessAction::Expand, &mut ctx);
    tree.compute_layout(800.0, 600.0);

    let branch = rows(&tree, host)[0];
    let mut ctx = EventContext::new();
    assert!(
        !tree.perform_access_action(branch as u64, AccessAction::Expand, &mut ctx),
        "expanding an open row is refused, not treated as a toggle"
    );
    tree.compute_layout(800.0, 600.0);
    assert_eq!(rows(&tree, host).len(), 5, "and changed nothing");

    let leaf = rows(&tree, host)[1]; // main.rs
    for action in [AccessAction::Expand, AccessAction::Collapse] {
        let mut ctx = EventContext::new();
        assert!(
            !tree.perform_access_action(leaf as u64, action, &mut ctx),
            "a leaf honours no disclosure request"
        );
    }
    tree.compute_layout(800.0, 600.0);
    assert_eq!(rows(&tree, host).len(), 5);
}

#[test]
fn an_at_click_selects_the_row_and_parks_the_cursor_there() {
    // The mirror of a mouse click on the label zone: select, and hand the
    // keyboard to the host so the arrows carry on from here.
    let (mut tree, host) = nested_tree();
    let readme = rows(&tree, host)[1];

    let mut ctx = EventContext::new();
    assert!(tree.perform_access_action(readme as u64, AccessAction::Click, &mut ctx));
    tree.compute_layout(800.0, 600.0);

    assert_eq!(
        tree.focused(),
        Some(host),
        "operating a row hands the keyboard to the host"
    );
    let readme = rows(&tree, host)[1];
    let snap = tree.accessibility_snapshot();
    assert_eq!(
        snap.focus_id, readme as u64,
        "and the cursor — so a11y focus — is parked on the row that was clicked"
    );
    assert_eq!(
        items(&snap, &tree, host)[1].node.selected,
        Some(true),
        "the click selected it"
    );
}
