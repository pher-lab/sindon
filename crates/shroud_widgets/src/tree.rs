//! Widget tree — manages the widget hierarchy and coordinates layout/paint/events.

use std::collections::HashMap;

use crate::event::{EventContext, EventResult, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Point, Rect, SecurityLevel, Size, Theme};
use shroud_layout::{LayoutEngine, LayoutNodeId};
use shroud_text::TextEngine;

/// An entry in the widget tree.
struct WidgetNode {
    widget: Box<dyn Widget>,
    layout_node: LayoutNodeId,
    children: Vec<usize>,
    /// Upward index for future tree-walk APIs (focus traversal, ancestor
    /// queries). Kept as it's cheap to maintain and removing would require
    /// re-threading every `add_child` call-site later.
    #[allow(dead_code)]
    parent: Option<usize>,
    /// Effective security level: max(parent_effective, self.declared).
    effective_security: SecurityLevel,
}

/// Manages a tree of widgets with layout integration.
///
/// Widgets are stored in a flat Vec. Parent-child relationships are tracked
/// via indices. Each widget has a corresponding Taffy layout node.
pub struct WidgetTree {
    nodes: Vec<WidgetNode>,
    root: Option<usize>,
    layout: LayoutEngine,
    /// Index of the widget currently under the cursor (for MouseEnter/Leave).
    hovered: Option<usize>,
}

