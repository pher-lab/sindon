//! Layout engine wrapping Taffy.

use shroud_core::{Rect, Size as CoreSize};
use taffy::prelude::*;

/// Query passed to a measure closure during `compute_with_measure`.
///
/// Each field is `Some` when Taffy has a concrete value from the parent
/// constraint resolution, or `None` when it is unconstrained (in which case
/// the measure function should report its natural size).
#[derive(Debug, Clone, Copy)]
pub struct MeasureQuery {
    /// Width already decided by the parent (takes priority over available).
    pub known_width: Option<f32>,
    /// Height already decided by the parent.
    pub known_height: Option<f32>,
    /// Definite available width — use as a shaping `max_width` for text.
    pub available_width: Option<f32>,
    /// Definite available height.
    pub available_height: Option<f32>,
}

/// Wraps a `TaffyTree` and provides layout computation.
///
/// Each widget registers a Taffy node via `add_leaf` or `add_container`.
/// After `compute()`, use `layout_rect()` to get absolute positions.
pub struct LayoutEngine {
    tree: TaffyTree,
}

impl LayoutEngine {
    pub fn new() -> Self {
        Self {
            tree: TaffyTree::new(),
        }
    }

    /// Add a leaf node (no children) with the given style.
    pub fn add_leaf(&mut self, style: impl Into<Style>) -> NodeId {
        self.tree
            .new_leaf(style.into())
            .expect("failed to add leaf node")
    }

    /// Add a container node with the given style and children.
    pub fn add_container(&mut self, style: impl Into<Style>, children: &[NodeId]) -> NodeId {
        self.tree
            .new_with_children(style.into(), children)
            .expect("failed to add container node")
    }

    /// Update the style of an existing node.
    pub fn set_style(&mut self, node: NodeId, style: impl Into<Style>) {
        self.tree
            .set_style(node, style.into())
            .expect("failed to set style");
    }

    /// Set the children of an existing node.
    pub fn set_children(&mut self, node: NodeId, children: &[NodeId]) {
        self.tree
            .set_children(node, children)
            .expect("failed to set children");
    }

    /// Remove a node from the tree.
    pub fn remove(&mut self, node: NodeId) {
        let _ = self.tree.remove(node);
    }

    /// Mark a node as dirty so its cached layout/measure is recomputed on
    /// the next `compute_with_measure` call.
    ///
    /// Taffy memoizes leaf measure results by `(node, available_width,
    /// available_height)`. When a widget's *content* changes (e.g., reactive
    /// text whose closure now returns a longer string) but its style and the
    /// available space don't, the cached measure is reused and the widget
    /// stays at its old width. Mark dirty before recomputing to force Taffy
    /// to re-invoke the measure closure.
    pub fn mark_dirty(&mut self, node: NodeId) {
        let _ = self.tree.mark_dirty(node);
    }

    /// Compute layout for the tree rooted at `root`.
    ///
    /// `available_width` and `available_height` are the viewport dimensions.
    pub fn compute(&mut self, root: NodeId, available_width: f32, available_height: f32) {
        self.tree
            .compute_layout(
                root,
                Size {
                    width: AvailableSpace::Definite(available_width),
                    height: AvailableSpace::Definite(available_height),
                },
            )
            .expect("layout computation failed");
    }

    /// Compute layout, calling `measure` for leaf nodes to resolve intrinsic size.
    ///
    /// The closure is invoked for each leaf node (typically a widget with no
    /// fixed size via flex style) and receives a `MeasureQuery` describing
    /// what Taffy has already decided and what's available. It returns the
    /// content size the widget wants. Taffy adds padding/border on top.
    pub fn compute_with_measure<F>(
        &mut self,
        root: NodeId,
        available_width: f32,
        available_height: f32,
        mut measure: F,
    ) where
        F: FnMut(NodeId, MeasureQuery) -> CoreSize,
    {
        self.tree
            .compute_layout_with_measure(
                root,
                Size {
                    width: AvailableSpace::Definite(available_width),
                    height: AvailableSpace::Definite(available_height),
                },
                |known, available, node_id, _node_ctx, _style| {
                    // MinContent → `Some(0.0)` so wrappable widgets (text)
                    // report their narrowest possible layout (= longest
                    // unbreakable word width) when Taffy probes the lower
                    // bound of their main-axis size. Without this, flexbox
                    // sees min-content = natural-content for text and a
                    // body column inside a row balloons to natural width,
                    // squeezing fixed-width siblings to zero (the markdown_demo
                    // blockquote bar bug). MaxContent stays `None` (= no
                    // constraint, returns natural unwrapped size).
                    let convert = |av: AvailableSpace| match av {
                        AvailableSpace::Definite(v) => Some(v),
                        AvailableSpace::MinContent => Some(0.0),
                        AvailableSpace::MaxContent => None,
                    };
                    let query = MeasureQuery {
                        known_width: known.width,
                        known_height: known.height,
                        available_width: convert(available.width),
                        available_height: convert(available.height),
                    };
                    let size = measure(node_id, query);
                    Size {
                        width: size.width,
                        height: size.height,
                    }
                },
            )
            .expect("layout computation failed");
    }

    /// Get the layout rectangle for a node (relative to its parent).
    pub fn layout(&self, node: NodeId) -> Rect {
        let layout = self.tree.layout(node).expect("node not found");
        Rect::new(
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }

    /// Get the absolute position of a node by walking up the tree.
    ///
    /// Accumulates parent offsets to produce a screen-space rectangle.
    pub fn absolute_rect(&self, node: NodeId) -> Rect {
        let layout = self.tree.layout(node).expect("node not found");
        let mut x = layout.location.x;
        let mut y = layout.location.y;

        let mut current = node;
        while let Some(parent) = self.tree.parent(current) {
            let parent_layout = self.tree.layout(parent).expect("parent not found");
            x += parent_layout.location.x;
            y += parent_layout.location.y;
            current = parent;
        }

        Rect::new(x, y, layout.size.width, layout.size.height)
    }

    /// Access the underlying TaffyTree (for advanced usage).
    pub fn tree(&self) -> &TaffyTree {
        &self.tree
    }

    /// Mutable access to the underlying TaffyTree.
    pub fn tree_mut(&mut self) -> &mut TaffyTree {
        &mut self.tree
    }
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for LayoutEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutEngine").finish_non_exhaustive()
    }
}
