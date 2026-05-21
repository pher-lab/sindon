//! Widget tree — manages the widget hierarchy and coordinates layout/paint/events.

use std::collections::HashMap;

use crate::event::{
    EventContext, EventResult, Key, MouseButton, NamedKey, TreeCommand, WidgetEvent,
};
use crate::focus::{FocusDirection, FocusManager};
use crate::layer::{LayerAnchor, LayerEntry, LayerOptions, Placement};
use crate::paint::PaintContext;
use crate::scroll_view::ScrollView;
use crate::shortcut::ShortcutRouter;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Point, Rect, SecurityLevel, Size, Theme};
use shroud_layout::{LayoutEngine, LayoutNodeId};
use shroud_text::TextEngine;

/// An entry in the widget tree.
struct WidgetNode {
    widget: Box<dyn Widget>,
    layout_node: LayoutNodeId,
    children: Vec<usize>,
    /// Upward index used by ancestor walks (hover bubble in
    /// `update_hover_in`, future focus / context queries).
    parent: Option<usize>,
    /// Effective security level: max(parent_effective, self.declared).
    effective_security: SecurityLevel,
    /// Last visibility state applied to Taffy. Used so we only call
    /// `set_style` when `Widget::visible()` actually flips — reverting from
    /// `display: none` back to the widget's real style requires a refresh,
    /// while a steady-state visible node can keep the style installed at
    /// insert time.
    last_applied_visible: bool,
}

/// Manages a tree of widgets with layout integration.
///
/// Widgets are stored in a flat `Vec<Option<WidgetNode>>`. Parent-child
/// relationships are tracked via indices. Each widget has a corresponding
/// Taffy layout node.
///
/// Indices are *stable across removals* — [`Self::remove`] tombstones the
/// slot (sets it to `None`) instead of shifting later entries. A closure
/// that captured an index before a remove keeps pointing at either the
/// still-live widget or a tombstone; it never silently aliases a different
/// widget. Operations on tombstoned indices panic via `widget()` /
/// `layout_rect()` — use [`Self::contains`] or [`Self::try_widget`] when
/// the index may refer to a removed node.
pub struct WidgetTree {
    nodes: Vec<Option<WidgetNode>>,
    root: Option<usize>,
    layout: LayoutEngine,
    /// Index of the widget currently under the cursor (for MouseEnter/Leave).
    hovered: Option<usize>,
    /// Tree-global keyboard focus tracker. Mutated by
    /// [`Self::advance_focus`] (Tab/Shift+Tab) and invalidated by
    /// [`Self::remove`] so a removed widget never stays focused.
    focus: FocusManager,
    /// Deferred initial focus queued by [`Self::focus_initially`]. Consumed
    /// by [`Self::flush_pending_focus`] — typically called once per redraw
    /// by the event loop. One-shot: the field is taken and cleared on flush,
    /// so re-arming requires another `focus_initially` call (e.g. inside a
    /// `replace_screen` build closure).
    pending_initial_focus: Option<usize>,
    /// Active overlay layers (modals, dropdowns, context menus), painted
    /// in push order over the main root. Topmost layer (last push) gets
    /// event priority. While `layers` is non-empty, the main tree
    /// receives no pointer or keyboard input — see [`Self::dispatch_event`].
    layers: Vec<LayerEntry>,
    /// Viewport size from the last layout pass. Held so paint and event
    /// dispatch can place layer roots relative to it without threading a
    /// size through every call site. `(0, 0)` until the first layout.
    viewport: (f32, f32),
    /// App-level keyboard shortcut registry. Consulted at the head of
    /// [`Self::dispatch_event`] for every `KeyDown`; populated via
    /// `AppScope::on_shortcut` after the build closure returns. See
    /// [`crate::shortcut`].
    shortcuts: ShortcutRouter,
}

/// Find the deepest node that appears in both ancestor chains (each
/// leaf-first, root-last). Returns `None` when the chains share no
/// ancestor — happens when the cursor enters a fresh subtree (e.g. the
/// first hover after `clear_hover` returns `self.hovered = None`).
///
/// Linear search rather than a `HashSet`: hover chains are short
/// (typical UI depth < 10), so the constant-factor win matters more
/// than the asymptotic shape.
fn lowest_common_ancestor(old_chain: &[usize], new_chain: &[usize]) -> Option<usize> {
    new_chain.iter().copied().find(|n| old_chain.contains(n))
}

/// Resolve a [`LayerAnchor`] against the freshly-measured layer size and the
/// current viewport, returning the layer's top-left offset in screen
/// coordinates.
///
/// - [`LayerAnchor::ViewportCenter`]: classic centered placement; clamps to
///   `(0, 0)` if the layer is larger than the viewport (the overflow is
///   clipped on the right/bottom).
/// - [`LayerAnchor::AnchorRect`]: places the layer immediately below the
///   trigger rect (or above, per [`Placement`]). The x-coordinate aligns
///   with `rect.x` and is clamped so the layer stays inside the viewport.
///   [`Placement::Auto`] flips above when below would overflow the
///   viewport bottom *and* above would fit; otherwise it falls back to
///   below and the overflow is clipped.
fn place_layer(anchor: LayerAnchor, size: Size, viewport: (f32, f32)) -> (f32, f32) {
    let (vw, vh) = viewport;
    match anchor {
        LayerAnchor::ViewportCenter => (
            ((vw - size.width) * 0.5).max(0.0),
            ((vh - size.height) * 0.5).max(0.0),
        ),
        LayerAnchor::AnchorRect { rect, prefer } => {
            // Horizontal: align left edge to the trigger, then clamp so the
            // popover does not run off either side. `max(0)` after `min(...)`
            // handles the degenerate case where the popover is wider than
            // the viewport — pin to the left and let the right edge clip.
            let x = rect.origin.x.min(vw - size.width).max(0.0);

            let below_y = rect.origin.y + rect.size.height;
            let above_y = rect.origin.y - size.height;
            let y = match prefer {
                Placement::Below => below_y,
                Placement::Above => above_y,
                Placement::Auto => {
                    // Flip only when below overflows AND above fits.
                    // If neither fits we still prefer below — clamping
                    // below moves the popover up over the trigger, which
                    // is the lesser evil compared to clamping above (which
                    // would push the popover *below* the trigger again).
                    let below_fits = below_y + size.height <= vh;
                    let above_fits = above_y >= 0.0;
                    if !below_fits && above_fits {
                        above_y
                    } else {
                        below_y
                    }
                }
            };
            // Vertical clamp so a too-tall popover stays on screen.
            let y = y.min(vh - size.height).max(0.0);
            (x, y)
        }
    }
}

