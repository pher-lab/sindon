//! Accessibility snapshot — a per-frame, `accesskit`-free description of the
//! whole widget tree for OS assistive technology.
//!
//! [`WidgetTree::accessibility_snapshot`](crate::tree::WidgetTree::accessibility_snapshot)
//! walks the live tree and produces an [`AccessSnapshot`]: a flat list of
//! [`AccessEntry`] (one per visible widget, plus a synthetic window root) with
//! each node's semantic [`AccessNode`], absolute viewport bounds, and child
//! ids, plus which node holds focus. `sindon_app` translates this into an
//! `accesskit::TreeUpdate`. Nothing here links `accesskit` — the same
//! discipline that keeps `winit` at the platform edge.
//!
//! Node ids are widget indices (`usize as u64`). Those indices are stable and
//! never reused (the tree tombstones removed slots and only ever pushes new
//! ones), so an index is a durable, collision-free id. The synthetic window
//! root uses [`A11Y_WINDOW_ROOT`], a reserved id no widget index can reach.
//! Composite controls that paint their own options (`Segmented`, `RadioGroup`)
//! contribute extra nodes that are not widgets; those get derived ids in a
//! reserved high-bit space — see [`access_child_id`] / [`access_target`].

use sindon_core::{AccessChild, AccessNode, Rect};

/// Reserved node id for the synthetic window root that bundles the main tree
/// and every overlay layer under a single a11y root. `u64::MAX` can never
/// collide with a real widget index (which is `nodes.len()` at insert time).
pub const A11Y_WINDOW_ROOT: u64 = u64::MAX;

/// High bit marking a node id as a synthetic per-option child rather than a
/// widget index. Widget indices are `nodes.len()` at insert time, so they can
/// never reach into this space.
const ACCESS_CHILD_FLAG: u64 = 1 << 63;
/// Low bits of a child id reserved for the option index.
const ACCESS_OPTION_BITS: u32 = 16;
/// Options past this many in one composite control cannot get a distinct id.
/// A radio group or segmented bar that long is not a real UI, and the walk
/// simply stops emitting children beyond it rather than colliding.
pub const ACCESS_MAX_OPTIONS: usize = 1 << ACCESS_OPTION_BITS;

/// The id of one synthetic option node, derived from its owner widget's index.
///
/// Packing (rather than a side table) keeps ids stateless: the tree can decode
/// an incoming action target back to `(owner, option)` with no bookkeeping that
/// could go stale across a rebuild. `option` must be under
/// [`ACCESS_MAX_OPTIONS`]; callers clamp.
pub fn access_child_id(owner: usize, option: usize) -> u64 {
    debug_assert!(option < ACCESS_MAX_OPTIONS, "option index out of id space");
    ACCESS_CHILD_FLAG | ((owner as u64) << ACCESS_OPTION_BITS) | (option as u64)
}

/// What an a11y node id refers to. The inverse of [`access_child_id`] plus the
/// two non-composite cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessTarget {
    /// The synthetic window root ([`A11Y_WINDOW_ROOT`]).
    Window,
    /// A widget, by tree index.
    Widget(usize),
    /// One option painted by a composite widget.
    Option {
        /// Tree index of the widget that owns (and paints) the option.
        owner: usize,
        /// Which option, in the order the widget reports its children.
        index: usize,
    },
}

/// Resolve a node id coming back from an assistive technology.
///
/// The window root is checked first: its all-ones id also has the child flag
/// set, and the root is not an option target (reaching it as one would take
/// ~2^47 live widgets).
pub fn access_target(id: u64) -> AccessTarget {
    if id == A11Y_WINDOW_ROOT {
        return AccessTarget::Window;
    }
    if id & ACCESS_CHILD_FLAG != 0 {
        let bare = id & !ACCESS_CHILD_FLAG;
        return AccessTarget::Option {
            owner: (bare >> ACCESS_OPTION_BITS) as usize,
            index: (bare & ((1 << ACCESS_OPTION_BITS) - 1)) as usize,
        };
    }
    AccessTarget::Widget(id as usize)
}

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
    /// referenced id has a matching entry in the snapshot. A composite control
    /// also lists its synthetic option ids here (see [`access_child_id`]).
    pub children: Vec<u64>,
    /// Whether this node is a modal surface (the topmost interactive layer's
    /// root). Drives `accesskit`'s modal flag so ATs treat the background as
    /// inert while a dialog is up.
    pub modal: bool,
    /// Whether the node can take keyboard focus
    /// ([`Widget::focusable`](crate::Widget::focusable)) — a tree-shape fact
    /// the semantic [`AccessNode`] deliberately omits. Decides whether the
    /// translation advertises a focus action to the OS.
    pub focusable: bool,
}

/// Build the entry for one synthetic option, given its owner's index and the
/// [`AccessChild`] the owner reported. `offset` is the layer offset already
/// being folded into the owner's own bounds.
fn child_entry(owner: usize, index: usize, child: AccessChild, offset: (f32, f32)) -> AccessEntry {
    AccessEntry {
        id: access_child_id(owner, index),
        node: child.node,
        bounds: Rect::new(
            child.bounds.origin.x + offset.0,
            child.bounds.origin.y + offset.1,
            child.bounds.size.width,
            child.bounds.size.height,
        ),
        children: Vec::new(),
        modal: false,
        // An option is operated through its owner, which is the focusable
        // thing: focus lands on the group, arrows move within it (the ARIA
        // radiogroup pattern the widgets already implement for the keyboard).
        focusable: false,
    }
}

/// Emit entries for every option a composite widget paints, appending their ids
/// to `into_children`. Options past [`ACCESS_MAX_OPTIONS`] are dropped rather
/// than given a colliding id.
pub(crate) fn push_child_entries(
    owner: usize,
    children: Vec<AccessChild>,
    offset: (f32, f32),
    into_children: &mut Vec<u64>,
    entries: &mut Vec<AccessEntry>,
) {
    for (index, child) in children.into_iter().enumerate().take(ACCESS_MAX_OPTIONS) {
        into_children.push(access_child_id(owner, index));
        entries.push(child_entry(owner, index, child, offset));
    }
}

#[cfg(test)]
mod id_tests {
    use super::*;

    #[test]
    fn child_ids_round_trip() {
        for (owner, option) in [(0, 0), (1, 2), (7, 0), (4096, 65535)] {
            let id = access_child_id(owner, option);
            assert_eq!(
                access_target(id),
                AccessTarget::Option {
                    owner,
                    index: option
                },
                "child id must decode back to its owner and option"
            );
        }
    }

    #[test]
    fn widget_and_window_ids_are_not_option_targets() {
        // A plain widget index has the child flag clear, so it stays a widget
        // no matter how large — these are the ids the snapshot walk emits.
        for idx in [0usize, 1, 65_537, 1 << 40] {
            assert_eq!(access_target(idx as u64), AccessTarget::Widget(idx));
        }
        // The window root is all-ones, so the child flag is set too — it must
        // still resolve as the root rather than as some absurd option.
        assert_eq!(access_target(A11Y_WINDOW_ROOT), AccessTarget::Window);
    }

    #[test]
    fn child_ids_never_collide_with_widget_ids() {
        // The two id spaces are disjoint: no owner/option pair can produce an
        // id a widget index could also produce.
        for (owner, option) in [(0, 0), (3, 5), (999, 1)] {
            let id = access_child_id(owner, option);
            assert!(
                matches!(access_target(id), AccessTarget::Option { .. }),
                "child id {id} decoded as a widget"
            );
        }
    }
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