impl WidgetTree {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            root: None,
            layout: LayoutEngine::new(),
            hovered: None,
        }
    }

    /// Add a widget as the root of the tree.
    pub fn set_root(&mut self, widget: impl Widget + 'static) -> usize {
        let effective_security = widget.security_level();
        let layout_node = self.layout.add_leaf(widget.style());
        let idx = self.nodes.len();
        self.nodes.push(WidgetNode {
            widget: Box::new(widget),
            layout_node,
            children: Vec::new(),
            parent: None,
            effective_security,
        });
        self.root = Some(idx);
        idx
    }

    /// Add a child widget to the given parent.
    pub fn add_child(&mut self, parent: usize, widget: impl Widget + 'static) -> usize {
        let parent_security = self.nodes[parent].effective_security;
        let effective_security = parent_security.merge(widget.security_level());
        let layout_node = self.layout.add_leaf(widget.style());
        let idx = self.nodes.len();
        self.nodes.push(WidgetNode {
            widget: Box::new(widget),
            layout_node,
            children: Vec::new(),
            parent: Some(parent),
            effective_security,
        });
        self.nodes[parent].children.push(idx);

        // Update Taffy parent-child relationship
        let parent_layout = self.nodes[parent].layout_node;
        let child_layout_nodes: Vec<LayoutNodeId> = self.nodes[parent]
            .children
            .iter()
            .map(|&i| self.nodes[i].layout_node)
            .collect();
        self.layout.set_children(parent_layout, &child_layout_nodes);

        idx
    }

    /// Compute layout for the entire tree (no intrinsic-size measurement).
    ///
    /// Use this for tests or for trees where no widget needs to report its
    /// intrinsic size. Leaf widgets like `TextWidget` or `Button` will be
    /// sized purely by their flex style — so centering them via
    /// `Container::column().center()` will collapse them to width 0 unless
    /// a fixed width is set. For the general case, prefer
    /// [`Self::compute_layout_with_measure`].
    pub fn compute_layout(&mut self, width: f32, height: f32) {
        if let Some(root) = self.root {
            let root_node = self.nodes[root].layout_node;
            self.layout.compute(root_node, width, height);
        }
    }

    /// Compute layout, consulting `Widget::measure` for every leaf.
    ///
    /// This is the path the real event loop uses. It lets `TextWidget` and
    /// `Button` report their natural size based on their shaped content, so
    /// flex centering / gap / grow work without wrapper containers.
    pub fn compute_layout_with_measure(
        &mut self,
        width: f32,
        height: f32,
        text_engine: &mut TextEngine,
        theme: &Theme,
    ) {
        let Some(root) = self.root else { return };
        let root_node = self.nodes[root].layout_node;

        // Invalidate Taffy's measure cache. Taffy memoizes leaf measure
        // results by (node, available_width, available_height); when a
        // reactive widget's *content* changes but its style and the viewport
        // don't, Taffy would otherwise reuse the stale size. Reproducer: the
        // counter example — clicking past 9 leaves "Count: 10" laid out at
        // the cached "Count: 0" width, so paint re-shapes with a too-narrow
        // max_width and wraps. Marking each node dirty per pass forces re-
        // measure. Cost is negligible at our tree sizes.
        for node in &self.nodes {
            self.layout.mark_dirty(node.layout_node);
        }

        // Reverse-lookup Taffy node → widget index. Built fresh every layout
        // pass; cheap for the tree sizes we target, and keeps us from having
        // to stash indices as Taffy node-context.
        let node_map: HashMap<LayoutNodeId, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.layout_node, i))
            .collect();

        let nodes = &self.nodes;
        self.layout
            .compute_with_measure(root_node, width, height, |node_id, query| {
                let Some(&widget_idx) = node_map.get(&node_id) else {
                    // Node unknown to us (shouldn't happen in practice) —
                    // return zero so Taffy falls back to style-based sizing.
                    return Size::ZERO;
                };

                // Width constraint: parent-decided width wins over available.
                let constraint = query.known_width.or(query.available_width);

                let widget = nodes[widget_idx].widget.as_ref();
                let mut ctx = MeasureContext::new(text_engine, theme);
                widget.measure(constraint, &mut ctx).unwrap_or(Size::ZERO)
            });
    }

    /// Paint the entire widget tree, returning draw commands.
    pub fn paint(&self, ctx: &mut PaintContext) {
        if let Some(root) = self.root {
            self.paint_node(root, ctx);
        }
    }

    fn paint_node(&self, idx: usize, ctx: &mut PaintContext) {
        let node = &self.nodes[idx];
        let layout_rect = self.layout.absolute_rect(node.layout_node);
        node.widget.paint(layout_rect, ctx);

        node.widget.paint_pre_children(layout_rect, ctx);
        for &child in &node.children {
            self.paint_node(child, ctx);
        }
        node.widget.paint_post_children(layout_rect, ctx);
    }

    /// Dispatch an event through the widget tree (hit-testing).
    ///
    /// For mouse events, tests against layout rects in reverse paint order
    /// (front-to-back) so the topmost widget gets the event first.
    ///
    /// For `MouseMove`, automatically generates `MouseEnter`/`MouseLeave`
    /// events when the cursor moves between widgets.
    pub fn dispatch_event(
        &mut self,
        event: &WidgetEvent,
        event_ctx: &mut EventContext,
    ) -> EventResult {
        // Generate MouseEnter/MouseLeave on cursor movement
        if let WidgetEvent::MouseMove { position } = event {
            self.update_hover(*position, event_ctx);
        }

        if let Some(root) = self.root {
            return self.dispatch_to_node(root, event, event_ctx);
        }
        EventResult::Ignored
    }

    /// Track which widget the cursor is over and generate Enter/Leave events.
    fn update_hover(&mut self, pos: Point, event_ctx: &mut EventContext) {
        let new_hover = self.hit_test(pos);

        if new_hover == self.hovered {
            return;
        }

        // Send MouseLeave to the widget we're leaving
        if let Some(old_idx) = self.hovered {
            let old_rect = self.layout.absolute_rect(self.nodes[old_idx].layout_node);
            self.nodes[old_idx]
                .widget
                .event(&WidgetEvent::MouseLeave, old_rect, event_ctx);
        }

        // Send MouseEnter to the widget we're entering
        if let Some(new_idx) = new_hover {
            let new_rect = self.layout.absolute_rect(self.nodes[new_idx].layout_node);
            self.nodes[new_idx]
                .widget
                .event(&WidgetEvent::MouseEnter, new_rect, event_ctx);
        }

        self.hovered = new_hover;
    }

    /// Find the deepest (frontmost) widget at the given position.
    fn hit_test(&self, pos: Point) -> Option<usize> {
        self.root.and_then(|root| self.hit_test_node(root, pos))
    }

    fn hit_test_node(&self, idx: usize, pos: Point) -> Option<usize> {
        let node = &self.nodes[idx];
        let rect = self.layout.absolute_rect(node.layout_node);

        if !rect.contains(pos) {
            return None;
        }

        // Transform cursor position for children that live in a scrolled
        // coordinate space.
        let (ox, oy) = node.widget.scroll_offset();
        let child_pos = if ox == 0.0 && oy == 0.0 {
            pos
        } else {
            Point::new(pos.x + ox, pos.y + oy)
        };

        // Check children in reverse paint order (last child = frontmost)
        for &child in node.children.iter().rev() {
            if let Some(hit) = self.hit_test_node(child, child_pos) {
                return Some(hit);
            }
        }

        // This node is the deepest match
        Some(idx)
    }

    fn dispatch_to_node(
        &mut self,
        idx: usize,
        event: &WidgetEvent,
        event_ctx: &mut EventContext,
    ) -> EventResult {
        // When descending into a widget that introduces a scroll-offset, the
        // children see the event with the cursor position shifted into their
        // coordinate space.
        let (ox, oy) = self.nodes[idx].widget.scroll_offset();
        let child_event;
        let child_event_ref: &WidgetEvent = if ox == 0.0 && oy == 0.0 {
            event
        } else {
            child_event = shift_event_position(event, ox, oy);
            &child_event
        };

        // Dispatch to children first (front-to-back: last child on top)
        let children: Vec<usize> = self.nodes[idx].children.clone();
        for &child in children.iter().rev() {
            if self.dispatch_to_node(child, child_event_ref, event_ctx) == EventResult::Consumed {
                return EventResult::Consumed;
            }
        }

        // Then try this node (unshifted event — this node is painted at
        // its original layout_rect, so screen coords apply).
        let node = &self.nodes[idx];
        let layout_rect = self.layout.absolute_rect(node.layout_node);

        // Hit test for mouse events
        if let Some(pos) = event_position(event) {
            if !layout_rect.contains(pos) {
                return EventResult::Ignored;
            }
        }

        // Borrow widget mutably (safe because we're not accessing children here)
        let node = &mut self.nodes[idx];
        node.widget.event(event, layout_rect, event_ctx)
    }

    /// Get the layout rectangle for a widget.
    pub fn layout_rect(&self, idx: usize) -> Rect {
        self.layout.absolute_rect(self.nodes[idx].layout_node)
    }

    /// Get the number of widgets in the tree.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Access the layout engine.
    pub fn layout_engine(&self) -> &LayoutEngine {
        &self.layout
    }

    /// Mutable access to the layout engine.
    pub fn layout_engine_mut(&mut self) -> &mut LayoutEngine {
        &mut self.layout
    }

    /// Access a widget by index.
    pub fn widget(&self, idx: usize) -> &dyn Widget {
        self.nodes[idx].widget.as_ref()
    }

    /// Mutable access to a widget by index.
    pub fn widget_mut(&mut self, idx: usize) -> &mut dyn Widget {
        self.nodes[idx].widget.as_mut()
    }

    /// Get the effective security level for a widget.
    ///
    /// This is `max(parent_effective, widget_declared)` — a child inside
    /// a `Protected` container inherits at least `Protected`.
    pub fn effective_security(&self, idx: usize) -> SecurityLevel {
        self.nodes[idx].effective_security
    }

    /// Get the currently hovered widget index (if any).
    pub fn hovered(&self) -> Option<usize> {
        self.hovered
    }
}

