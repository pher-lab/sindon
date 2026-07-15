//! Accessibility snapshot — a per-frame, `accesskit`-free description of the
//! whole widget tree for OS assistive technology.
//!
//! [`WidgetTree::accessibility_snapshot`](crate::tree::WidgetTree::accessibility_snapshot)
//! walks the live tree and produces an [`AccessSnapshot`]: a flat list of
//! [`AccessEntry`] (one per visible widget, plus a synthetic window root) with
//! each node's semantic [`AccessNode`], absolute viewport bounds, and child
//! ids, plus which node holds focus. `shroud_app` translates this into an
//! `accesskit::TreeUpdate`. Nothing here links `accesskit` — the same
//! discipline that keeps `winit` at the platform edge.
//!
//! Node ids are widget indices (`usize as u64`). Those indices are stable and
//! never reused (the tree tombstones removed slots and only ever pushes new
//! ones), so an index is a durable, collision-free id. The synthetic window
//! root uses [`A11Y_WINDOW_ROOT`], a reserved id no widget index can reach.

use shroud_core::{AccessNode, Rect};

/// Reserved node id for the synthetic window root that bundles the main tree
/// and every overlay layer under a single a11y root. `u64::MAX` can never
/// collide with a real widget index (which is `nodes.len()` at insert time).
pub const A11Y_WINDOW_ROOT: u64 = u64::MAX;

/// One node in an [`AccessSnapshot`]: a widget's semantics plus the tree-shape
/// facts (bounds, children) the semantic [`AccessNode`] deliberately omits.
#[derive(Debug, Clone)]
pub struct AccessEntry {
    /// Stable node id — the widget index, or [`A11Y_WINDOW_ROOT`] for the root.
    pub id: u64,
    /// Role / name / state. Secret-safe: a protected node never carries a value.
    pub node: AccessNode,
    /// Absolute bounds in viewport coordinates (layer offset already folded in).
    pub bounds: Rect,
    /// Child node ids, in tree order. Only visible children appear, so every
    /// referenced id has a matching entry in the snapshot.
    pub children: Vec<u64>,
    /// Whether this node is a modal surface (the topmost interactive layer's
    /// root). Drives `accesskit`'s modal flag so ATs treat the background as
    /// inert while a dialog is up.
    pub modal: bool,
}

/// A complete a11y description of the tree for one frame.
///
/// `entries` includes exactly one node per visible widget plus the synthetic
/// window root. `focus_id` always names a node present in `entries` (it falls
/// back to the window root when nothing focusable is focused or the focused
/// widget is not currently visible), so a translation can set focus without a
/// dangling reference.
#[derive(Debug, Clone)]
pub struct AccessSnapshot {
    /// Id of the tree root — always [`A11Y_WINDOW_ROOT`].
    pub root_id: u64,
    /// Id of the focused node; [`A11Y_WINDOW_ROOT`] when nothing is focused.
    pub focus_id: u64,
    /// Every node in the tree, root included. Order is unspecified.
    pub entries: Vec<AccessEntry>,
}