impl WidgetTree {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            root: None,
            layout: LayoutEngine::new(),
            hovered: None,
            focus: FocusManager::new(),
            pending_initial_focus: None,
            layers: Vec::new(),
            viewport: (0.0, 0.0),
            shortcuts: ShortcutRouter::new(),
        }
    }

    /// Mutable access to the app-level shortcut registry.
    ///
    /// Used by `shroud_app::AppScope` to drain queued shortcut
    /// registrations into the tree after the build closure returns.
    /// Apps generally register via `AppScope::on_shortcut` instead of
    /// touching this directly.
    pub fn shortcut_router_mut(&mut self) -> &mut ShortcutRouter {
        &mut self.shortcuts
    }

    /// Queue a focus target to apply on the next [`Self::flush_pending_focus`].
    ///
    /// Built for the boot path and screen transitions: callsites that build
    /// the widget hierarchy do not have an [`EventContext`] and so cannot
    /// dispatch `FocusGained` directly via [`Self::focus`]. They call
    /// `focus_initially(idx)` after the widget is in the tree; the event
    /// loop applies it before the first paint of the new tree.
    ///
    /// One-shot — overwrites any prior pending target. After flush, the
    /// pending field is cleared and a subsequent call is needed to re-arm
    /// (e.g. inside a `replace_screen` build closure that wants to focus
    /// the first input on the new screen).
    pub fn focus_initially(&mut self, idx: usize) {
        self.pending_initial_focus = Some(idx);
    }

    /// Apply any pending initial focus from [`Self::focus_initially`].
    ///
    /// Called by the event loop at the top of each redraw. Cheap when
    /// nothing is pending (single field check). When a target is pending,
    /// dispatches `FocusLost`/`FocusGained` through [`Self::focus`], and
    /// drains any commands those handlers enqueue so the focus change
    /// settles before paint.
    pub fn flush_pending_focus(&mut self, event_ctx: &mut EventContext) {
        let Some(target) = self.pending_initial_focus.take() else {
            return;
        };
        // Race: target tombstoned between `focus_initially` and flush. Skip
        // the focus call entirely — `WidgetTree::focus` would still update
        // the FocusManager pointer to a tombstoned slot otherwise (it only
        // guards the FocusGained dispatch, not the pointer set). One-shot
        // semantics still hold because `take()` cleared the pending field.
        if !self.contains(target) {
            return;
        }
        self.focus(Some(target), event_ctx);
        self.drain_commands(event_ctx);
    }

    /// Add a widget as the root of the tree.
    ///
    /// If a root is already set, it remains as a stranded subtree — use
    /// [`Self::replace_root`] instead when swapping.
    pub fn set_root(&mut self, widget: impl Widget + 'static) -> usize {
        self.set_root_boxed(Box::new(widget))
    }

    /// Add a child widget to the given parent.
    pub fn add_child(&mut self, parent: usize, widget: impl Widget + 'static) -> usize {
        self.add_child_boxed(parent, Box::new(widget))
    }

    /// Replace the current root (and its entire subtree) with a new,
    /// childless root widget.
    ///
    /// All descendants of the old root are dropped in post-order, so
    /// widgets holding secure resources (`SecureInput`, `SecureText`)
    /// zeroize their backing memory as part of the swap. Returns the new
    /// root's index.
    ///
    /// For multi-widget screen transitions, prefer
    /// [`EventContext::replace_screen`] (from an event handler) or build
    /// the replacement tree inline after this call.
    pub fn replace_root(&mut self, widget: impl Widget + 'static) -> usize {
        self.replace_root_boxed(Box::new(widget))
    }

    /// Push an overlay layer with `widget` as its root, returning the
    /// root's index so the caller can populate children via
    /// [`Self::add_child`].
    ///
    /// Layers paint over the main tree in push order (last push = topmost)
    /// and receive events first. While any layer is up, the main tree
    /// receives no pointer or keyboard events — Tab routing is also
    /// trapped inside the topmost layer (see [`Self::focusable_in_tab_order`]).
    ///
    /// The root widget defines the layer's natural size; positioning
    /// inside the viewport follows [`LayerOptions::anchor`]. See
    /// [`LayerOptions::modal`] / [`LayerOptions::popover`] for the
    /// common presets.
    ///
    /// For event-handler driven pushes (e.g. opening a dialog from a
    /// button click), use [`EventContext::push_layer`] instead — this
    /// direct method is for app boot and tests.
    pub fn push_layer(&mut self, options: LayerOptions, widget: impl Widget + 'static) -> usize {
        self.push_layer_boxed(options, Box::new(widget))
    }

    /// Remove the top layer (last pushed), tombstoning its entire subtree.
    ///
    /// No-op when no layer is active. Returns the removed layer's root
    /// index for callers that want to assert or log the dismiss.
    pub fn pop_top_layer(&mut self) -> Option<usize> {
        let entry = self.layers.pop()?;
        let root = entry.root;
        self.remove(root);
        Some(root)
    }

    /// Remove the layer whose root is `root`, regardless of position in
    /// the stack. Tombstones the entire subtree; returns `true` if a
    /// matching layer was found.
    ///
    /// Useful when an event handler captured a specific layer's root
    /// index and wants to dismiss that layer even though another layer
    /// landed on top in the meantime.
    pub fn pop_layer(&mut self, root: usize) -> bool {
        let Some(pos) = self.layers.iter().position(|l| l.root == root) else {
            return false;
        };
        self.layers.remove(pos);
        self.remove(root);
        true
    }

    /// Number of currently active overlay layers.
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// Index of the topmost layer's root, or `None` if no layer is active.
    pub fn top_layer_root(&self) -> Option<usize> {
        self.layers.last().map(|l| l.root)
    }

    /// Viewport-space offset of the topmost layer's root, computed by the
    /// last layout pass. `None` when no layer is active.
    ///
    /// The layer's child rects (via [`Self::layout_rect`]) are reported in
    /// the layer's local Taffy frame; add this offset to translate them
    /// into viewport coordinates (matching what paint and event dispatch
    /// see).
    pub fn top_layer_offset(&self) -> Option<(f32, f32)> {
        self.layers.last().map(|l| l.offset)
    }

    /// Children of `idx` as a fresh `Vec`. Empty when the slot is
    /// tombstoned or has no children.
    ///
    /// Returns a clone of the internal child list to keep the borrow
    /// shape simple for callers that want to traverse on a `&WidgetTree`.
    pub fn children(&self, idx: usize) -> Vec<usize> {
        self.node(idx)
            .map(|n| n.children.clone())
            .unwrap_or_default()
    }

    /// Remove `idx` and every descendant from the tree.
    ///
    /// The slots are tombstoned (left as `None`) rather than compacted, so
    /// surviving indices stay stable. Every widget in the subtree is
    /// dropped in post-order; `Drop` impls fire as expected. If the root
    /// itself is removed, `root` becomes `None`.
    ///
    /// Silently no-ops when `idx` is out of range or already tombstoned,
    /// so repeated removes (e.g. from racing event handlers) don't panic.
    pub fn remove(&mut self, idx: usize) {
        if self.node(idx).is_none() {
            return;
        }

        // Collect the subtree in post-order (leaves first) so we can drop
        // Taffy nodes bottom-up. Taffy tolerates top-down removal too, but
        // bottom-up keeps the intermediate tree valid at every step.
        let mut to_remove = Vec::new();
        self.collect_subtree_postorder(idx, &mut to_remove);

        // Detach from parent's child list (or clear `root` if removing it).
        // Done in two phases so the mut borrow of the parent slot doesn't
        // overlap the immut borrows needed to gather the remaining
        // children's Taffy node ids.
        let parent_idx = self.nodes[idx].as_ref().and_then(|n| n.parent);
        if let Some(p) = parent_idx {
            if let Some(parent) = self.nodes[p].as_mut() {
                parent.children.retain(|&c| c != idx);
            }
            if let Some(parent) = self.node(p) {
                let parent_layout = parent.layout_node;
                let child_ids: Vec<LayoutNodeId> = parent
                    .children
                    .iter()
                    .map(|&i| {
                        self.node(i)
                            .expect("live child under live parent")
                            .layout_node
                    })
                    .collect();
                self.layout.set_children(parent_layout, &child_ids);
            }
        } else if self.root == Some(idx) {
            self.root = None;
        }

        // Clear hover if it landed on something we're about to remove.
        if let Some(h) = self.hovered {
            if to_remove.contains(&h) {
                self.hovered = None;
            }
        }

        // Clear focus if it landed on something we're about to remove —
        // otherwise a later `advance_focus` would find the current index
        // not in the fresh tab order and jump to first/last, which looks
        // fine but masks the bug of the focus pointer dangling on a
        // tombstoned slot.
        if let Some(f) = self.focus.focused() {
            if to_remove.contains(&f) {
                self.focus.set(None);
            }
        }

        // Drop nodes and their Taffy layout nodes. Widget `Drop` runs here
        // — secure widgets zeroize their backing memory.
        for i in &to_remove {
            if let Some(node) = self.nodes[*i].take() {
                self.layout.remove(node.layout_node);
            }
        }

        // Defensive: if any of the removed nodes was a layer root (e.g.
        // a user called `remove` directly instead of `pop_layer`), drop
        // the matching LayerEntry so paint / dispatch don't iterate over
        // a tombstoned root. Preserves the relative order of surviving
        // layers, which determines paint / event priority.
        self.layers.retain(|l| !to_remove.contains(&l.root));
    }

    fn collect_subtree_postorder(&self, idx: usize, out: &mut Vec<usize>) {
        let Some(node) = self.node(idx) else { return };
        // Clone to avoid carrying an immutable borrow while we recurse; the
        // child list is short enough that this is cheap.
        let children: Vec<usize> = node.children.clone();
        for c in children {
            self.collect_subtree_postorder(c, out);
        }
        out.push(idx);
    }

    fn set_root_boxed(&mut self, widget: Box<dyn Widget + 'static>) -> usize {
        let effective_security = widget.security_level();
        let initial_visible = widget.visible();
        let mut style = widget.style();
        if !initial_visible {
            style = style.display_none();
        }
        let layout_node = self.layout.add_leaf(style);
        let idx = self.nodes.len();
        self.nodes.push(Some(WidgetNode {
            widget,
            layout_node,
            children: Vec::new(),
            parent: None,
            effective_security,
            last_applied_visible: initial_visible,
        }));
        self.root = Some(idx);
        idx
    }

    fn add_child_boxed(&mut self, parent: usize, widget: Box<dyn Widget + 'static>) -> usize {
        let parent_security = self
            .node(parent)
            .map(|n| n.effective_security)
            .unwrap_or(SecurityLevel::Normal);
        let effective_security = parent_security.merge(widget.security_level());
        let initial_visible = widget.visible();
        let mut style = widget.style();
        if !initial_visible {
            style = style.display_none();
        }
        if Self::is_scroll_view_idx(&self.nodes, parent) {
            // Direct children of a `ScrollView` must keep their natural
            // intrinsic size — Taffy's default `flex-shrink: 1` on a
            // fixed-height column would otherwise squash them to fit the
            // viewport, defeating the whole point of a scrollable container
            // (this is what `overflow: scroll` does for free in CSS).
            style = style.shrink(0.0);
        }
        let layout_node = self.layout.add_leaf(style);
        let idx = self.nodes.len();
        self.nodes.push(Some(WidgetNode {
            widget,
            layout_node,
            children: Vec::new(),
            parent: Some(parent),
            effective_security,
            last_applied_visible: initial_visible,
        }));

        // Same two-phase dance as remove(): mutate the parent's children
        // vec, then reborrow immutably to gather Taffy node ids.
        if let Some(parent_node) = self.nodes[parent].as_mut() {
            parent_node.children.push(idx);
        }
        if let Some(parent_node) = self.node(parent) {
            let parent_layout = parent_node.layout_node;
            let child_layout_nodes: Vec<LayoutNodeId> = parent_node
                .children
                .iter()
                .map(|&i| self.node(i).expect("live child just pushed").layout_node)
                .collect();
            self.layout.set_children(parent_layout, &child_layout_nodes);
        }

        idx
    }

    fn replace_root_boxed(&mut self, widget: Box<dyn Widget + 'static>) -> usize {
        if let Some(old) = self.root {
            self.remove(old);
        }
        self.set_root_boxed(widget)
    }

    /// Insert `widget` as a parent-less node and register a layer entry
    /// pointing at it. The node lives in the same `nodes` Vec as the main
    /// tree (so `add_child`, `remove`, focus, etc. all work uniformly),
    /// but is not reachable from `self.root` and so won't appear in the
    /// main paint / hit-test walk.
    fn push_layer_boxed(
        &mut self,
        options: LayerOptions,
        widget: Box<dyn Widget + 'static>,
    ) -> usize {
        let effective_security = widget.security_level();
        let initial_visible = widget.visible();
        let mut style = widget.style();
        if !initial_visible {
            style = style.display_none();
        }
        let layout_node = self.layout.add_leaf(style);
        let idx = self.nodes.len();
        self.nodes.push(Some(WidgetNode {
            widget,
            layout_node,
            children: Vec::new(),
            parent: None,
            effective_security,
            last_applied_visible: initial_visible,
        }));
        self.layers.push(LayerEntry {
            root: idx,
            options,
            offset: (0.0, 0.0),
            measured_size: Size::ZERO,
        });
        // Cursor is no longer "over" any main-tree widget once a layer
        // takes input priority — clear so a stale MouseLeave doesn't fire
        // at the wrong time when the layer pops.
        self.hovered = None;
        idx
    }

    /// Apply queued tree commands from an event context.
    ///
    /// Called by [`Self::dispatch_event`] (via [`Self::drain_commands`])
    /// after the walk returns. Kept crate-private because the command enum
    /// itself is internal. `event_ctx` is threaded through because
    /// `TreeCommand::Focus` re-enters [`Self::focus`], which dispatches
    /// `FocusLost`/`FocusGained`.
    fn apply_commands(&mut self, commands: Vec<TreeCommand>, event_ctx: &mut EventContext) {
        for cmd in commands {
            match cmd {
                TreeCommand::AddChild { parent, widget } => {
                    if self.node(parent).is_some() {
                        self.add_child_boxed(parent, widget);
                    }
                    // Else: parent already removed — silently drop. Widget
                    // runs its Drop (zeroize) as the Box goes out of scope.
                }
                TreeCommand::Remove { idx } => {
                    self.remove(idx);
                }
                TreeCommand::ReplaceRoot { widget } => {
                    self.replace_root_boxed(widget);
                }
                TreeCommand::ReplaceScreen { build } => {
                    if let Some(old) = self.root {
                        self.remove(old);
                    }
                    build(self);
                }
                TreeCommand::RebuildChildren { parent, build } => {
                    // Skip if a prior command has already tombstoned the
                    // parent. Matches AddChild's best-effort semantics.
                    if self.node(parent).is_none() {
                        continue;
                    }
                    // Snapshot the current child list; `remove` mutates the
                    // parent's children vec in place, so iterating by clone
                    // keeps the loop stable.
                    let children: Vec<usize> = self
                        .node(parent)
                        .map(|n| n.children.clone())
                        .unwrap_or_default();
                    for c in children {
                        self.remove(c);
                    }
                    build(self, parent);
                }
                TreeCommand::Focus { target } => {
                    // Skip the call entirely if the target was tombstoned
                    // between `EventContext::focus(idx)` and this drain —
                    // `WidgetTree::focus` only guards the FocusGained
                    // dispatch, not the FocusManager pointer set. `None`
                    // (= blur) always passes through. FocusGained handlers
                    // may enqueue further commands; `drain_commands` keeps
                    // draining until the queue settles.
                    if let Some(idx) = target {
                        if !self.contains(idx) {
                            continue;
                        }
                    }
                    self.focus(target, event_ctx);
                }
                TreeCommand::PushLayer {
                    options,
                    root_widget,
                    populate,
                } => {
                    let layer_root = self.push_layer_boxed(options, root_widget);
                    populate(self, layer_root);
                }
                TreeCommand::PopLayer { root } => {
                    self.pop_layer(root);
                }
                TreeCommand::PopTopLayer => {
                    self.pop_top_layer();
                }
            }
        }
    }

    /// Drain the command queue until empty, applying each batch.
    ///
    /// Handlers (notably `FocusGained`/`FocusLost` via `TreeCommand::Focus`,
    /// and any user code that enqueues commands inside a screen-rebuild
    /// closure) can themselves enqueue more commands. The drain loop keeps
    /// going until no new commands arrive or a hard cap fires — the cap
    /// exists only as a safety belt against pathological cycles; well-formed
    /// handlers settle in 1–2 iterations.
    fn drain_commands(&mut self, event_ctx: &mut EventContext) {
        const MAX_ITERATIONS: usize = 64;
        let mut iterations = 0;
        loop {
            let commands = event_ctx.take_commands();
            if commands.is_empty() {
                return;
            }
            iterations += 1;
            // Cycle protection. Debug build trips the assert so tests catch
            // it; release build silently drops the residual queue. No `log`
            // dep here — keeps the widget crate's deps minimal.
            debug_assert!(
                iterations <= MAX_ITERATIONS,
                "WidgetTree::drain_commands exceeded {} iterations \u{2014} likely a focus/event cycle in a handler",
                MAX_ITERATIONS,
            );
            if iterations > MAX_ITERATIONS {
                return;
            }
            self.apply_commands(commands, event_ctx);
        }
    }

    fn node(&self, idx: usize) -> Option<&WidgetNode> {
        self.nodes.get(idx).and_then(|slot| slot.as_ref())
    }

    fn node_mut(&mut self, idx: usize) -> Option<&mut WidgetNode> {
        self.nodes.get_mut(idx).and_then(|slot| slot.as_mut())
    }

    /// Compute layout for the entire tree (no intrinsic-size measurement).
    ///
    /// Use this for tests or for trees where no widget needs to report its
    /// intrinsic size. Leaf widgets like `TextWidget` or `Button` will be
    /// sized purely by their flex style — so centering them via
    /// `Container::column().center()` will collapse them to width 0 unless
    /// a fixed width is set. For the general case, prefer
    /// [`Self::compute_layout_with_measure`].
    ///
    /// Active layers are each laid out against the viewport with their
    /// anchor-derived offset cached for paint and event dispatch.
    pub fn compute_layout(&mut self, width: f32, height: f32) {
        self.viewport = (width, height);
        self.refresh_visibility_styles();
        if let Some(root) = self.root {
            let root_node = self
                .node(root)
                .expect("root stays populated between set_root and remove")
                .layout_node;
            self.layout.compute(root_node, width, height);
        }
        let layer_root_nodes: Vec<(usize, LayoutNodeId)> = self
            .layers
            .iter()
            .map(|l| {
                (
                    l.root,
                    self.node(l.root).expect("live layer root").layout_node,
                )
            })
            .collect();
        for (_, root_node) in &layer_root_nodes {
            self.layout.compute(*root_node, width, height);
        }
        for (i, (root, root_node)) in layer_root_nodes.iter().enumerate() {
            let layout_rect = self.layout.absolute_rect(*root_node);
            let size = layout_rect.size;
            let offset = place_layer(self.layers[i].options.anchor, size, (width, height));
            self.layers[i].measured_size = size;
            self.layers[i].offset = offset;
            let _ = root;
        }

        self.sync_scroll_view_content_heights();
    }

    /// Returns whether the node at `idx` holds a `ScrollView` widget. Used
    /// when adding children / re-applying styles to keep direct children
    /// of a scroll container unshrinkable.
    fn is_scroll_view_idx(nodes: &[Option<WidgetNode>], idx: usize) -> bool {
        nodes
            .get(idx)
            .and_then(|slot| slot.as_ref())
            .map(|n| {
                let w: &dyn Widget = n.widget.as_ref();
                (w as &dyn std::any::Any).is::<ScrollView>()
            })
            .unwrap_or(false)
    }

    /// Re-apply Taffy styles for widgets whose `visible()` flipped since the
    /// last layout pass. A widget that went hidden gets `display: none`; one
    /// that came back visible gets its real style re-installed. Stable-visible
    /// nodes are left alone (their install-time style still applies) to avoid
    /// redundant `set_style` churn every frame.
    ///
    /// Children of a `ScrollView` keep the `flex-shrink: 0` override that
    /// `add_child_boxed` installed at insert time — re-emit it so a widget
    /// going visible-again does not start shrinking under a fixed-height
    /// scroll viewport.
    fn refresh_visibility_styles(&mut self) {
        // Snapshot dirty indices first so we can read `is_scroll_view_idx`
        // (borrows `self.nodes`) without overlapping the per-node mut borrow
        // we need for `set_style`.
        let dirty: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                let node = slot.as_ref()?;
                (node.widget.visible() != node.last_applied_visible).then_some(i)
            })
            .collect();
        for i in dirty {
            let parent_is_scroll = self.nodes[i]
                .as_ref()
                .and_then(|n| n.parent)
                .map(|p| Self::is_scroll_view_idx(&self.nodes, p))
                .unwrap_or(false);
            let Some(node) = self.nodes[i].as_mut() else {
                continue;
            };
            let vis = node.widget.visible();
            let mut style = node.widget.style();
            if parent_is_scroll {
                style = style.shrink(0.0);
            }
            let style = if vis { style } else { style.display_none() };
            let layout_node = node.layout_node;
            node.last_applied_visible = vis;
            self.layout.set_style(layout_node, style);
        }
    }

    /// Compute layout, consulting `Widget::measure` for every leaf.
    ///
    /// This is the path the real event loop uses. It lets `TextWidget` and
    /// `Button` report their natural size based on their shaped content, so
    /// flex centering / gap / grow work without wrapper containers.
    ///
    /// Each active overlay layer is laid out as its own independent root
    /// against the viewport; the resulting size + anchor-derived offset
    /// is cached on the `LayerEntry` for paint and event dispatch.
    pub fn compute_layout_with_measure(
        &mut self,
        width: f32,
        height: f32,
        text_engine: &mut TextEngine,
        theme: &Theme,
    ) {
        self.viewport = (width, height);
        self.refresh_visibility_styles();

        // Invalidate Taffy's measure cache. Taffy memoizes leaf measure
        // results by (node, available_width, available_height); when a
        // reactive widget's *content* changes but its style and the viewport
        // don't, Taffy would otherwise reuse the stale size. Reproducer: the
        // counter example — clicking past 9 leaves "Count: 10" laid out at
        // the cached "Count: 0" width, so paint re-shapes with a too-narrow
        // max_width and wraps. Marking each node dirty per pass forces re-
        // measure. Cost is negligible at our tree sizes.
        for n in self.nodes.iter().flatten() {
            self.layout.mark_dirty(n.layout_node);
        }

        // Reverse-lookup Taffy node → widget index. Built fresh every layout
        // pass; cheap for the tree sizes we target, and keeps us from having
        // to stash indices as Taffy node-context. Shared across the main
        // root and every layer root pass — measure logic is the same
        // closure either way.
        let node_map: HashMap<LayoutNodeId, usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|n| (n.layout_node, i)))
            .collect();

        // Collect Taffy roots up front so the immut `&self.nodes` borrow
        // can drop before we call `&mut self.layout` for compute.
        let main_root_node = self.root.and_then(|r| self.node(r)).map(|n| n.layout_node);
        let layer_root_nodes: Vec<(usize, LayoutNodeId)> = self
            .layers
            .iter()
            .map(|l| {
                (
                    l.root,
                    self.node(l.root).expect("live layer root").layout_node,
                )
            })
            .collect();

        if let Some(root_node) = main_root_node {
            let nodes = &self.nodes;
            self.layout
                .compute_with_measure(root_node, width, height, |node_id, query| {
                    measure_node(&node_map, nodes, text_engine, theme, node_id, query)
                });
        }

        // Each layer root is its own Taffy root with the viewport as
        // available space. The widget's flex style controls how it
        // grows / shrinks inside that space (e.g. fixed-width modal vs.
        // content-sized popover).
        for (_, root_node) in &layer_root_nodes {
            let nodes = &self.nodes;
            self.layout
                .compute_with_measure(*root_node, width, height, |node_id, query| {
                    measure_node(&node_map, nodes, text_engine, theme, node_id, query)
                });
        }

        // Update each layer's offset + measured size from the layout
        // result. Reads must come after compute so the rects are fresh.
        for (i, (root, root_node)) in layer_root_nodes.iter().enumerate() {
            let layout_rect = self.layout.absolute_rect(*root_node);
            let size = layout_rect.size;
            let offset = place_layer(self.layers[i].options.anchor, size, (width, height));
            self.layers[i].measured_size = size;
            self.layers[i].offset = offset;
            let _ = root;
        }

        self.sync_scroll_view_content_heights();
    }

    /// Walk every live widget; for each `ScrollView`, sum the relative
    /// bottoms of its visible direct children and write the result (plus the
    /// scroll view's bottom padding) back into its `auto_content_height`.
    ///
    /// Called at the end of both `compute_layout` and
    /// `compute_layout_with_measure` so callers who never pinned an explicit
    /// content height get a scrollable extent that tracks the laid-out
    /// children — wrapped text, dynamic lists, markdown previews all "just
    /// scroll" without the caller having to guess the right slack value.
    ///
    /// Children's `engine.layout(...).origin.y` is relative to the parent's
    /// border-box origin and already includes the scroll view's top padding,
    /// so we only need to add the bottom padding to keep the bottom margin
    /// flush with the last child when fully scrolled.
    fn sync_scroll_view_content_heights(&mut self) {
        let candidates: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                let node = slot.as_ref()?;
                let w: &dyn Widget = node.widget.as_ref();
                (w as &dyn std::any::Any)
                    .downcast_ref::<ScrollView>()
                    .map(|_| i)
            })
            .collect();

        for sv_idx in candidates {
            let Some(node) = self.node(sv_idx) else {
                continue;
            };
            let children = node.children.clone();
            let mut max_bottom: f32 = 0.0;
            for child in &children {
                let Some(child_node) = self.node(*child) else {
                    continue;
                };
                if !child_node.widget.visible() {
                    continue;
                }
                let rect = self.layout.layout(child_node.layout_node);
                let bottom = rect.origin.y + rect.size.height;
                if bottom > max_bottom {
                    max_bottom = bottom;
                }
            }

            let sv_node = self
                .node_mut(sv_idx)
                .expect("scroll-view slot vanished between read and write");
            let widget_any: &mut dyn std::any::Any = sv_node.widget.as_mut();
            if let Some(sv) = widget_any.downcast_mut::<ScrollView>() {
                let bottom_pad = sv.measured_bottom_padding();
                sv.set_measured_content_height(max_bottom + bottom_pad);
            }
        }
    }

    /// Paint the entire widget tree, returning draw commands.
    ///
    /// Order:
    /// 1. Main root subtree (if any) — drawn first so layers appear on top.
    /// 2. Each layer in push order; before painting, the layer's batch
    ///    boundary is recorded via [`PaintContext::begin_layer`] so the
    ///    renderer can flush rects → glyphs *within* the layer before
    ///    starting the next layer. Without this the main tree's text
    ///    would draw on top of layer backgrounds (rect/glyph pipelines
    ///    are flushed once each, so paint order within the same pipeline
    ///    is preserved, but glyphs always overdraw all rects globally).
    ///    For each layer: full-viewport scrim rect (if configured),
    ///    then the layer subtree shifted by its anchor offset so the
    ///    painted rect lands at the right viewport position.
    pub fn paint(&self, ctx: &mut PaintContext) {
        if let Some(root) = self.root {
            self.paint_node(root, ctx);
        }
        for layer in &self.layers {
            ctx.begin_layer();
            if let Some(scrim) = layer.options.scrim {
                let (vw, vh) = self.viewport;
                ctx.fill_rect(Rect::new(0.0, 0.0, vw, vh), scrim);
            }
            let (ox, oy) = layer.offset;
            ctx.push_offset(ox, oy);
            self.paint_node(layer.root, ctx);
            ctx.pop_offset();
        }
    }

    fn paint_node(&self, idx: usize, ctx: &mut PaintContext) {
        let Some(node) = self.node(idx) else {
            return;
        };
        // `display: none` already collapses the subtree in layout, but we
        // also skip paint so reactive paint-time work (colors, text shaping)
        // is not spent on an invisible widget.
        if !node.widget.visible() {
            return;
        }
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
    ///
    /// When at least one overlay layer is active, dispatch is constrained
    /// to the topmost layer's subtree — the main tree (and lower layers)
    /// see no pointer or keyboard input. Pointer events outside the layer's
    /// interactive rect trigger the layer's dismiss-on-outside-click path
    /// (when configured) and are otherwise swallowed; `Escape` dismisses
    /// the topmost layer when its `dismiss_on_escape` flag is set.
    ///
    /// After the walk completes, drains any tree mutations that handlers
    /// queued on the context (see [`EventContext::add_child`] and friends).
    pub fn dispatch_event(
        &mut self,
        event: &WidgetEvent,
        event_ctx: &mut EventContext,
    ) -> EventResult {
        // Pick the subtree that owns this event: the topmost layer if any,
        // otherwise the main root. `offset` translates viewport coords into
        // the subtree's local space (zero for the main tree).
        let (target_root, offset, layer_active) = match self.layers.last() {
            Some(layer) => (Some(layer.root), layer.offset, true),
            None => (self.root, (0.0, 0.0), false),
        };

        let result = self.dispatch_with_target(target_root, offset, layer_active, event, event_ctx);

        // Drain tree-mutation commands queued by handlers. Loops because
        // `TreeCommand::Focus` re-enters `Widget::event` (FocusLost/Gained)
        // and those handlers can themselves enqueue more commands —
        // `drain_commands` keeps going until the queue settles.
        self.drain_commands(event_ctx);

        result
    }

    fn dispatch_with_target(
        &mut self,
        target_root: Option<usize>,
        offset: (f32, f32),
        layer_active: bool,
        event: &WidgetEvent,
        event_ctx: &mut EventContext,
    ) -> EventResult {
        let Some(target) = target_root else {
            return EventResult::Ignored;
        };

        // App-level keyboard shortcuts. Checked before the Escape and
        // Tab interceptors so a registered binding wins over both layer
        // dismiss and focus navigation (the router itself skips raw
        // Tab/Enter/Escape — see `is_reserved_bare_key`).
        if matches!(event, WidgetEvent::KeyDown { .. }) {
            let accepts_text = self
                .focus
                .focused()
                .and_then(|i| self.try_widget(i))
                .map(|w| w.accepts_text())
                .unwrap_or(false);
            let layer_blocks = self
                .layers
                .last()
                .map(|l| l.options.block_shortcuts)
                .unwrap_or(false);
            if self.shortcuts.try_dispatch(
                event,
                event_ctx.modifiers,
                accepts_text,
                layer_blocks,
                event_ctx,
            ) {
                return EventResult::Consumed;
            }
        }

        // Escape dismisses the topmost layer when configured. Checked
        // first so dismiss-aware modal flows aren't accidentally swallowed
        // by a focused Input's KeyDown handler.
        if layer_active
            && matches!(
                event,
                WidgetEvent::KeyDown {
                    key: Key::Named(NamedKey::Escape)
                }
            )
        {
            let top = self.layers.last().expect("layer_active implies a layer");
            if top.options.dismiss_on_escape {
                self.pop_top_layer();
                return EventResult::Consumed;
            }
        }

        // Tab / Shift+Tab: intercept before the normal walk. The tree
        // rotates [`FocusManager`] to the next or previous focusable
        // widget and dispatches `FocusLost` + `FocusGained` to the two
        // affected widgets; no widget sees the raw Tab KeyDown. Shift
        // state comes from the modifier snapshot maintained by the
        // event loop on `ModifiersChanged`. `focusable_in_tab_order`
        // already walks the topmost layer's subtree when one is active,
        // so Tab is trapped inside the layer without extra plumbing.
        if matches!(
            event,
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Tab)
            }
        ) {
            let dir = if event_ctx.modifiers.shift {
                FocusDirection::Backward
            } else {
                FocusDirection::Forward
            };
            self.advance_focus(dir, event_ctx);
            return EventResult::Consumed;
        }

        // Pointer events: when a layer is up, route based on whether the
        // cursor is inside the layer's interactive rect. Outside hits
        // either dismiss (configurable) or are silently swallowed, but
        // never fall through to the main tree.
        if layer_active {
            if let Some(pos) = event_position(event) {
                let local_pos = Point::new(pos.x - offset.0, pos.y - offset.1);
                let layer_size = self.layers.last().unwrap().measured_size;
                let layer_rect = Rect::new(0.0, 0.0, layer_size.width, layer_size.height);
                if !layer_rect.contains(local_pos) {
                    // Outside the layer's content rect.
                    if let WidgetEvent::MouseMove { .. } = event {
                        // Cursor over scrim / margin: drop any layer-side
                        // hover (so widgets get a final MouseLeave) but
                        // don't dismiss on hover.
                        self.clear_hover(event_ctx);
                        return EventResult::Ignored;
                    }
                    if let WidgetEvent::MouseDown { .. } = event {
                        let dismiss = self
                            .layers
                            .last()
                            .map(|l| l.options.dismiss_on_outside_click)
                            .unwrap_or(false);
                        if dismiss {
                            self.pop_top_layer();
                        }
                    }
                    // MouseUp / Scroll / other position-bearing events
                    // outside the layer are silently swallowed.
                    return EventResult::Consumed;
                }
            }
        }

        // From here on, events are dispatched against `target`. Position-
        // bearing events are pre-shifted by `-offset` so widget callbacks
        // see positions in the subtree's local coordinate space (matching
        // their `absolute_rect`, which Taffy already reports relative to
        // the subtree root).
        let shifted_owned;
        let shifted: &WidgetEvent = if offset == (0.0, 0.0) {
            event
        } else {
            shifted_owned = shift_event_position(event, -offset.0, -offset.1);
            &shifted_owned
        };

        // Generate MouseEnter/MouseLeave on cursor movement within the
        // target subtree.
        if let WidgetEvent::MouseMove { position } = shifted {
            self.update_hover_in(target, *position, event_ctx);
        }

        // Click-to-focus: route focus before the widget sees MouseDown so
        // its handler observes a consistent `focused` flag. Scoped to the
        // target subtree — clicking on a layer never refocuses something
        // behind it. Only the **primary** (Left) button drives focus —
        // matches web/native behavior where right-click opens a context
        // menu without stealing focus from the currently focused element.
        if let WidgetEvent::MouseDown {
            position,
            button: MouseButton::Left,
        } = shifted
        {
            let hit = self.hit_test_in(target, *position);
            let new_focus = hit.filter(|&idx| {
                self.node(idx)
                    .map(|n| n.widget.focusable())
                    .unwrap_or(false)
            });
            self.focus(new_focus, event_ctx);
        }

        self.dispatch_to_node(target, shifted, event_ctx)
    }

    /// Index of the widget that currently has keyboard focus, if any.
    ///
    /// Updated by [`Self::advance_focus`] (Tab/Shift+Tab) and cleared
    /// automatically when the focused widget is removed from the tree.
    pub fn focused(&self) -> Option<usize> {
        self.focus.focused()
    }

    /// Collect focusable widgets in tab order — DFS pre-order over the
    /// active subtree, skipping invisible branches.
    ///
    /// Invisible widgets collapse the entire subtree (matching
    /// `display: none` layout semantics), so a focusable child inside a
    /// hidden container is not reachable. When at least one layer is
    /// active, traversal walks the topmost layer's subtree only — this
    /// is what traps Tab inside an open modal. Exposed for tests and
    /// for callers that want to implement custom traversal on top of
    /// the primitive set.
    pub fn focusable_in_tab_order(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let start = self.top_layer_root().or(self.root);
        if let Some(root) = start {
            self.collect_focusable(root, &mut out);
        }
        out
    }

    fn collect_focusable(&self, idx: usize, out: &mut Vec<usize>) {
        let Some(node) = self.node(idx) else { return };
        if !node.widget.visible() {
            return;
        }
        if node.widget.focusable() {
            out.push(idx);
        }
        // Clone to avoid holding an immut borrow across the recursion;
        // child lists are short enough that this is cheap.
        let children: Vec<usize> = node.children.clone();
        for c in children {
            self.collect_focusable(c, out);
        }
    }

    /// Move keyboard focus one step in `dir`, wrapping at the ends.
    ///
    /// Returns the newly focused widget index, or `None` if the tree
    /// has no focusable widgets. Dispatches `FocusLost` to the
    /// previously focused widget (if any) and `FocusGained` to the new
    /// one; both events go through the same `event` path as input
    /// events, so handlers can queue tree mutations via `event_ctx`.
    ///
    /// Invoked by the tree's own Tab routing — callers rarely need
    /// this directly, but it is public so tests and custom shortcut
    /// bindings can reuse the traversal policy.
    pub fn advance_focus(
        &mut self,
        dir: FocusDirection,
        event_ctx: &mut EventContext,
    ) -> Option<usize> {
        let order = self.focusable_in_tab_order();
        if order.is_empty() {
            return None;
        }

        let next = match (self.focus.focused(), dir) {
            (None, FocusDirection::Forward) => order[0],
            (None, FocusDirection::Backward) => *order.last().unwrap(),
            (Some(cur), dir) => {
                // Look up `cur` in the fresh order: it may not appear
                // if the widget was hidden or marked non-focusable
                // since the last traversal. In that case fall back to
                // the direction's end so Tab still advances.
                match order.iter().position(|&i| i == cur) {
                    Some(pos) => {
                        let n = order.len();
                        let step = match dir {
                            FocusDirection::Forward => (pos + 1) % n,
                            FocusDirection::Backward => (pos + n - 1) % n,
                        };
                        order[step]
                    }
                    None => match dir {
                        FocusDirection::Forward => order[0],
                        FocusDirection::Backward => *order.last().unwrap(),
                    },
                }
            }
        };

        self.focus(Some(next), event_ctx);
        Some(next)
    }

    /// Set focus to `new`, dispatching `FocusLost` to the previously
    /// focused widget (if any) and `FocusGained` to the new one.
    ///
    /// Returns the previously focused index, for callers that want to
    /// chain restore / log / test-assert on the transition.
    ///
    /// This is the one-and-only focus-change entrypoint — both the
    /// built-in Tab routing and click-to-focus path funnel through here,
    /// and apps call it directly for programmatic focus (e.g. focusing
    /// the first input after a screen transition).
    ///
    /// If the target equals the current focus, the call is a no-op and
    /// no events fire. Passing an index that is tombstoned or out-of-range
    /// silently skips dispatch for that side, which keeps the call safe
    /// against races with a removal.
    pub fn focus(&mut self, new: Option<usize>, event_ctx: &mut EventContext) -> Option<usize> {
        let prev = self.focus.set(new);
        if prev == new {
            return prev;
        }

        if let Some(p) = prev {
            if let Some(n) = self.node(p) {
                let rect = self.layout.absolute_rect(n.layout_node);
                if let Some(node) = self.node_mut(p) {
                    node.widget.event(&WidgetEvent::FocusLost, rect, event_ctx);
                }
            }
        }

        if let Some(n) = new {
            if let Some(nd) = self.node(n) {
                let rect = self.layout.absolute_rect(nd.layout_node);
                if let Some(node) = self.node_mut(n) {
                    node.widget
                        .event(&WidgetEvent::FocusGained, rect, event_ctx);
                }
            }
        }

        prev
    }

    /// Update the tree-wide `hovered` pointer using a hit-test rooted at
    /// `subtree_root`, and emit `MouseEnter` / `MouseLeave` on the
    /// affected widgets. `pos` is in the subtree's local coordinate
    /// space (callers translate any layer offset before invoking).
    ///
    /// Bubbles the events up the ancestor chain, DOM-style: every
    /// ancestor that newly contains (or no longer contains) the cursor
    /// also gets a MouseEnter / MouseLeave, stopping at the lowest
    /// common ancestor so widgets that span both the old and new path
    /// stay quietly hovered. Lets a hoverable `Container` light up when
    /// the cursor enters a child `Button` inside it.
    ///
    /// `new_hover` may be `None` when the cursor is over the subtree's
    /// area but missed every visible widget (e.g. on a layer's scrim or
    /// padding), in which case the previous hover chain receives final
    /// `MouseLeave`s and the pointer clears.
    fn update_hover_in(&mut self, subtree_root: usize, pos: Point, event_ctx: &mut EventContext) {
        let new_hover = self.hit_test_in(subtree_root, pos);

        if new_hover == self.hovered {
            return;
        }

        let old_chain = self.ancestors_inclusive(self.hovered);
        let new_chain = self.ancestors_inclusive(new_hover);
        let lca = lowest_common_ancestor(&old_chain, &new_chain);

        // MouseLeave: every node in the old chain up to (but not
        // including) the LCA, leaf first so deeper widgets see leave
        // before their parents.
        for &idx in &old_chain {
            if Some(idx) == lca {
                break;
            }
            self.emit_to(idx, &WidgetEvent::MouseLeave, event_ctx);
        }

        // MouseEnter: every node in the new chain up to (but not
        // including) the LCA, outer-most first so a parent's hover
        // state is set before its child reacts.
        let enter_stop = new_chain.iter().position(|&idx| Some(idx) == lca);
        let enter_slice = match enter_stop {
            Some(stop) => &new_chain[..stop],
            None => &new_chain[..],
        };
        for &idx in enter_slice.iter().rev() {
            self.emit_to(idx, &WidgetEvent::MouseEnter, event_ctx);
        }

        self.hovered = new_hover;
    }

    /// Clear the hovered pointer, emitting `MouseLeave` to every widget
    /// in the current hover chain (leaf first). Used when the cursor
    /// leaves the topmost layer's interactive rect (the scrim acts like
    /// "nothing is hovered" from the widget's perspective).
    fn clear_hover(&mut self, event_ctx: &mut EventContext) {
        if let Some(old_idx) = self.hovered.take() {
            let chain = self.ancestors_inclusive(Some(old_idx));
            for idx in chain {
                self.emit_to(idx, &WidgetEvent::MouseLeave, event_ctx);
            }
        }
    }

    /// Collect `idx` and all its ancestors, leaf first. Returns an
    /// empty vec when `idx` is `None`. Stops at any tombstoned node
    /// (defense-in-depth — `remove` invalidates `hovered`, so a
    /// dangling index should not arise in practice).
    fn ancestors_inclusive(&self, idx: Option<usize>) -> Vec<usize> {
        let mut chain = Vec::new();
        let mut cur = idx;
        while let Some(i) = cur {
            let Some(node) = self.node(i) else { break };
            chain.push(i);
            cur = node.parent;
        }
        chain
    }

    /// Send `event` to the widget at `idx`, fetching its layout rect
    /// for the dispatch. Silent if the slot is tombstoned. Used by the
    /// hover-bubble path which already filtered indices through
    /// `ancestors_inclusive`.
    fn emit_to(&mut self, idx: usize, event: &WidgetEvent, event_ctx: &mut EventContext) {
        let Some(rect) = self
            .node(idx)
            .map(|n| self.layout.absolute_rect(n.layout_node))
        else {
            return;
        };
        if let Some(n) = self.node_mut(idx) {
            n.widget.event(event, rect, event_ctx);
        }
    }

    /// Hit-test starting from `subtree_root`. `pos` is in that subtree's
    /// local coordinate space — for layer dispatch the caller subtracts
    /// the layer's anchor offset before calling.
    fn hit_test_in(&self, subtree_root: usize, pos: Point) -> Option<usize> {
        self.hit_test_node(subtree_root, pos)
    }

    fn hit_test_node(&self, idx: usize, pos: Point) -> Option<usize> {
        let node = self.node(idx)?;
        if !node.widget.visible() {
            return None;
        }
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
        let Some(node) = self.node(idx) else {
            return EventResult::Ignored;
        };
        if !node.widget.visible() {
            return EventResult::Ignored;
        }
        // When descending into a widget that introduces a scroll-offset, the
        // children see the event with the cursor position shifted into their
        // coordinate space.
        let (ox, oy) = node.widget.scroll_offset();
        let child_event;
        let child_event_ref: &WidgetEvent = if ox == 0.0 && oy == 0.0 {
            event
        } else {
            child_event = shift_event_position(event, ox, oy);
            &child_event
        };

        // Dispatch to children first (front-to-back: last child on top)
        let children: Vec<usize> = node.children.clone();
        for &child in children.iter().rev() {
            if self.dispatch_to_node(child, child_event_ref, event_ctx) == EventResult::Consumed {
                return EventResult::Consumed;
            }
        }

        // Then try this node (unshifted event — this node is painted at
        // its original layout_rect, so screen coords apply).
        let Some(node) = self.node(idx) else {
            return EventResult::Ignored;
        };
        let layout_rect = self.layout.absolute_rect(node.layout_node);

        // Hit test for mouse events
        if let Some(pos) = event_position(event) {
            if !layout_rect.contains(pos) {
                return EventResult::Ignored;
            }
        }

        // Borrow widget mutably (safe because we're not accessing children here)
        let Some(node) = self.node_mut(idx) else {
            return EventResult::Ignored;
        };
        node.widget.event(event, layout_rect, event_ctx)
    }

    /// Get the layout rectangle for a widget.
    ///
    /// Panics if `idx` is out of range or has been tombstoned by a prior
    /// remove. Use [`Self::contains`] to check liveness first.
    pub fn layout_rect(&self, idx: usize) -> Rect {
        let node = self
            .node(idx)
            .expect("layout_rect called with stale or out-of-range index");
        self.layout.absolute_rect(node.layout_node)
    }

    /// Number of live widgets in the tree (tombstoned slots excluded).
    pub fn len(&self) -> usize {
        self.nodes.iter().filter(|s| s.is_some()).count()
    }

    /// Whether the tree has any live widgets. Tombstoned slots do not
    /// count, so a tree that had its root removed reports `true` here.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `idx` currently refers to a live widget.
    ///
    /// Useful when a captured index may have been tombstoned by a prior
    /// remove; check this before calling [`Self::widget`] or similar
    /// panicking accessors.
    pub fn contains(&self, idx: usize) -> bool {
        self.node(idx).is_some()
    }

    /// Access the layout engine.
    pub fn layout_engine(&self) -> &LayoutEngine {
        &self.layout
    }

    /// Mutable access to the layout engine.
    pub fn layout_engine_mut(&mut self) -> &mut LayoutEngine {
        &mut self.layout
    }

    /// Access a widget by index. Panics if the slot is tombstoned or
    /// out of range — use [`Self::try_widget`] for a checked variant.
    pub fn widget(&self, idx: usize) -> &dyn Widget {
        self.node(idx)
            .expect("widget called with stale or out-of-range index")
            .widget
            .as_ref()
    }

    /// Checked accessor for a widget by index. Returns `None` if the slot
    /// has been tombstoned by a prior remove.
    pub fn try_widget(&self, idx: usize) -> Option<&dyn Widget> {
        self.node(idx).map(|n| n.widget.as_ref())
    }

    /// Mutable access to a widget by index. Panics if the slot is
    /// tombstoned or out of range.
    pub fn widget_mut(&mut self, idx: usize) -> &mut dyn Widget {
        self.node_mut(idx)
            .expect("widget_mut called with stale or out-of-range index")
            .widget
            .as_mut()
    }

    /// Typed accessor: borrow a widget as a concrete type `T` via runtime
    /// downcast. Returns `None` if the slot is tombstoned, out of range,
    /// or holds a different concrete widget type. Intended primarily for
    /// tests and introspection — production code should already know what
    /// widget lives at a given index.
    pub fn widget_as<T: Widget>(&self, idx: usize) -> Option<&T> {
        let w = self.try_widget(idx)?;
        (w as &dyn std::any::Any).downcast_ref::<T>()
    }

    /// Get the effective security level for a widget.
    ///
    /// This is `max(parent_effective, widget_declared)` — a child inside
    /// a `Protected` container inherits at least `Protected`.
    pub fn effective_security(&self, idx: usize) -> SecurityLevel {
        self.node(idx)
            .expect("effective_security called with stale or out-of-range index")
            .effective_security
    }

    /// Get the currently hovered widget index (if any).
    pub fn hovered(&self) -> Option<usize> {
        self.hovered
    }

    /// Index of the current root widget, if set.
    pub fn root(&self) -> Option<usize> {
        self.root
    }
}

impl Default for WidgetTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared measure callback for `compute_with_measure`. Factored out so the
/// main-root and per-layer compute calls use identical logic without
/// duplicating the closure body in `compute_layout_with_measure`.
fn measure_node(
    node_map: &HashMap<LayoutNodeId, usize>,
    nodes: &[Option<WidgetNode>],
    text_engine: &mut TextEngine,
    theme: &Theme,
    node_id: LayoutNodeId,
    query: shroud_layout::MeasureQuery,
) -> Size {
    let Some(&widget_idx) = node_map.get(&node_id) else {
        return Size::ZERO;
    };
    let Some(node) = nodes[widget_idx].as_ref() else {
        return Size::ZERO;
    };
    let constraint = query.known_width.or(query.available_width);
    let widget = node.widget.as_ref();
    let mut ctx = MeasureContext::new(text_engine, theme);
    widget.measure(constraint, &mut ctx).unwrap_or(Size::ZERO)
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