impl Default for WidgetTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the position from events that carry one, for hit testing.
fn event_position(event: &WidgetEvent) -> Option<Point> {
    match event {
        WidgetEvent::MouseDown { position, .. }
        | WidgetEvent::MouseUp { position, .. }
        | WidgetEvent::MouseMove { position }
        | WidgetEvent::Scroll { position, .. } => Some(*position),
        _ => None,
    }
}

/// Return a new event with its position shifted by `(dx, dy)`.
///
/// Used when descending through a widget that introduces a scroll offset, so
/// the children see the event in their own (scrolled) coordinate space.
fn shift_event_position(event: &WidgetEvent, dx: f32, dy: f32) -> WidgetEvent {
    match event {
        WidgetEvent::MouseDown { position, button } => WidgetEvent::MouseDown {
            position: Point::new(position.x + dx, position.y + dy),
            button: *button,
        },
        WidgetEvent::MouseUp { position, button } => WidgetEvent::MouseUp {
            position: Point::new(position.x + dx, position.y + dy),
            button: *button,
        },
        WidgetEvent::MouseMove { position } => WidgetEvent::MouseMove {
            position: Point::new(position.x + dx, position.y + dy),
        },
        WidgetEvent::Scroll {
            position,
            delta_x,
            delta_y,
        } => WidgetEvent::Scroll {
            position: Point::new(position.x + dx, position.y + dy),
            delta_x: *delta_x,
            delta_y: *delta_y,
        },
        other => other.clone(),
    }
}
